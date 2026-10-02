//! Commit: the complete truth table becomes the answer.

use crate::support::{arity, provenance_path, rows};
use crate::views;
use metron_core::cost::Cost;
use metron_core::evidence::Evidence;
use metron_core::id::{OperatorId, ViewId};
use metron_core::inquiry::{Answer, Inquiry};
use metron_core::operator::{Operator, OperatorError, OperatorKind, OperatorOutcome, OperatorSpec};
use metron_core::rng::Rng;

/// Commits the complete table as the answer, in the frame the question asks
/// for, with evidence tracing back to every observation the table rests on.
/// Confidence is the fraction of rows that were actually observed.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommitTruthTable;

impl CommitTruthTable {
    /// The operator identifier.
    pub const ID: &'static str = "commit-truth-table";
}

impl<W> Operator<W> for CommitTruthTable {
    fn spec(&self) -> OperatorSpec {
        OperatorSpec::new(
            Self::ID,
            OperatorKind::Commit,
            "Commit the complete truth table as the answer.",
        )
        .reads(views::COMPLETE_TABLE)
    }

    fn apply(
        &self,
        inquiry: &mut Inquiry,
        _world: &mut W,
        _rng: &mut Rng,
    ) -> Result<OperatorOutcome, OperatorError> {
        let arity = arity(inquiry)?;
        let total = rows(arity);
        let view_id = ViewId::from(views::COMPLETE_TABLE);
        let view = inquiry.view(&view_id).ok_or(OperatorError::NotApplicable)?;
        if view.frame != inquiry.question.answer_frame {
            return Ok(OperatorOutcome::failed(
                Cost::work(1),
                format!(
                    "complete table is in frame `{}` but the question wants `{}`",
                    view.frame, inquiry.question.answer_frame
                ),
            ));
        }
        let content = view.content.clone();
        let frame = view.frame.clone();
        let observations = inquiry.observation_closure(&view_id);
        let mut via = provenance_path(inquiry, &view_id);
        via.push(OperatorId::from(Self::ID));
        let evidence: Vec<Evidence> = observations
            .iter()
            .map(|&o| Evidence::derived(o, via.clone()))
            .collect();
        let confidence = (observations.len().min(total)) as f64 / total as f64;
        inquiry.commit(Answer {
            frame,
            content,
            confidence,
            evidence,
            by: OperatorId::from(Self::ID),
        });
        Ok(
            OperatorOutcome::answered(Cost::work(observations.len() as u64)).with_note(format!(
                "committed with {} independent observations, confidence {confidence:.3}",
                observations.len()
            )),
        )
    }
}
