//! A scripted service for tests.

use crate::service::ServiceBackend;
use metron_core::cost::Cost;
use metron_core::id::{InquiryId, OperatorId};
use metron_core::inquiry::Representation;
use metron_core::receipt::{OperationKind, ResourceReceipt};
use metron_core::world::{IdSource, ServiceAnswer, ServiceError, ServiceResponse};

/// Answers every request with the output of a closure.
pub struct StubService {
    service: String,
    answer: Box<dyn FnMut(&Representation) -> Representation + Send>,
    receipts: Vec<ResourceReceipt>,
    answers: Vec<ServiceAnswer>,
}

impl std::fmt::Debug for StubService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StubService({})", self.service)
    }
}

impl StubService {
    /// A stub for `service` answering with `answer`.
    pub fn new(
        service: impl Into<String>,
        answer: impl FnMut(&Representation) -> Representation + Send + 'static,
    ) -> Self {
        Self {
            service: service.into(),
            answer: Box::new(answer),
            receipts: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// A stub that always answers the same text.
    pub fn constant(service: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(service, move |_| Representation::Text(text.clone()))
    }
}

impl ServiceBackend for StubService {
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
        let content = (self.answer)(request);
        let receipt = ResourceReceipt::seal(
            ids.next_receipt(),
            OperationKind::ExternalCall {
                service: self.service.clone(),
            },
            operator.clone(),
            inquiry,
            request.content_hash(),
            content.content_hash(),
            Cost::external(1),
            Vec::new(),
            0,
        );
        self.answers.push(ServiceAnswer {
            receipt: receipt.id,
            service: self.service.clone(),
            request: request.clone(),
            response: content.clone(),
            answered_by: "stub".into(),
        });
        self.receipts.push(receipt);
        Ok(ServiceResponse {
            content,
            answered_by: "stub".into(),
        })
    }

    fn take_receipts(&mut self) -> Vec<ResourceReceipt> {
        std::mem::take(&mut self.receipts)
    }

    fn take_answers(&mut self) -> Vec<ServiceAnswer> {
        std::mem::take(&mut self.answers)
    }
}
