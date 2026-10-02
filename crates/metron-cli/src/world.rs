//! The composed world.

use metron_adapters::ServiceBackend;
use metron_core::evidence::Observation;
use metron_core::id::{InquiryId, OperatorId};
use metron_core::inquiry::Representation;
use metron_core::receipt::ResourceReceipt;
use metron_core::world::{
    Counter, ExternalService, Knowledge, Oracle, OracleError, Receipts, ServiceAnswer,
    ServiceError, ServiceResponse,
};
use metron_lab::LabWorld;
use metron_lab::world::WorldState;
use serde::{Deserialize, Serialize};

/// Public state of a composed world, for checkpoints.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposedState {
    /// Laboratory state.
    pub lab: WorldState,
    /// Shared identifier counter.
    pub ids: Counter,
}

/// The laboratory oracle and knowledge plus an optional service backend,
/// sharing one identifier counter so receipts and observations are unique
/// across ports.
pub struct ComposedWorld {
    lab: LabWorld,
    backend: Option<Box<dyn ServiceBackend>>,
    ids: Counter,
}

impl std::fmt::Debug for ComposedWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposedWorld")
            .field("lab", &self.lab)
            .field(
                "backend",
                &self.backend.as_ref().map(|b| b.service().to_owned()),
            )
            .field("ids", &self.ids)
            .finish()
    }
}

impl ComposedWorld {
    /// A world with the laboratory only. Consultations are unavailable.
    #[must_use]
    pub fn new(lab: LabWorld) -> Self {
        Self {
            lab,
            backend: None,
            ids: Counter::default(),
        }
    }

    /// Adds a service backend.
    #[must_use]
    pub fn with_backend(mut self, backend: Box<dyn ServiceBackend>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// The laboratory.
    #[must_use]
    pub fn lab(&self) -> &LabWorld {
        &self.lab
    }

    /// The backend, if any.
    #[must_use]
    pub fn backend(&self) -> Option<&dyn ServiceBackend> {
        self.backend.as_deref()
    }

    /// Public state, for checkpoints.
    #[must_use]
    pub fn state(&self) -> ComposedState {
        ComposedState {
            lab: self.lab.state(),
            ids: self.ids.clone(),
        }
    }

    /// Restores public state.
    pub fn restore(&mut self, state: ComposedState) {
        self.lab.restore(state.lab);
        self.ids = state.ids;
    }
}

impl Receipts for ComposedWorld {
    fn drain_receipts(&mut self) -> Vec<ResourceReceipt> {
        let mut receipts = self.lab.drain_receipts();
        if let Some(b) = &mut self.backend {
            receipts.extend(b.take_receipts());
        }
        receipts.sort_by_key(|r| r.id);
        receipts
    }

    fn drain_service_answers(&mut self) -> Vec<ServiceAnswer> {
        self.backend
            .as_mut()
            .map_or_else(Vec::new, |b| b.take_answers())
    }
}

impl Oracle for ComposedWorld {
    fn probe(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        probe: &Representation,
    ) -> Result<Observation, OracleError> {
        self.lab
            .probe_using(&mut self.ids, operator, inquiry, probe)
    }

    fn probes_remaining(&self) -> Option<u64> {
        self.lab.probes_remaining()
    }
}

impl Knowledge for ComposedWorld {
    fn document(&self, name: &str) -> Option<&Representation> {
        self.lab.document(name)
    }
}

impl ExternalService for ComposedWorld {
    fn consult(
        &mut self,
        operator: &OperatorId,
        inquiry: InquiryId,
        service: &str,
        request: &Representation,
    ) -> Result<ServiceResponse, ServiceError> {
        match &mut self.backend {
            Some(b) if b.service() == service => {
                b.consult_using(&mut self.ids, operator, inquiry, request)
            }
            _ => Err(ServiceError::Unavailable(service.to_owned())),
        }
    }
}
