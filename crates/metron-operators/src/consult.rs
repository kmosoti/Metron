//! Consultation: ask a language model for a formula, then verify it.
//!
//! The model is a hypothesis generator, never a decider. Its proposal lands
//! in the consultations frame; [`FormulaToTable`] parses it and refuses it
//! unless it agrees with every observation. Only then does it reach the
//! truth-table frame, under an approximate contract that says the rows
//! nobody observed are still a proposal.

use crate::formula::Formula;
use crate::support::{arity, observed_rows, rows};
use crate::{frames, views};
use metron_core::cost::Cost;
use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::inquiry::{Derivation, Inquiry, Representation};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;
use metron_core::world::{ExternalService, ServiceError};

/// The service name consultation operators use.
pub const LLM_SERVICE: &str = "llm";

/// Asks the `llm` service for a formula consistent with the observations.
#[derive(Debug, Default, Clone, Copy)]
pub struct LlmProposeFormula;

impl LlmProposeFormula {
    /// The operator identifier.
    pub const ID: &'static str = "llm-propose-formula";

    /// The prompt for the current observations.
    #[must_use]
    pub fn prompt(arity: usize, observed: &[(usize, bool)]) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "A hidden Boolean function f takes {arity} inputs x0..x{}. Below are observed input/output pairs.",
            arity.saturating_sub(1)
        ));
        lines.push("Each line lists the inputs as x0 x1 ... and then the output.".into());
        for &(row, out) in observed {
            let bits: Vec<String> = (0..arity).map(|j| ((row >> j) & 1).to_string()).collect();
            lines.push(format!("  {} -> {}", bits.join(" "), u8::from(out)));
        }
        if observed.is_empty() {
            lines.push("  (no observations yet)".into());
        }
        lines.push(String::new());
        lines.push(
            "Propose the simplest formula consistent with ALL observations, using variables x0..x{n}, \
             operators & (and), | (or), ^ (xor), ~ (not), parentheses, and constants 0 and 1."
                .replace("{n}", &arity.saturating_sub(1).to_string()),
        );
        lines.push("Reply with exactly one formula on a single line and nothing else.".into());
        lines.join("\n")
    }
}

impl<W: ExternalService> Operator<W> for LlmProposeFormula {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Consult {
                service: LLM_SERVICE.into(),
            },
            "Ask the language-model service for a formula consistent with the observations.",
        )
        .writes(views::CONSULTATION_LLM)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let observed = observed_rows(inquiry, arity);
        let prompt = Self::prompt(arity, &observed);
        let request = Representation::Json(serde_json::json!({
            "prompt": prompt,
            "arity": arity,
            "observations": observed.iter().map(|&(r, o)| serde_json::json!({"row": r, "output": u8::from(o)})).collect::<Vec<_>>(),
        }));
        let response = match world.consult(
            &OperatorId::from(Self::ID),
            inquiry.id,
            LLM_SERVICE,
            &request,
        ) {
            Ok(r) => r,
            Err(ServiceError::Pending { request_id }) => {
                return Err(OperatorError::Pending {
                    service: LLM_SERVICE.into(),
                    request_id,
                });
            }
            Err(other) => return Err(other.into()),
        };
        let text = match &response.content {
            Representation::Text(t) => t.clone(),
            Representation::Json(v) => v
                .get("formula")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| v.to_string(), str::to_owned),
            Representation::Bits(b) => b.to_hex(),
        };
        let ids = inquiry.observation_ids();
        inquiry.write_view(
            views::CONSULTATION_LLM,
            frames::CONSULTATIONS,
            Representation::Json(serde_json::json!({
                "prompt": prompt,
                "response": text,
                "answered_by": response.answered_by,
                "observations_in_prompt": observed.len(),
            })),
            Derivation::by(OperatorId::from(Self::ID), inquiry.steps).with_observations(ids),
        );
        Ok(
            OperatorOutcome::progressed(Cost::work(1)).with_note(format!(
                "proposal from {}: {}",
                response.answered_by,
                text.lines().last().unwrap_or("").trim()
            )),
        )
    }
}

