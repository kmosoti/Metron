//! SDK observations on inherited stderr, outside deterministic runtime authority.
use metron_inference_local::{
    artifact,
    chunk_prefill::SchedulerLimits,
    resources::{self, ResourceRecord, quantity, unavailable},
};
use metron_protocol::{LocalDevice, LocalProfile, MAX_ENVELOPE, MAX_LOCAL_CONTEXT};
use mistralrs::Device;
use mistralrs_core::{MemoryUsage, Pipeline};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, sync::OnceLock};

pub(crate) fn aggregate(device: &Device) -> Value {
    let mut result = json!({
        "scope": "whole device including other processes; allocator counters are worker CUDA pools",
        "source": "SDK MemoryUsage and Linux /proc/self/status",
        "process_gpu_bytes": unavailable("bytes", "no process-scoped GPU sampler"),
        "allocator": unavailable("bytes", "CUDA allocator query unsupported or unavailable")
    });
    match MemoryUsage.query(device) {
        Ok(memory) => {
            result["device_total"] =
                quantity(memory.total() as u64, "bytes", "SDK whole-device total");
            result["device_free"] = quantity(
                memory.physical_available() as u64,
                "bytes",
                "SDK whole-device physical free",
            );
            result["device_used"] = quantity(
                memory.allocated() as u64,
                "bytes",
                "SDK device aggregate; includes other processes",
            );
        }
        Err(_) => {
            for key in ["device_total", "device_free", "device_used"] {
                result[key] = unavailable("bytes", "SDK device memory query failed");
            }
        }
    }
    #[cfg(feature = "cuda")]
    if let Ok(Some(allocator)) = MemoryUsage.query_cuda_allocator(device) {
        result["allocator"] = json!({
            "scope":"worker CUDA pools; may overlap device usage",
            "async_pool": allocator.async_pool.map(|p| json!({
                "used": quantity(p.current.used as u64,"bytes","SDK CUDA async pool current"),
                "reserved":quantity(p.current.reserved as u64,"bytes","SDK CUDA async pool current"),
                "used_high":quantity(p.used_high as u64,"bytes","SDK CUDA async pool high-water counter"),
                "reserved_high":quantity(p.reserved_high as u64,"bytes","SDK CUDA async pool high-water counter")
            })).unwrap_or_else(|| unavailable("bytes","async pool unavailable")),
            "graph_pool": allocator.graph_pool.map(|p| json!({
                "used":quantity(p.used as u64,"bytes","SDK CUDA graph pool current"),
                "reserved":quantity(p.reserved as u64,"bytes","SDK CUDA graph pool current"),
                "used_high":quantity(p.used_high as u64,"bytes","SDK CUDA graph pool high-water counter"),
                "reserved_high":quantity(p.reserved_high as u64,"bytes","SDK CUDA graph pool high-water counter")
            })).unwrap_or_else(|| unavailable("bytes","graph pool unavailable"))
        });
    }
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| {
                    line.split_whitespace()
                        .nth(1)?
                        .parse::<u64>()
                        .ok()?
                        .checked_mul(1024)
                })
        });
    result["process_rss"] = rss
        .map(|bytes| quantity(bytes, "bytes", "Linux /proc/self/status VmRSS"))
        .unwrap_or_else(|| unavailable("bytes", "process RSS unavailable"));
    result
}

