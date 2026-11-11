//! The fixed scripted fixture, model and publication effect drivers.
use crate::{
    policy::{AuthorizedEffect, EffectDriver},
    publication,
};
use metron_protocol::{
    domain::{
        ActualUsage, ApprovedRequest, CandidateResult, Receipt, ReceiptData, ReceiptOutput,
        UsageVector, parse_sum_input,
    },
    types::{BoundedText, DecimalUsage, ProducerReceiptId, content_digest},
};
use std::path::PathBuf;

const FIXTURE: &str = "{\"values\":[17,-4,9]}\n";

/// Observer hook runs after the file and directory are synced, before receipt creation.
pub struct ScriptedDriver<'a> {
    pub run_dir: PathBuf,
    pub publication_synced: &'a mut dyn FnMut(),
}

/// Failure is uncertainty: an effect may have occurred before the error.
pub enum DriverOutcome {
    Receipt(Box<Receipt>),
    Uncertain,
}
impl EffectDriver for ScriptedDriver<'_> {
    type Outcome = DriverOutcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> Self::Outcome {
        fn produce(
            driver: &mut ScriptedDriver<'_>,
            effect: &AuthorizedEffect,
        ) -> Result<Receipt, ()> {
            let prepared = effect.request();
            let (output, model, tool) = match &prepared.request {
                ApprovedRequest::ReadFixture { fixture_key, .. }
                    if fixture_key.as_str() == "fixture://sum-v1/input" =>
                {
                    (
                        ReceiptOutput::Fixture {
                            bytes: BoundedText::new(FIXTURE).map_err(|_| ())?,
                            digest: content_digest(FIXTURE.as_bytes()),
                        },
                        "0",
                        "1",
                    )
                }
                ApprovedRequest::RequestModel {
                    input_digest,
                    route,
                    context,
                    ..
                } if route.as_str() == "scripted/sum-v1"
                    && content_digest(context.as_str().as_bytes()) == *input_digest =>
                {
                    let input = parse_sum_input(context.as_str().as_bytes()).map_err(|_| ())?;
                    let sum = input
                        .values
                        .iter()
                        .try_fold(0_i64, |acc, n| acc.checked_add(*n))
                        .ok_or(())?;
                    (
                        ReceiptOutput::Model {
                            candidate: CandidateResult {
                                count: u32::try_from(input.values.len()).map_err(|_| ())?,
                                sum,
                                input_digest: input_digest.clone(),
                            },
                        },
                        "1",
                        "0",
                    )
                }
                ApprovedRequest::PublishArtifact { plan } => {
                    publication::publish(&driver.run_dir, plan, driver.publication_synced)
                        .map_err(|_| ())?;
                    (
                        ReceiptOutput::Published {
                            artifact_id: plan.artifact_id.clone(),
                            content_digest: plan.content_digest.clone(),
                            content_length: plan.content_length,
                        },
                        "0",
                        "1",
                    )
                }
                _ => return Err(()),
            };
            let usage = ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new(model).map_err(|_| ())?,
                    tool_calls: DecimalUsage::new(tool).map_err(|_| ())?,
                    spend_micros_usd: DecimalUsage::new("0").map_err(|_| ())?,
                },
            };
            Receipt::new(ReceiptData {
                schema_version: effect.schema_version(),
                producer_receipt_id: ProducerReceiptId::new(prepared.action.dispatch_id.as_str())
                    .map_err(|_| ())?,
                task_id: prepared.action.task_id.clone(),
                intent_id: prepared.intent_id.clone(),
                dispatch_id: prepared.action.dispatch_id.clone(),
                action_digest: prepared.action_digest.clone(),
                usage,
                output,
            })
            .map_err(|_| ())
        }
        match produce(self, &effect) {
            Ok(receipt) => DriverOutcome::Receipt(Box::new(receipt)),
            Err(()) => DriverOutcome::Uncertain,
        }
    }
}
