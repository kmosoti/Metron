//! Out-of-band resource observations; these values are never profile or journal identity.
use serde_json::{Value, json};

pub const MAX_RECORD_BYTES: usize = 16 * 1024;
pub const MAX_LAYERS: usize = 24;
pub const TRAINED_CONTEXT_TOKENS: u64 = 262_144;
pub const ATTENTION_LAYERS: u64 = 11;
pub const FFN_PACKED_LOWER_BOUND_BYTES: u64 = 685_670_400;

/// A numeric observation with an explicit unit and source. Missing is never zero.
pub fn quantity(value: u64, unit: &str, basis: &str) -> Value {
    json!({"value": value, "unit": unit, "basis": basis, "unavailable_reason": null})
}

pub fn unavailable(unit: &str, reason: &str) -> Value {
    json!({"value": null, "unit": unit, "basis": null, "unavailable_reason": reason})
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagedGeometry {
    pub blocks: u64,
    pub allocated_tokens_per_layer: u64,
    pub usable_tokens: u64,
    pub tensor_payload_bytes: u64,
}

/// Pinned LFM3: all eleven attention layers own full-capacity K/V pools;
/// five sliding-window layers do not shrink the paged allocation. Block zero is reserved.
pub fn lfm3_paged_geometry(
    requested_tokens: u64,
    block_size: u64,
    scalar_bytes: u64,
) -> Result<PagedGeometry, &'static str> {
    if requested_tokens == 0 || block_size == 0 || !matches!(scalar_bytes, 2 | 4) {
        return Err("invalid LFM3 geometry");
    }
    let blocks = requested_tokens
        .div_ceil(block_size)
        .checked_add(1)
        .ok_or("page count overflow")?;
    let allocated_tokens_per_layer = blocks.checked_mul(block_size).ok_or("page size overflow")?;
    let tensor_payload_bytes = [
        allocated_tokens_per_layer,
        ATTENTION_LAYERS,
        2,
        4,
        128,
        scalar_bytes,
    ]
    .into_iter()
    .try_fold(1_u64, |total, next| total.checked_mul(next))
    .ok_or("KV payload overflow")?;
    Ok(PagedGeometry {
        blocks,
        allocated_tokens_per_layer,
        usable_tokens: (blocks - 1)
            .checked_mul(block_size)
            .ok_or("capacity overflow")?,
        tensor_payload_bytes,
    })
}

pub fn lfm3_unpaged_retained_estimate(tokens: u64, scalar_bytes: u64) -> Result<u64, &'static str> {
    if !matches!(scalar_bytes, 2 | 4) {
        return Err("unqualified scalar width");
    }
    tokens
        .checked_mul(6)
        .and_then(|global| tokens.min(256).checked_mul(5)?.checked_add(global))
        .and_then(|retained| retained.checked_mul(2 * 4 * 128)?.checked_mul(scalar_bytes))
        .ok_or("retained KV overflow")
}

/// Thirteen recurrent layers, nine initial pool slots, 2048x7 convolution
/// state plus the SDK's one-element recurrent placeholder per slot.
pub fn lfm3_recurrent_initial_estimate(scalar_bytes: u64) -> Result<u64, &'static str> {
    if !matches!(scalar_bytes, 2 | 4) {
        return Err("unqualified scalar width");
    }
    (13_u64 * 9 * (2048 * 7 + 1))
        .checked_mul(scalar_bytes)
        .ok_or("recurrent payload overflow")
}

/// This can reject a source-proven lower bound, never establish physical fit.
pub fn lfm3_fit_lower_bound(
    total_device_bytes: u64,
    tokens: u64,
    scalar_bytes: u64,
) -> Result<Value, &'static str> {
    let kv = lfm3_paged_geometry(tokens, 32, scalar_bytes)?;
    let minimum = kv
        .tensor_payload_bytes
        .checked_add(FFN_PACKED_LOWER_BOUND_BYTES)
        .and_then(|bytes| bytes.checked_add(lfm3_recurrent_initial_estimate(scalar_bytes).ok()?))
        .ok_or("fit lower bound overflow")?;
    Ok(json!({
        "status": if minimum > total_device_bytes { "rejected" } else { "not_evaluated" },
        "basis": "source-derived all-11-layer paged KV plus packed FFN and recurrent lower bounds; remaining weights/runtime/workspace excluded",
        "minimum_bytes": quantity(minimum, "bytes", "pinned LFM3 source/inventory lower bound"),
        "strict_allocation_cap": "unsupported"
    }))
}

/// Concrete reporting groups, deliberately separate from aggregate phase telemetry.
pub struct ResourceRecord {
    pub phase: &'static str,
    pub identity: Value,
    pub weights: Value,
    pub load_baseline: Value,
    pub kv: Value,
    pub runtime: Value,
    pub aggregate: Value,
    pub limits: Value,
}

