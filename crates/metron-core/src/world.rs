//! The ports through which the system touches the world.
//!
//! The system never holds a hidden target or a model. It holds a `W` that
//! implements some of these ports:
//!
//! * [`Receipts`] — every world hands the runner the receipts it issued;
//! * [`Oracle`] — answers probes about the hidden target;
//! * [`Knowledge`] — serves public documents the question refers to by
//!   hash (a hypothesis pool, a grammar). Reading them is not an external
//!   operation and issues no receipt;
//! * [`ExternalService`] — consults an outside service such as a language
//!   model session. A call may be [`ServiceError::Pending`], in which case
//!   the episode suspends until an answer arrives.
//!
//! Whoever implements the ports (the laboratory, a bridge to a Claude Code
//! session, a replay of a journal) decides the protocol and keeps whatever
//! must stay hidden to itself.

use crate::evidence::Observation;
use crate::id::{InquiryId, ObservationId, OperatorId, ReceiptId};
use crate::inquiry::Representation;
use crate::receipt::ResourceReceipt;
use serde::{Deserialize, Serialize};

/// Hands out identifiers for receipts and observations.
///
/// A composed world holds exactly one source so identifiers are unique
/// across its ports.
pub trait IdSource {
    /// The next receipt identifier.
    fn next_receipt(&mut self) -> ReceiptId;
    /// The next observation identifier.
    fn next_observation(&mut self) -> ObservationId;
}

/// A plain counter-based [`IdSource`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counter {
    /// Next receipt identifier to hand out.
    pub next_receipt: ReceiptId,
    /// Next observation identifier to hand out.
    pub next_observation: ObservationId,
}

impl Default for Counter {
    fn default() -> Self {
        Self {
            next_receipt: ReceiptId(1),
            next_observation: ObservationId(1),
        }
    }
}

impl IdSource for Counter {
    fn next_receipt(&mut self) -> ReceiptId {
        let id = self.next_receipt;
        self.next_receipt = id.next();
        id
    }

    fn next_observation(&mut self) -> ObservationId {
        let id = self.next_observation;
        self.next_observation = id.next();
        id
    }
}

/// A recorded answer from an external service, kept in full so that an
/// episode can be replayed without the service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceAnswer {
    /// The receipt for the call.
    pub receipt: ReceiptId,
    /// Service name.
    pub service: String,
    /// What was asked.
    pub request: Representation,
    /// What came back.
    pub response: Representation,
    /// Who or what answered (for example a Claude Code session, or a
    /// replay of an earlier journal).
    pub answered_by: String,
}

/// Something that issues receipts.
pub trait Receipts {
    /// Receipts issued since the last drain, in issue order.
    fn drain_receipts(&mut self) -> Vec<ResourceReceipt>;

    /// Service answers recorded since the last drain, in order. Worlds
    /// without external services return nothing.
    fn drain_service_answers(&mut self) -> Vec<ServiceAnswer> {
        Vec::new()
    }
}

/// Errors an oracle can return.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OracleError {
    /// The protocol's probe cap is spent.
    #[error("probe budget exhausted")]
    BudgetExhausted,
    /// The probe did not follow the protocol.
    #[error("malformed probe: {0}")]
    MalformedProbe(String),
    /// The oracle is not available.
    #[error("oracle unavailable: {0}")]
    Unavailable(String),
}

/// A source of observations about a hidden target.
pub trait Oracle: Receipts {
    /// Executes one probe on behalf of `operator` for `inquiry`.
    ///
    /// A successful probe returns an [`Observation`] and issues exactly one
    /// [`ResourceReceipt`], retrievable through [`Receipts::drain_receipts`].
    fn probe(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        probe: &Representation,
    ) -> Result<Observation, OracleError>;

    /// Probes still allowed by the protocol, or `None` if uncapped.
    fn probes_remaining(&self) -> Option<u64>;
}

/// Public documents attached to a question.
///
/// A question names its documents by hash in its parameters; the world
/// serves their content. They are static and public, so reading them is not
/// an external operation.
pub trait Knowledge {
    /// The document called `name`, if the world has it.
    fn document(&self, name: &str) -> Option<&Representation>;
}

/// A response from an external service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceResponse {
    /// The content.
    pub content: Representation,
    /// Who or what answered.
    pub answered_by: String,
}

/// Errors from consulting an external service.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// The request has been issued but not answered yet. The episode
    /// suspends; resuming retries the same operator.
    #[error("request {request_id} is pending")]
    Pending {
        /// Identifier of the pending request.
        request_id: String,
    },
    /// No such service in this world.
    #[error("service `{0}` is not available")]
    Unavailable(String),
    /// The service answered something unusable.
    #[error("malformed service response: {0}")]
    Malformed(String),
    /// The service declined.
    #[error("service refused: {0}")]
    Refused(String),
}

/// An outside service the system may consult, such as a language model.
pub trait ExternalService: Receipts {
    /// Consults `service` on behalf of `operator` for `inquiry`.
    ///
    /// A successful call issues exactly one [`ResourceReceipt`] and one
    /// [`ServiceAnswer`].
    fn consult(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        service: &str,
        request: &Representation,
    ) -> Result<ServiceResponse, ServiceError>;
}

impl<R: Receipts + ?Sized> Receipts for Box<R> {
    fn drain_receipts(&mut self) -> Vec<ResourceReceipt> {
        (**self).drain_receipts()
    }

    fn drain_service_answers(&mut self) -> Vec<ServiceAnswer> {
        (**self).drain_service_answers()
    }
}

impl<O: Oracle + ?Sized> Oracle for Box<O> {
    fn probe(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        probe: &Representation,
    ) -> Result<Observation, OracleError> {
        (**self).probe(operator, inquiry, probe)
    }

    fn probes_remaining(&self) -> Option<u64> {
        (**self).probes_remaining()
    }
}

impl<K: Knowledge + ?Sized> Knowledge for Box<K> {
    fn document(&self, name: &str) -> Option<&Representation> {
        (**self).document(name)
    }
}

impl<S: ExternalService + ?Sized> ExternalService for Box<S> {
    fn consult(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        service: &str,
        request: &Representation,
    ) -> Result<ServiceResponse, ServiceError> {
        (**self).consult(operator, inquiry, service, request)
    }
}
