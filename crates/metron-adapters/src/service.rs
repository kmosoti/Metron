//! The backend interface shared by the service adapters.
//!
//! A composed world owns one identifier counter and one receipt log; the
//! backends draw identifiers from it and hand their receipts and answers
//! back through this interface.

use metron_core::id::{InquiryId, OperatorId};
use metron_core::inquiry::Representation;
use metron_core::receipt::ResourceReceipt;
use metron_core::world::{IdSource, ServiceAnswer, ServiceError, ServiceResponse};

/// A service backend that can be composed into a world.
pub trait ServiceBackend: Send {
    /// Name of the service this backend answers for.
    fn service(&self) -> &str;

    /// Consults the service, drawing identifiers from `ids`.
    fn consult_using(
        &mut self,
        ids: &mut dyn IdSource,
        operator: &OperatorId,
        inquiry: InquiryId,
        request: &Representation,
    ) -> Result<ServiceResponse, ServiceError>;

    /// Receipts issued since the last take.
    fn take_receipts(&mut self) -> Vec<ResourceReceipt>;

    /// Answers recorded since the last take.
    fn take_answers(&mut self) -> Vec<ServiceAnswer>;
}
