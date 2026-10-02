//! Resource receipts.
//!
//! Every operation executed outside the system — an oracle probe, an external
//! call — produces a [`ResourceReceipt`]. Receipts are issued by the port that
//! executed the operation, not by the operator that asked for it, and the
//! runner copies every receipt into the episode journal. That is what makes
//! cost accounting and replay possible.

use crate::cost::Cost;
use crate::hash::ContentHash;
use crate::id::{InquiryId, ObservationId, OperatorId, ReceiptId};
use serde::{Deserialize, Serialize};

/// The kind of external operation a receipt is for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationKind {
    /// A probe answered by an oracle.
    OracleProbe,
    /// A call to some other external service.
    ExternalCall {
        /// Service name.
        service: String,
    },
}

/// A receipt for one externally executed operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceReceipt {
    /// Identifier, unique within an episode.
    pub id: ReceiptId,
    /// What was executed.
    pub operation: OperationKind,
    /// Operator on whose behalf.
    pub operator: OperatorId,
    /// Inquiry it served.
    pub inquiry: InquiryId,
    /// Hash of the request content.
    pub request_hash: ContentHash,
    /// Hash of the response content.
    pub response_hash: ContentHash,
    /// Resources charged.
    pub cost: Cost,
    /// Observations the operation produced.
    #[serde(default)]
    pub observations: Vec<ObservationId>,
    /// Wall-clock nanoseconds the operation took. Informational; excluded
    /// from `hash`.
    #[serde(default)]
    pub wall_nanos: u64,
    /// Hash of every field above except `wall_nanos`.
    pub hash: ContentHash,
}

/// The fields a receipt's hash covers.
#[derive(Serialize)]
struct Sealed<'a> {
    id: ReceiptId,
    operation: &'a OperationKind,
    operator: &'a OperatorId,
    inquiry: InquiryId,
    request_hash: &'a ContentHash,
    response_hash: &'a ContentHash,
    cost: &'a Cost,
    observations: &'a [ObservationId],
}

impl ResourceReceipt {
    /// Issues a sealed receipt.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn seal(
        id: ReceiptId,
        operation: OperationKind,
        operator: OperatorId,
        inquiry: InquiryId,
        request_hash: ContentHash,
        response_hash: ContentHash,
        cost: Cost,
        observations: Vec<ObservationId>,
        wall_nanos: u64,
    ) -> Self {
        let hash = Self::digest(
            id,
            &operation,
            &operator,
            inquiry,
            &request_hash,
            &response_hash,
            &cost,
            &observations,
        );
        Self {
            id,
            operation,
            operator,
            inquiry,
            request_hash,
            response_hash,
            cost,
            observations,
            wall_nanos,
            hash,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn digest(
        id: ReceiptId,
        operation: &OperationKind,
        operator: &OperatorId,
        inquiry: InquiryId,
        request_hash: &ContentHash,
        response_hash: &ContentHash,
        cost: &Cost,
        observations: &[ObservationId],
    ) -> ContentHash {
        ContentHash::of_json(&Sealed {
            id,
            operation,
            operator,
            inquiry,
            request_hash,
            response_hash,
            cost,
            observations,
        })
        .expect("receipts are always serialisable")
    }

    /// Whether the hash matches the contents.
    #[must_use]
    pub fn verify(&self) -> bool {
        self.hash
            == Self::digest(
                self.id,
                &self.operation,
                &self.operator,
                self.inquiry,
                &self.request_hash,
                &self.response_hash,
                &self.cost,
                &self.observations,
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_excludes_wall_time_and_detects_edits() {
        let mk = |wall| {
            ResourceReceipt::seal(
                ReceiptId(1),
                OperationKind::OracleProbe,
                OperatorId::from("probe"),
                InquiryId(1),
                ContentHash::of_bytes(b"req"),
                ContentHash::of_bytes(b"res"),
                Cost::probes(1),
                vec![ObservationId(1)],
                wall,
            )
        };
        let a = mk(10);
        let b = mk(20);
        assert_eq!(a.hash, b.hash);
        assert!(a.verify());
        let mut tampered = a.clone();
        tampered.cost = Cost::probes(2);
        assert!(!tampered.verify());
    }
}
