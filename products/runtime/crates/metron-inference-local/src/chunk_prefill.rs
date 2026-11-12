//! Bounded scheduler settings for the unqualified chunked-prefill experiment.
use metron_protocol::{LocalDevice, LocalProfile};

/// Part of the worker-build-bound experimental contract, not runtime chunk counts.
pub const CHUNK_PREFILL_CONTRACT: &str = "paged-chunk-prefill-v1";
pub const MAX_PREFILL_CHUNK_TOKENS: usize = 256;
pub const MAX_BATCHED_TOKENS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulerLimits {
    pub max_sequences: usize,
    pub batched_tokens: usize,
    pub prefill_chunk_tokens: usize,
}

pub fn kv_target(profile: &LocalProfile) -> Result<Option<usize>, &'static str> {
    if profile.scoped_prefix_cache
        || (!profile.chunked_prefill && profile.cache_target_bytes.is_some())
    {
        return Err("unsupported profile setting");
    }
    if !profile.chunked_prefill {
        return Ok(None);
    }
    profile
        .validate()
        .map_err(|_| "invalid chunked prefill profile")?;
    if !matches!(profile.device, LocalDevice::Cuda { .. }) {
        return Err("chunked prefill requires CUDA");
    }
    let bytes = profile
        .cache_target_bytes
        .ok_or("missing KV planning target")?
        .get();
    usize::try_from(bytes)
        .map(Some)
        .map_err(|_| "KV planning target overflow")
}

/// Retain the realized sequence count and never increase either SDK token limit.
pub fn clamp_scheduler(
    realized: Option<SchedulerLimits>,
    expected_sequences: usize,
) -> Result<SchedulerLimits, &'static str> {
    let current = realized.ok_or("paged scheduler unavailable")?;
    if current.max_sequences != expected_sequences
        || expected_sequences == 0
        || current.batched_tokens == 0
        || current.prefill_chunk_tokens == 0
    {
        return Err("invalid realized scheduler limits");
    }
    let batched_tokens = current.batched_tokens.min(MAX_BATCHED_TOKENS);
    let prefill_chunk_tokens = current
        .prefill_chunk_tokens
        .min(MAX_PREFILL_CHUNK_TOKENS)
        .min(batched_tokens);
    if batched_tokens < expected_sequences {
        return Err("scheduler token limit below sequence count");
    }
    Ok(SchedulerLimits {
        max_sequences: current.max_sequences,
        batched_tokens,
        prefill_chunk_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_realized_paged_limits_and_only_reduces() {
        assert!(clamp_scheduler(None, 2).is_err());
        assert!(
            clamp_scheduler(
                Some(SchedulerLimits {
                    max_sequences: 1,
                    batched_tokens: 1024,
                    prefill_chunk_tokens: 512
                }),
                2
            )
            .is_err()
        );
        assert!(
            clamp_scheduler(
                Some(SchedulerLimits {
                    max_sequences: 2,
                    batched_tokens: 0,
                    prefill_chunk_tokens: 2
                }),
                2
            )
            .is_err()
        );
        assert_eq!(
            clamp_scheduler(
                Some(SchedulerLimits {
                    max_sequences: 2,
                    batched_tokens: 1024,
                    prefill_chunk_tokens: 512
                }),
                2
            )
            .unwrap(),
            SchedulerLimits {
                max_sequences: 2,
                batched_tokens: 512,
                prefill_chunk_tokens: 256
            }
        );
        assert_eq!(
            clamp_scheduler(
                Some(SchedulerLimits {
                    max_sequences: 2,
                    batched_tokens: 80,
                    prefill_chunk_tokens: 100
                }),
                2
            )
            .unwrap()
            .prefill_chunk_tokens,
            80
        );
    }
}