impl ResourceRecord {
    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        if !matches!(
            self.phase,
            "before_load" | "loaded_idle" | "diagnostic_prefill" | "diagnostic_end"
        ) {
            return Err("unsupported resource observation phase");
        }
        let record = json!({
            "kind": "metron_resource_observation", "schema_version": 1,
            "phase": self.phase, "identity": self.identity,
            "weights": self.weights, "load_baseline": self.load_baseline,
            "kv": self.kv, "runtime": self.runtime, "aggregate": self.aggregate,
            "limits": self.limits
        });
        fn bounded(value: &Value) -> bool {
            match value {
                Value::String(text) => text.len() <= 512,
                Value::Array(values) => values.len() <= MAX_LAYERS && values.iter().all(bounded),
                Value::Object(values) => values.values().all(bounded),
                _ => true,
            }
        }
        if !bounded(&record) {
            return Err("resource observation label or layer limit exceeded");
        }
        let bytes =
            serde_json::to_vec(&record).map_err(|_| "resource observation serialization failed")?;
        if bytes.len() + 1 > MAX_RECORD_BYTES {
            return Err("resource observation exceeds 16 KiB");
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eleven_layer_pages_reserve_block_zero_and_do_not_discount_sliding_layers() {
        let g = lfm3_paged_geometry(TRAINED_CONTEXT_TOKENS, 32, 2).unwrap();
        assert_eq!(
            g,
            PagedGeometry {
                blocks: 8193,
                allocated_tokens_per_layer: 262176,
                usable_tokens: 262144,
                tensor_payload_bytes: 5_906_300_928
            }
        );
        assert_eq!(lfm3_paged_geometry(513, 32, 2).unwrap().usable_tokens, 544);
        assert_eq!(
            lfm3_paged_geometry(TRAINED_CONTEXT_TOKENS, 32, 4)
                .unwrap()
                .tensor_payload_bytes,
            2 * g.tensor_payload_bytes
        );
        assert_eq!(lfm3_recurrent_initial_estimate(2).unwrap(), 3_354_858);
        assert_eq!(lfm3_recurrent_initial_estimate(4).unwrap(), 6_709_716);
        assert_eq!(
            lfm3_unpaged_retained_estimate(TRAINED_CONTEXT_TOKENS, 2).unwrap(),
            3_223_846_912
        );
        assert_eq!(
            lfm3_fit_lower_bound(6 * 1024_u64.pow(3), TRAINED_CONTEXT_TOKENS, 2).unwrap()["status"],
            "rejected"
        );
        assert_eq!(
            lfm3_fit_lower_bound(6 * 1024_u64.pow(3), TRAINED_CONTEXT_TOKENS, 4).unwrap()["status"],
            "rejected"
        );
        assert_eq!(
            lfm3_fit_lower_bound(6 * 1024_u64.pow(3), 513, 2).unwrap()["status"],
            "not_evaluated"
        );
        assert!(lfm3_paged_geometry(u64::MAX, 32, 2).is_err());
        assert!(lfm3_paged_geometry(1, 0, 2).is_err());
        assert!(lfm3_paged_geometry(1, 32, 1).is_err());
    }
    fn record() -> ResourceRecord {
        ResourceRecord {
            phase: "loaded_idle",
            identity: json!({}),
            weights: json!({}),
            load_baseline: json!({"scope":"model plus runtime"}),
            kv: json!({}),
            runtime: json!({}),
            aggregate: json!({}),
            limits: json!({"kv_target_status":"planning_target", "strict_allocation_cap":"unsupported"}),
        }
    }
    #[test]
    fn unknown_is_distinct_from_observed_zero_and_reports_are_bounded() {
        assert_ne!(
            unavailable("bytes", "not attributable"),
            quantity(0, "bytes", "observed")
        );
        let mut r = record();
        let bytes = r.encode().unwrap();
        let decoded: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded["limits"]["kv_target_status"], "planning_target");
        assert_eq!(decoded["limits"]["strict_allocation_cap"], "unsupported");
        assert_eq!(decoded["load_baseline"]["scope"], "model plus runtime");
        r.kv = json!({"layers": vec![json!({});25]});
        assert!(r.encode().is_err());
        r.kv = json!({"label": "x".repeat(513)});
        assert!(r.encode().is_err());
        r.kv = Value::Object(
            (0..100)
                .map(|i| (format!("k{i}"), json!("x".repeat(500))))
                .collect(),
        );
        assert!(r.encode().is_err());
        r.phase = "arbitrary";
        assert!(r.encode().is_err());
    }
}
