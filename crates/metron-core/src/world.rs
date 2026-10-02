//! The ports through which the system touches the world.
//!
//! The system never holds a hidden target. It holds a `W` that implements
//! [`Oracle`], asks it questions, and gets back observations with receipts.
//! Whoever implements the port (the laboratory, in experiments) decides the
//! protocol, enforces the probe cap, and keeps the ground truth to itself.

use crate::evidence::Observation;
use crate::id::{InquiryId, OperatorId};
use crate::inquiry::Representation;
use crate::receipt::ResourceReceipt;

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
pub trait Oracle {
    /// Executes one probe on behalf of `operator` for `inquiry`.
    ///
    /// A successful probe returns an [`Observation`] and issues exactly one
    /// [`ResourceReceipt`], retrievable through [`Oracle::drain_receipts`].
    fn probe(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        probe: &Representation,
    ) -> Result<Observation, OracleError>;

    /// Receipts issued since the last drain, in issue order.
    fn drain_receipts(&mut self) -> Vec<ResourceReceipt>;

    /// Probes still allowed by the protocol, or `None` if uncapped.
    fn probes_remaining(&self) -> Option<u64>;
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

    fn drain_receipts(&mut self) -> Vec<ResourceReceipt> {
        (**self).drain_receipts()
    }

    fn probes_remaining(&self) -> Option<u64> {
        (**self).probes_remaining()
    }
}
