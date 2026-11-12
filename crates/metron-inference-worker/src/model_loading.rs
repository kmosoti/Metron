//! Verified model loading shared by the worker and its bounded diagnostic.
use crate::resources;
use metron_inference_local::{
    artifact::{self, Artifact},
    chunk_prefill::{self, SchedulerLimits},
};
use metron_protocol::{LOCAL_TEAM_RECIPE_SHA, LocalDevice, LocalProfile, is_local_coding_recipe};
use mistralrs::{
    Device, DeviceLayerMapMetadata, DeviceMapMetadata, GgufModelBuilder, MemoryGoal, Model,
    PagedAttentionConfig, PagedCacheType, PrefixCacheCapacity, SchedulerConfig, TokenSource,
};
use mistralrs_core::Pipeline;
use std::{path::Path, sync::Arc};
const RECIPE_SHA: &str = "7b8725c7b6328da6a97f487991ca56ac09b05e484a8d8738384c22645a0931a4";

pub(crate) struct LoadedModel {
    pub model: Model,
    pub pipeline: Arc<tokio::sync::Mutex<dyn Pipeline>>,
    pub device: Device,
    pub scheduler: Option<SchedulerLimits>,
}
/// Removes only the verified output from a fresh private directory owned by this worker.
pub(crate) struct PreparedTokenizerGuard {
    pub(crate) directory: std::path::PathBuf,
    pub(crate) file: Option<(std::path::PathBuf, String)>,
}
impl PreparedTokenizerGuard {
    fn create() -> Result<Self, String> {
        use std::os::unix::fs::DirBuilderExt;
        let directory =
            std::env::temp_dir().join(format!("metron-lfm3-tokenizer-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|_| "private tokenizer directory unavailable")?;
        Ok(Self {
            directory,
            file: None,
        })
    }
}
impl Drop for PreparedTokenizerGuard {
    fn drop(&mut self) {
        if let Some((path, sha)) = &self.file {
            let verified = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
                && artifact::sha256_file(path).is_ok_and(|actual| &actual == sha);
            if verified {
                let _ = std::fs::remove_file(path);
            }
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}

pub(crate) async fn load(profile: &LocalProfile, path: &Path) -> Result<LoadedModel, String> {
    let kv_target = chunk_prefill::kv_target(profile)?;
    if !([RECIPE_SHA, LOCAL_TEAM_RECIPE_SHA].contains(&profile.prompt_recipe_digest.as_str())
        || is_local_coding_recipe(profile.prompt_recipe_digest.as_str()))
    {
        return Err("unsupported profile setting".into());
    }
    let directory = path.parent().ok_or("invalid artifact path")?;
    let artifact = match profile.artifact_name.as_str() {
        "LFM3-1.7B-Q4_K_M.gguf" => Artifact::Control,
        "LFM3-1.7B-QAD-Q4_0.gguf" => Artifact::Challenger,
        "LFM3-1.7B-Q6_K.gguf" => Artifact::Q6K,
        "LFM3-1.7B-Q8_0.gguf" => Artifact::Q8Zero,
        _ => return Err("unsupported artifact".into()),
    };
    if path.file_name().and_then(|x| x.to_str()) != Some(artifact.filename()) {
        return Err("artifact path mismatch".into());
    }
    artifact::verify_artifact(path, artifact).map_err(|_| "artifact verification failed")?;
    for entry in std::fs::read_dir(directory).map_err(|_| "model directory unreadable")? {
        let entry = entry.map_err(|_| "model directory unreadable")?;
        let name = entry.file_name();
        if name != "auxiliary"
            && name != Artifact::Control.filename()
            && name != Artifact::Challenger.filename()
            && name != Artifact::Q6K.filename()
            && name != Artifact::Q8Zero.filename()
        {
            return Err("unexpected file in model root; embedded tokenizer required".into());
        }
    }
    let auxiliary = directory
        .join("auxiliary")
        .join(profile.auxiliary_revision.as_str());
    for aux in artifact::AUXILIARY_FILES {
        artifact::verify_auxiliary(&auxiliary.join(aux.filename), *aux)
            .map_err(|_| "auxiliary verification failed")?;
    }
    let corrected_tokenizer = matches!(
        profile.prompt_recipe_digest.as_str(),
        metron_protocol::LOCAL_CODING_V3_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V4_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V5_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V6_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V7_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V8_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V9_RECIPE_SHA
            | metron_protocol::LOCAL_CODING_V10_RECIPE_SHA
    );
    let tokenizer_identity = if corrected_tokenizer {
        metron_protocol::gguf_lfm3_corrected_tokenizer_digest(&profile.artifact_digest)
    } else {
        metron_protocol::gguf_embedded_tokenizer_digest(&profile.artifact_digest)
    }
    .map_err(|_| "invalid embedded tokenizer binding")?;
    if profile.embedded_tokenizer_digest != tokenizer_identity
        || profile.template_digest.as_str()
            != "f45005c50cd2a97329722bf3a2dee9f16c04cdf825a589b280fb7e99e6827818"
    {
        return Err("tokenizer or template digest mismatch".into());
    }
    let _template = artifact::read_verified_template(&auxiliary).map_err(|_| "invalid template")?;
    let mut builder = GgufModelBuilder::new(directory.to_string_lossy(), vec![artifact.filename()])
        .with_chat_template(auxiliary.join("chat_template.jinja").to_string_lossy())
        .with_token_source(TokenSource::None)
        .with_max_model_len(profile.context_tokens as usize)
        .with_max_num_seqs(profile.max_sequences as usize)
        .with_prefix_cache_n(PrefixCacheCapacity::DISABLED);
    builder = match kv_target {
        Some(bytes) => builder.with_paged_attn(
            PagedAttentionConfig::new(None, MemoryGoal::KvBytes(bytes), PagedCacheType::Auto)
                .map_err(|error| format!("invalid paged KV target: {error:#}"))?,
        ),
        None => builder.without_paged_attn(),
    };
    let mut tokenizer_guard = if corrected_tokenizer {
        Some(PreparedTokenizerGuard::create()?)
    } else {
        None
    };
    if let Some(guard) = &mut tokenizer_guard {
        let prepared = metron_inference_local::tokenizer::prepare_lfm3_tokenizer(
            path,
            &auxiliary.join("tokenizer.json"),
            &guard.directory,
        )?;
        builder = builder.with_tokenizer_json(prepared.path.to_string_lossy());
        eprintln!("metron corrected tokenizer sha256: {}", prepared.sha256);
        guard.file = Some((prepared.path, prepared.sha256));
    }
    let device = match &profile.device {
        LocalDevice::Cpu => Device::Cpu,
        LocalDevice::Cuda { .. } => {
            Device::new_cuda(0).map_err(|error| format!("CUDA device unavailable: {error:#}"))?
        }
    };
    resources::emit(profile, &device, None, None, "before_load");
    builder = match &profile.device {
        LocalDevice::Cpu => builder.with_force_cpu(),
        LocalDevice::Cuda { .. } => builder.with_device(device.clone()).with_device_mapping(
            DeviceMapMetadata::from_num_device_layers(vec![DeviceLayerMapMetadata {
                ordinal: 0,
                layers: 24,
            }]),
        ),
    };
    let (pipeline, mut scheduler, add_model_config) =
        mistralrs::model_builder_trait::build_gguf_pipeline(builder)
            .await
            .map_err(|error| format!("model load failed: {error:#}"))?;
    let mut effective_scheduler = None;
    if let Some(bytes) = kv_target {
        let SchedulerConfig::PagedAttentionMeta {
            max_num_seqs,
            max_num_batched_tokens,
            max_prefill_chunk_tokens,
            ..
        } = &mut scheduler
        else {
            return Err("chunked prefill requires a realized paged scheduler".into());
        };
        let limits = chunk_prefill::clamp_scheduler(
            Some(SchedulerLimits {
                max_sequences: *max_num_seqs,
                batched_tokens: *max_num_batched_tokens,
                prefill_chunk_tokens: *max_prefill_chunk_tokens,
            }),
            usize::from(profile.max_sequences),
        )?;
        *max_num_batched_tokens = limits.batched_tokens;
        *max_prefill_chunk_tokens = limits.prefill_chunk_tokens;
        eprintln!(
            "metron experimental scheduler={} kv_target_bytes={} max_sequences={} batched_tokens={} prefill_chunk_tokens={}",
            chunk_prefill::CHUNK_PREFILL_CONTRACT,
            bytes,
            limits.max_sequences,
            limits.batched_tokens,
            limits.prefill_chunk_tokens
        );
        effective_scheduler = Some(limits);
    }
    let retained_pipeline = Arc::clone(&pipeline);
    let model = mistralrs::model_builder_trait::build_model_from_pipeline(
        pipeline,
        scheduler,
        add_model_config,
    )
    .await;
    drop(tokenizer_guard);
    let actual_device = model
        .config()
        .map_err(|error| format!("backend device unavailable: {error}"))?
        .device;
    if matches!(profile.device, LocalDevice::Cuda { .. }) && !actual_device.is_cuda() {
        return Err("CUDA requested but loaded backend is not CUDA".into());
    }
    eprintln!("metron inference backend device: {actual_device:?}");
    let loaded = LoadedModel {
        model,
        pipeline: retained_pipeline,
        device: actual_device,
        scheduler: effective_scheduler,
    };
    resources::emit(
        profile,
        &loaded.device,
        Some(&*loaded.pipeline.lock().await),
        loaded.scheduler,
        "loaded_idle",
    );
    Ok(loaded)
}
