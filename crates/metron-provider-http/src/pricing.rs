//! Deterministic integer token-cost pricing table (`spend_micros_usd`).

/// Per-million-token pricing rate in micro-USD (1 USD = 1_000_000 micros).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPricing {
    /// Micro-USD per 1,000,000 input tokens (e.g. `$2.50 / 1M` = `2_500_000`).
    pub input_micros_per_1m: u64,
    /// Micro-USD per 1,000,000 output tokens (e.g. `$10.00 / 1M` = `10_000_000`).
    pub output_micros_per_1m: u64,
}

impl ModelPricing {
    /// Computes deterministic ceiling micro-USD spend for `(input_tokens, output_tokens)`.
    #[must_use]
    pub fn compute_micros(&self, input_tokens: u64, output_tokens: u64) -> u64 {
        let in_cost = u128::from(input_tokens)
            .saturating_mul(u128::from(self.input_micros_per_1m))
            .div_ceil(1_000_000);
        let out_cost = u128::from(output_tokens)
            .saturating_mul(u128::from(self.output_micros_per_1m))
            .div_ceil(1_000_000);
        u64::try_from(in_cost.saturating_add(out_cost)).unwrap_or(u64::MAX)
    }
}

/// Looks up a built-in model rate table entry by model prefix/family.
///
/// Returns a conservative default rate for unrecognized compatible models so
/// `spend_micros_usd` is always accounted for when tokens are non-zero.
#[must_use]
pub fn lookup_model_pricing(model: &str) -> ModelPricing {
    let m = model.to_ascii_lowercase();
    if m.starts_with("gpt-4o-mini")
        || m.starts_with("gpt-4.1-mini")
        || m.starts_with("gpt-4.1-nano")
    {
        ModelPricing {
            input_micros_per_1m: 150_000,
            output_micros_per_1m: 600_000,
        }
    } else if m.starts_with("gpt-4o") || m.starts_with("gpt-4.1") {
        ModelPricing {
            input_micros_per_1m: 2_500_000,
            output_micros_per_1m: 10_000_000,
        }
    } else if m.starts_with("o3-mini") || m.starts_with("o4-mini") {
        ModelPricing {
            input_micros_per_1m: 1_100_000,
            output_micros_per_1m: 4_400_000,
        }
    } else if m.starts_with("o3") || m.starts_with("o1") {
        ModelPricing {
            input_micros_per_1m: 10_000_000,
            output_micros_per_1m: 40_000_000,
        }
    } else if m.contains("haiku") {
        ModelPricing {
            input_micros_per_1m: 800_000,
            output_micros_per_1m: 4_000_000,
        }
    } else if m.contains("sonnet") {
        ModelPricing {
            input_micros_per_1m: 3_000_000,
            output_micros_per_1m: 15_000_000,
        }
    } else if m.contains("opus") {
        ModelPricing {
            input_micros_per_1m: 15_000_000,
            output_micros_per_1m: 75_000_000,
        }
    } else {
        // Conservative baseline for compatible / self-hosted endpoints.
        ModelPricing {
            input_micros_per_1m: 1_000_000,
            output_micros_per_1m: 3_000_000,
        }
    }
}

/// Computes deterministic micro-USD spend using optional explicit override or built-in table.
#[must_use]
pub fn estimate_spend_micros_usd(
    model: &str,
    override_pricing: Option<ModelPricing>,
    input_tokens: u64,
    output_tokens: u64,
) -> u64 {
    if input_tokens == 0 && output_tokens == 0 {
        return 0;
    }
    let pricing = override_pricing.unwrap_or_else(|| lookup_model_pricing(model));
    pricing.compute_micros(input_tokens, output_tokens)
}
