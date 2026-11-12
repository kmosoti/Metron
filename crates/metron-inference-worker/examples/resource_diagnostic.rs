//! Bounded, one-process/one-route diagnostic. This never qualifies numerical equivalence.
#![forbid(unsafe_code)]
#[path = "../src/model_loading.rs"]
mod model_loading;
#[path = "../src/resources.rs"]
mod resources;
use either::Either;
use metron_inference_local::resources::{TRAINED_CONTEXT_TOKENS, lfm3_fit_lower_bound};
use metron_protocol::{LocalDevice, LocalProfile, SqlU64, content_digest};
use mistralrs::{
    Constraint, CustomLogitsProcessor, DType, Device, RequestBuilder, SamplingParams, Tensor,
    TextMessageRole, TextMessages,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};

fn messages(content: &str) -> TextMessages {
    TextMessages::new().add_message(TextMessageRole::User, content)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        return Err("usage: resource_diagnostic PROFILE_JSON ARTIFACT unpaged|paged 128|513 MINIMUM_FREE_BYTES".into());
    }
    if std::env::var("MISTRALRS_CUDA_GRAPHS").as_deref() != Ok("0") {
        return Err("MISTRALRS_CUDA_GRAPHS=0 is required before SDK initialization".into());
    }
    let target: usize = args[4].parse()?;
    if !matches!(target, 128 | 513) || !matches!(args[3].as_str(), "unpaged" | "paged") {
        return Err("only bounded128/513 and unpaged/paged routes are supported".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&args[1])?
        .take((metron_protocol::MAX_ENVELOPE + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > metron_protocol::MAX_ENVELOPE {
        return Err("profile input too large".into());
    }
    let original: LocalProfile = serde_json::from_slice(&bytes)?;
    original.validate()?;
    let mut profile = original.clone();
    profile.context_tokens = 1024;
    profile.max_input_tokens = 1023;
    profile.max_output_tokens = 1;
    profile.max_sequences = 1;
    profile.scoped_prefix_cache = false;
    profile.chunked_prefill = args[3] == "paged";
    profile.cache_target_bytes = if profile.chunked_prefill {
        Some(SqlU64::new(64 * 1024 * 1024)?)
    } else {
        None
    };
    profile.validate()?;
    if profile.artifact_digest.as_str()
        != "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4"
        || !matches!(profile.device, LocalDevice::Cuda { .. })
    {
        return Err("diagnostic is restricted to pinned Q4_K_M on CUDA".into());
    }
    let minimum_free: u64 = args[5].parse()?;
    if minimum_free == 0 {
        return Err("explicit positive physical headroom preflight required".into());
    }
    let device = Device::new_cuda(0)?;
    let memory = mistralrs_core::MemoryUsage.query(&device)?;
    if (memory.physical_available() as u64) < minimum_free {
        return Err("physical free VRAM below diagnostic preflight threshold".into());
    }
    let declared_uuid = match &profile.device {
        LocalDevice::Cuda { uuid, .. } => uuid.as_str(),
        _ => unreachable!(),
    };
    if resources::actual_device_uuid(&device).as_deref() != Some(declared_uuid) {
        return Err("physical CUDA UUID does not match profile".into());
    }
    let loaded = model_loading::load(&profile, Path::new(&args[2])).await?;
    let (activation_dtype, loaded_record, cache) = {
        let pipeline = loaded.pipeline.lock().await;
        let metadata = pipeline.get_metadata();
        let dtype = metadata.activation_dtype;
        if metadata.is_xlora
            || metadata.no_kv_cache
            || !pipeline.cache().is_hybrid()
            || !loaded.device.is_cuda()
        {
            return Err(
                "source-derived route predicates failed: require CUDA/non-XLora/hybrid/KV enabled"
                    .into(),
            );
        }
        if !matches!(dtype, DType::F16 | DType::BF16 | DType::F32) {
            return Err("unqualified actual activation dtype".into());
        }
        if profile.chunked_prefill
            && (loaded.scheduler.is_none() || pipeline.get_metadata().cache_engine.is_none())
        {
            return Err("paged scheduler/cache metadata unavailable".into());
        }
        (
            dtype,
            serde_json::from_slice::<Value>(
                &resources::record(
                    &profile,
                    &loaded.device,
                    Some(&*pipeline),
                    loaded.scheduler,
                    "loaded_idle",
                )
                .encode()?,
            )?,
            pipeline.cache().clone(),
        )
    };
    if profile.chunked_prefill {
        let layers = loaded_record["kv"]["layers"]
            .as_array()
            .ok_or("actual paged layer geometry unavailable")?;
        if layers.len() != 11
            || loaded_record["kv"]["allocated_pool_tensor_payload"]["value"]
                .as_u64()
                .is_none()
            || layers.iter().any(|layer| {
                layer["tensors"].as_array().is_none_or(|tensors| {
                    tensors.len() != 2
                        || tensors
                            .iter()
                            .any(|tensor| tensor["dtype"] != format!("{activation_dtype:?}"))
                })
            })
        {
            return Err(
                "actual eleven-layer paged pool geometry/dtype unavailable or inconsistent".into(),
            );
        }
    }
    // Construct the exact rendered boundary in at most eight tokenization-only attempts.
    let mut repetitions = target.saturating_sub(8);
    let mut selected = None;
    for _ in 0..8 {
        let content = " a".repeat(repetitions);
        let tokens = loaded
            .model
            .tokenize(Either::Left(messages(&content)), None, true, true, None)
            .await?;
        if tokens.len() == target {
            selected = Some((content, tokens));
            break;
        }
        if tokens.len() < target {
            repetitions += target - tokens.len();
        } else {
            repetitions = repetitions
                .checked_sub(tokens.len() - target)
                .ok_or("token construction underflow")?;
        }
    }
    let (content, tokens) =
        selected.ok_or("could not construct exact bounded rendered token count")?;
    let rendered = loaded.model.detokenize(tokens.clone(), false).await?;
    let token_bytes: Vec<u8> = tokens
        .iter()
        .flat_map(|token| token.to_le_bytes())
        .collect();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);
    let callback_device = loaded.device.clone();
    let unpaged = !profile.chunked_prefill;
    let processor: Arc<dyn CustomLogitsProcessor> = Arc::new(
        move |logits: &Tensor, context: &[u32]| {
            let values = logits.to_vec1::<f32>()?;
            let snapshot = resources::aggregate(&callback_device);
            // LFM3's forward_hidden cache guard and clone_out_cache guard have
            // returned before sampling. This never locks the Pipeline mutex.
            let logical_kv = if unpaged && cache.is_hybrid() {
                let guard = cache.hybrid();
                let mut layers = Vec::new();
                let mut logical_bytes = 0_u64;
                for (layer, entry) in guard.caches.iter().enumerate() {
                    if let Some(kv) = entry.as_kv_cache()
                        && let (Some(k), Some(v)) = (kv.k()?, kv.v()?)
                    {
                        let bytes = [&k, &v].into_iter().try_fold(0_u64, |sum, t| {
                            sum.checked_add(
                                (t.elem_count() as u64)
                                    .checked_mul(t.dtype().size_in_bytes() as u64)?,
                            )
                        });
                        if let Some(bytes) = bytes {
                            logical_bytes = logical_bytes
                                .checked_add(bytes)
                                .ok_or_else(|| std::io::Error::other("logical KV overflow"))?;
                        } else {
                            return Err(std::io::Error::other("logical KV overflow").into());
                        }
                        layers.push(json!({"layer":layer,"k_dtype":format!("{:?}",k.dtype()),"v_dtype":format!("{:?}",v.dtype()),"k_shape":k.dims(),"v_shape":v.dims()}));
                    }
                }
                json!({"layers":layers,"logical_tensor_payload_bytes":logical_bytes,"basis":"current retained K/V views after completed forward; not allocation capacity/resident bytes/peak"})
            } else {
                json!({"unavailable_reason":"paged logical block occupancy not exposed by public pipeline metadata; fixed pool observed at load"})
            };
            captured_clone
                .lock()
                .map_err(|_| std::io::Error::other("diagnostic capture poisoned"))?
                .push((values, context.to_vec(), snapshot, logical_kv));
            Ok(logits.clone())
        },
    );
    let mut sampling = SamplingParams::neutral();
    sampling.temperature = Some(0.0);
    sampling.top_k = Some(1);
    sampling.top_p = Some(1.0);
    sampling.min_p = Some(0.0);
    sampling.repetition_penalty = Some(1.0);
    sampling.frequency_penalty = Some(0.0);
    sampling.presence_penalty = Some(0.0);
    sampling.max_len = Some(1);
    sampling.n_choices = 1;
    sampling.ignore_eos = true;
    sampling.logits_bias = None;
    sampling.dry_params = None;
    let request = RequestBuilder::new()
        .add_message(TextMessageRole::User, &content)
        .set_constraint(Constraint::None)
        .set_sampling(sampling)
        .add_logits_processor(processor);
    let response = loaded.model.send_chat_request(request).await?;
    let (logits, context, prefill_aggregate, logical_kv) = {
        let mut captures = captured.lock().map_err(|_| "diagnostic capture poisoned")?;
        if captures.len() != 1 {
            return Err("expected exactly one next-token logits callback".into());
        }
        captures.pop().unwrap()
    };
    if context != tokens || logits.len() != 128_000 || logits.iter().any(|value| !value.is_finite())
    {
        return Err("logits/context identity, shape, or finiteness check failed".into());
    }
    if unpaged
        && logical_kv["layers"].as_array().is_none_or(|layers| {
            layers.len() != 11
                || layers.iter().any(|layer| {
                    layer["k_dtype"] != format!("{activation_dtype:?}")
                        || layer["v_dtype"] != format!("{activation_dtype:?}")
                })
        })
    {
        return Err(
            "actual unpaged eleven-layer KV dtype/logical snapshot unavailable or inconsistent"
                .into(),
        );
    }
    let inferred_chunks = loaded.scheduler.map(|s| {
        let chunk = s.prefill_chunk_tokens.min(s.batched_tokens);
        (0..tokens.len())
            .step_by(chunk)
            .map(|start| (tokens.len() - start).min(chunk))
            .collect::<Vec<_>>()
    });
    let logit_bytes: Vec<u8> = logits
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let top_token = logits
        .iter()
        .enumerate()
        .max_by(|(ai, a), (bi, b)| a.total_cmp(b).then_with(|| bi.cmp(ai)))
        .map(|(i, _)| i)
        .ok_or("empty logits")?;
    let end_record = {
        let pipeline = loaded.pipeline.lock().await;
        serde_json::from_slice::<Value>(
            &resources::record(
                &profile,
                &loaded.device,
                Some(&*pipeline),
                loaded.scheduler,
                "diagnostic_end",
            )
            .encode()?,
        )?
    };
    let output = json!({
        "kind":"metron_resource_diagnostic","schema_version":1,"status":"diagnostic_only",
        "numeric_tolerance":{"status":"not_qualified","reason":"no defensible frozen full-model dtype-specific tolerance"},
        "process_id":std::process::id(),"route":args[3],"profile":profile,"original_profile_digest":original.digest()?,
        "actual_activation_dtype":format!("{activation_dtype:?}"),"sequence_count":1,"max_completion_tokens":1,
        "return_raw_logits":false,"prefix_cache":false,"cuda_graphs":false,
        "diagnostic_sampling_overrides":{"temperature":0.0,"top_k":1,"top_p":1.0,"min_p":0.0,"repetition_penalty":1.0,"presence_penalty":0.0,"frequency_penalty":0.0,"logits_bias":null,"dry_params":null,"constraint":"none","ignore_eos":true,"max_len":1,"n_choices":1},
        "sampling_basis":"matched identity CustomLogitsProcessor; repeat1, frequency/presence0, no bias/DRY/constraint; callback disables CUDA sampling optimization; no speed claim",
        "execution_participation":{"status":"unavailable","reason":"SDK public metadata exposes realized scheduler/configuration but no per-forward chunk participation trace"},
        "route_inference":{"status":"source_derived","basis":"pinned engine628/scheduler298,372,232: CUDA non-XLora hybrid KV-enabled fresh singleton PromptAndDecode plain-text Chat; normal request rawfalse/suffixNone; no prefix/media/speculative path; scheduler min(prefill,batch) bounds each recurrent prompt chunk","expected_paged_prompt_chunk_sizes":inferred_chunks,
            "asserted_public_predicates":{"cuda":true,"is_xlora":false,"no_kv_cache":false,"hybrid":true,"fresh_process":true,"sequence_count":1,"plain_text_chat":true,"return_raw_logits":false,"suffix":null,"prefix_reuse":false,"media":false,"speculative_model":false}},
        "minimum_free_bytes":minimum_free,"preflight_device_free_bytes":memory.physical_available(),
        "rendered_tokens":tokens.len(),"input_content":content,"rendered":rendered,"rendered_digest":content_digest(rendered.as_bytes()),
        "token_ids":tokens,"token_ids_digest":content_digest(&token_bytes),"captured_context":context,
        "logits_f32":logits,"logits_digest":content_digest(&logit_bytes),"top_token":top_token,"finite":true,
        "completion_tokens":response.usage.completion_tokens,"loaded_observation":loaded_record,
        "prefill_aggregate":prefill_aggregate,"prefill_logical_kv":logical_kv,"end_observation":end_record,
        "full_context_negative_control":lfm3_fit_lower_bound(memory.total() as u64,TRAINED_CONTEXT_TOKENS,activation_dtype.size_in_bytes() as u64)?
    });
    serde_json::to_writer(std::io::stdout().lock(), &output)?;
    std::io::stdout().write_all(b"\n")?;
    Ok(())
}