/// Parses the consulted formula and, if it agrees with every observation,
/// writes its complete table.
#[derive(Debug, Default, Clone, Copy)]
pub struct FormulaToTable;

impl FormulaToTable {
    /// The operator identifier.
    pub const ID: &'static str = "formula-to-table";

    /// The contract this transform runs under.
    #[must_use]
    pub fn contract() -> TransformContract {
        TransformContract::new(
            [frames::CONSULTATIONS, frames::OBSERVATIONS],
            frames::TRUTH_TABLE_COMPLETE,
        )
        .preserves("every observed row, which the proposal is checked against")
        .assumes("the proposed formula is correct on the rows nobody observed")
        .approximate("unobserved rows come from an unverified proposal")
    }

    /// Extracts the formula text from a response: the last non-empty line,
    /// stripped of code fences and surrounding punctuation.
    #[must_use]
    pub fn extract(response: &str) -> String {
        response
            .lines()
            .map(|l| l.trim().trim_matches('`').trim())
            .rfind(|l| !l.is_empty() && !l.starts_with("```"))
            .unwrap_or("")
            .trim_start_matches("f =")
            .trim_start_matches("f=")
            .trim()
            .to_owned()
    }
}

impl<W> Operator<W> for FormulaToTable {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Transform {
                contract: Self::contract(),
            },
            "Parse the proposed formula and, if every observation agrees with it, write its table.",
        )
        .reads(views::CONSULTATION_LLM)
        .writes(views::COMPLETE_TABLE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let total = rows(arity);
        let response = inquiry
            .view_str(views::CONSULTATION_LLM)
            .and_then(|v| v.content.as_json())
            .and_then(|v| v.get("response"))
            .and_then(serde_json::Value::as_str)
            .map(Self::extract)
            .ok_or(OperatorError::NotApplicable)?;
        let formula = match Formula::parse(&response) {
            Ok(f) => f,
            Err(e) => {
                return Ok(OperatorOutcome::no_change(Cost::work(1))
                    .with_note(format!("proposal `{response}` rejected: {e}")));
            }
        };
        let table = match formula.to_table(arity) {
            Ok(t) => t,
            Err(e) => {
                return Ok(OperatorOutcome::no_change(Cost::work(1))
                    .with_note(format!("proposal `{response}` rejected: {e}")));
            }
        };
        let observed = observed_rows(inquiry, arity);
        if let Some(&(row, out)) = observed.iter().find(|&&(row, out)| table.bit(row) != out) {
            return Ok(
                OperatorOutcome::no_change(Cost::work(observed.len() as u64)).with_note(format!(
                    "proposal `{formula}` refuted by row {row} (observed {})",
                    u8::from(out)
                )),
            );
        }
        let mut derivation = Derivation::by(OperatorId::from(Self::ID), inquiry.steps)
            .from_view(views::CONSULTATION_LLM);
        if inquiry.view_str(views::OBSERVATIONS).is_some() {
            derivation = derivation.from_view(views::OBSERVATIONS);
        }
        inquiry.write_view(
            views::COMPLETE_TABLE,
            frames::TRUTH_TABLE_COMPLETE,
            Representation::Bits(table),
            derivation,
        );
        Ok(
            OperatorOutcome::progressed(Cost::work(total as u64 + observed.len() as u64))
                .with_note(format!(
                    "proposal `{formula}` agrees with all {} observations",
                    observed.len()
                )),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_tolerates_chatty_answers() {
        assert_eq!(
            FormulaToTable::extract("Sure!\n```\n(x0 & x1) | x2\n```\n"),
            "(x0 & x1) | x2"
        );
        assert_eq!(FormulaToTable::extract("f = x0 ^ x1"), "x0 ^ x1");
        assert_eq!(FormulaToTable::extract(""), "");
    }

    #[test]
    fn prompt_lists_observations() {
        let p = LlmProposeFormula::prompt(3, &[(5, true), (0, false)]);
        assert!(p.contains("1 0 1 -> 1"));
        assert!(p.contains("0 0 0 -> 0"));
        assert!(p.contains("x0..x2"));
    }
}
