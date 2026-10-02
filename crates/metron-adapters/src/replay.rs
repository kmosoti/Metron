//! Replays recorded service answers.

use crate::service::ServiceBackend;
use metron_core::cost::Cost;
use metron_core::hash::ContentHash;
use metron_core::id::{InquiryId, OperatorId};
use metron_core::inquiry::Representation;
use metron_core::journal::Episode;
use metron_core::receipt::{OperationKind, ResourceReceipt};
use metron_core::world::{IdSource, ServiceAnswer, ServiceError, ServiceResponse};
use std::collections::BTreeMap;

/// Answers consultations from a journal's recorded answers, keyed by
/// request content.
#[derive(Debug, Default)]
pub struct ReplayService {
    service: String,
    answers_by_request: BTreeMap<ContentHash, ServiceAnswer>,
    receipts: Vec<ResourceReceipt>,
    answers: Vec<ServiceAnswer>,
}

impl ReplayService {
    /// Builds a replay from the answers of `episode` for `service`.
    #[must_use]
    pub fn from_episode(episode: &Episode, service: &str) -> Self {
        let answers_by_request = episode
            .service_answers()
            .filter(|a| a.service == service)
            .map(|a| (a.request.content_hash(), a.clone()))
            .collect();
        Self {
            service: service.to_owned(),
            answers_by_request,
            receipts: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// Number of recorded answers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.answers_by_request.len()
    }

    /// Whether nothing is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.answers_by_request.is_empty()
    }
}

impl ServiceBackend for ReplayService {
    fn service(&self) -> &str {
        &self.service
    }

    fn consult_using(
        &mut self,
        ids: &mut dyn IdSource,
        operator: &OperatorId,
        inquiry: InquiryId,
        request: &Representation,
    ) -> Result<ServiceResponse, ServiceError> {
        let recorded = self
            .answers_by_request
            .get(&request.content_hash())
            .cloned()
            .ok_or_else(|| {
                ServiceError::Unavailable("no recorded answer for this request".into())
            })?;
        let receipt = ResourceReceipt::seal(
            ids.next_receipt(),
            OperationKind::ExternalCall {
                service: self.service.clone(),
            },
            operator.clone(),
            inquiry,
            request.content_hash(),
            recorded.response.content_hash(),
            Cost::external(1),
            Vec::new(),
            0,
        );
        self.answers.push(ServiceAnswer {
            receipt: receipt.id,
            service: self.service.clone(),
            request: request.clone(),
            response: recorded.response.clone(),
            answered_by: recorded.answered_by.clone(),
        });
        self.receipts.push(receipt);
        Ok(ServiceResponse {
            content: recorded.response,
            answered_by: recorded.answered_by,
        })
    }

    fn take_receipts(&mut self) -> Vec<ResourceReceipt> {
        std::mem::take(&mut self.receipts)
    }

    fn take_answers(&mut self) -> Vec<ServiceAnswer> {
        std::mem::take(&mut self.answers)
    }
}