pub(crate) fn actual_device_uuid(device: &Device) -> Option<String> {
    #[cfg(feature = "cuda")]
    if let Device::Cuda(cuda) = device {
        let uuid = cuda.cuda_stream().context().uuid().ok()?;
        let hex: String = uuid
            .bytes
            .iter()
            .map(|b| format!("{:02x}", *b as u8))
            .collect();
        return Some(format!(
            "GPU-{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        ));
    }
    let _ = device;
    None
}

fn geometry(pipeline: &dyn Pipeline) -> Value {
    let metadata = pipeline.get_metadata();
    let mut rows = Vec::new();
    let mut storage = BTreeMap::new();
    let mut invalid = false;
    if let Some(engine) = &metadata.cache_engine {
        for (layer, (k, v)) in engine.get_kv_cache().iter().enumerate() {
            if k.elem_count() == 0 && v.elem_count() == 0 {
                continue;
            }
            let mut tensors = Vec::new();
            for (name, tensor) in [("k", k), ("v", v)] {
                let bytes =
                    (tensor.elem_count() as u64).checked_mul(tensor.dtype().size_in_bytes() as u64);
                // The public storage guard identifies shared allocation ownership. These are
                // full pool tensors allocated by CacheEngine, not arbitrary sliced views.
                let (guard, layout) = tensor.storage_and_layout();
                let key = std::ptr::from_ref(&*guard).addr();
                if !layout.is_contiguous() || layout.start_offset() != 0 {
                    invalid = true;
                }
                if let Some(previous) = storage.insert(key, bytes) {
                    invalid |= previous != bytes;
                }
                invalid |= bytes.is_none();
                tensors.push(json!({"name":name,"dtype":format!("{:?}",tensor.dtype()),"shape":tensor.dims(),"device":format!("{:?}",tensor.device()),"tensor_payload": bytes.map(|b|quantity(b,"bytes","full SDK pool tensor shape times scalar width")).unwrap_or_else(|| unavailable("bytes","tensor geometry overflow"))}));
            }
            rows.push(json!({"layer":layer,"tensors":tensors}));
        }
    }
    let total = if invalid {
        None
    } else {
        storage
            .values()
            .try_fold(0_u64, |sum, bytes| sum.checked_add((*bytes)?))
    };
    let pool = if metadata.cache_engine.is_none() {
        unavailable("bytes", "unpaged route has no preallocated paged K/V pool")
    } else {
        total
            .map(|bytes| {
                quantity(
                    bytes,
                    "bytes",
                    "deduplicated full SDK pool tensor payload; excludes allocator overhead",
                )
            })
            .unwrap_or_else(|| {
                unavailable(
                    "bytes",
                    "pool storage attribution or checked arithmetic unavailable",
                )
            })
    };
    json!({
        "activation_dtype":format!("{:?}",metadata.activation_dtype),
        "route":if metadata.cache_config.is_some() {"paged"} else {"unpaged"},
        "attention_pool_layer_count":rows.len(), "layers":rows,
        "allocated_pool_tensor_payload":pool,
        "logical_retained_prefill_payload":unavailable("bytes","idle observation cannot establish prefill logical retained cache or peak"),
        "usable_capacity": metadata.cache_config.as_ref().map(|c|quantity(c.usable_tokens() as u64,"tokens","realized SDK pool excluding reserved block zero"))
            .unwrap_or_else(||unavailable("tokens","unpaged cache has no fixed page capacity")),
        "block_size":metadata.cache_config.as_ref().map(|c|quantity(c.block_size as u64,"tokens","SDK CacheConfig"))
            .unwrap_or_else(||unavailable("tokens","unpaged")),
        "gpu_blocks":metadata.cache_config.as_ref().map(|c|quantity(c.num_gpu_blocks as u64,"count","SDK CacheConfig includes reserved block zero"))
            .unwrap_or_else(||unavailable("count","unpaged")),
        "cache_type":metadata.cache_config.as_ref().map(|c|format!("{:?}",c.cache_type)),
        "dtype_basis":"activation dtype is metadata; KV dtype is read from each actual pool tensor; unpaged prefill KV dtype unavailable at idle"
    })
}

pub(crate) fn record(
    profile: &LocalProfile,
    device: &Device,
    pipeline: Option<&dyn Pipeline>,
    scheduler: Option<SchedulerLimits>,
    phase: &'static str,
) -> ResourceRecord {
    static BUILD: OnceLock<Option<String>> = OnceLock::new();
    let build = BUILD.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|path| artifact::sha256_file(&path).ok())
    });
    let observed = aggregate(device);
    let unknown_runtime = unavailable(
        "bytes",
        "SDK does not isolate exclusive runtime/workspace resident allocation",
    );
    let metadata = pipeline.map(|p| p.get_metadata());
    let mut kv = pipeline
        .map(geometry)
        .unwrap_or_else(|| json!({"unavailable_reason":"model not loaded"}));
    kv["configured_target"] = profile
        .cache_target_bytes
        .map(|v| {
            quantity(
                v.get(),
                "bytes",
                "profile KV planning target, not a hard allocator cap",
            )
        })
        .unwrap_or_else(|| unavailable("bytes", "no configured KV pool target"));
    let plan = metadata.as_ref().and_then(|m|m.memory_plan).map(|p|json!({
        "status":"planning_reservation",
        "primary_activation":quantity(p.primary_activation_bytes as u64,"bytes","SDK RuntimeMemoryPlan estimate"),
        "mapped_activation":quantity(p.mapped_activation_bytes as u64,"bytes","SDK RuntimeMemoryPlan estimate"),
        "primary_non_attention":quantity(p.primary_non_attention_bytes as u64,"bytes","SDK RuntimeMemoryPlan estimate"),
        "mapped_non_attention":quantity(p.mapped_non_attention_bytes as u64,"bytes","SDK RuntimeMemoryPlan estimate"),
        "encoder_activation":quantity(p.encoder_activation_bytes as u64,"bytes","SDK RuntimeMemoryPlan estimate"),
        "planned_query_tokens":quantity(p.planned_query_tokens as u64,"tokens","SDK RuntimeMemoryPlan"),
        "logits_bytes_per_query_token":quantity(p.logits_bytes_per_query_token as u64,"bytes/token","SDK RuntimeMemoryPlan"),
        "custom_prefill_mask":p.custom_prefill_mask,
        "planning_limits":{"max_sequences":p.limits.max_num_seqs,"max_batched_tokens":p.limits.max_num_batched_tokens,"max_prefill_chunk_tokens":p.limits.max_prefill_chunk_tokens}
    })).unwrap_or_else(||json!({"unavailable_reason":"SDK runtime plan unavailable"}));
    ResourceRecord {
        phase,
        identity: json!({"worker_build_digest":build,"profile_digest":profile.digest().ok(),"artifact_digest":profile.artifact_digest,
            "declared_device_uuid": match &profile.device {LocalDevice::Cuda{uuid,..}=>Some(uuid.as_str()), _=>None},
            "observed_device_uuid":actual_device_uuid(device),"device":format!("{device:?}"),
            "configured_context_tokens":profile.context_tokens,"configured_output_tokens":profile.max_output_tokens,"configured_sequences":profile.max_sequences,
            "realized_scheduler_mode":if metadata.is_none(){"not_loaded"}else if scheduler.is_some(){"paged_with_chunk_limit"}else{"unpaged"},
            "route_basis":"idle SDK metadata and realized scheduler configuration; forward participation not directly traced"}),
        weights: json!({"artifact_storage_bytes":quantity(profile.artifact_bytes.get(),"bytes","verified GGUF file size"),
            "tensor_payload_estimate_bytes": if profile.artifact_digest.as_str()=="3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4" {quantity(1_091_380_480,"bytes","descriptor inventory of verified pinned Q4_K_M GGUF; packed storage, not GPU residency")}else{unavailable("bytes","no descriptor inventory for this artifact")},
            "resident_gpu_bytes":unavailable("bytes","SDK public metadata does not isolate resident weight allocation")}),
        load_baseline: json!({"scope":"model plus runtime", "phase":phase,
            "snapshot_ref":if phase=="loaded_idle"{json!("aggregate; same-phase inclusive observation")}else{json!({"unavailable_reason":"loaded-idle baseline not observed at this phase"})}}),
        kv,
        runtime: json!({"exclusive_resident_bytes":unknown_runtime,"workspace_peak_bytes":unavailable("bytes","no isolated workspace peak counter"),
            "recurrent_initial_tensor_estimate":metadata.as_ref().and_then(|m|resources::lfm3_recurrent_initial_estimate(m.activation_dtype.size_in_bytes() as u64).ok()).map(|bytes|quantity(bytes,"bytes","source estimate:13 layers,9initial slots,2048x7 convolution plus1placeholder per slot; not measured residency")).unwrap_or_else(||unavailable("bytes","actual scalar width unavailable")),
            "recurrent_resident_bytes":unavailable("bytes","no independently attributed recurrent resident measurement"),"sdk_plan":plan}),
        aggregate: observed,
        limits: json!({"context_tokens":quantity(profile.context_tokens as u64,"tokens","enforced profile ceiling"),
            "input_tokens":quantity(profile.max_input_tokens as u64,"tokens","enforced rendered input ceiling"),
            "output_tokens":quantity(profile.max_output_tokens as u64,"tokens","enforced profile output ceiling"),
            "request_context":quantity(MAX_LOCAL_CONTEXT as u64,"bytes","enforced protocol bound"),
            "reply_frame":quantity(MAX_ENVELOPE as u64,"bytes","enforced worker reply envelope bound"),
            "request_frame":quantity(metron_protocol::MAX_LOCAL_REQUEST_FRAME as u64,"bytes","enforced worker input frame bound"),
            "internal_serialization":quantity(metron_protocol::MAX_INTERNAL as u64,"bytes","enforced journal/internal serialization bound"),
            "coding_prompt_serialization":quantity(16*1024,"bytes","enforced coding_runtime prompt serialization bound"),
            "coding_selection_serialization":quantity(48*1024,"bytes","enforced coding_runtime selection serialization bound"),
            "trained_context_tokens":quantity(resources::TRAINED_CONTEXT_TOKENS,"tokens","pinned model ceiling; not physical fit"),
            "realized_model_context": metadata.as_ref().map(|m|quantity(m.max_model_len as u64,"tokens","SDK resolved context ceiling")).unwrap_or_else(||unavailable("tokens","not loaded")),
            "kv_target_status":"planning_target", "strict_allocation_cap":"unsupported","fit_status":"not_evaluated",
            "scheduler":scheduler.map(|s|json!({"max_sequences":s.max_sequences,"max_batched_tokens":s.batched_tokens,"max_prefill_chunk_tokens":s.prefill_chunk_tokens,"basis":"realized paged scheduler after first-party clamps"}))}),
    }
}

pub(crate) fn emit(
    profile: &LocalProfile,
    device: &Device,
    pipeline: Option<&dyn Pipeline>,
    scheduler: Option<SchedulerLimits>,
    phase: &'static str,
) {
    let mut record = record(profile, device, pipeline, scheduler, phase);
    let bytes = match record.encode() {
        Ok(bytes) => bytes,
        Err(reason) => {
            record.kv = json!({"unavailable_reason":reason});
            record.runtime = json!({"unavailable_reason":reason});
            match record.encode() {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("metron resource observation unavailable: {error}");
                    return;
                }
            }
        }
    };
    let mut stderr = std::io::stderr().lock();
    if let Err(error) = stderr
        .write_all(&bytes)
        .and_then(|()| stderr.write_all(b"\n"))
    {
        eprintln!("metron resource observation write failed: {error}");
    }
}
