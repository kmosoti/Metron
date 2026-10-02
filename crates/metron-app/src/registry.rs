//! The operator registry.

use metron_core::frame::TransformContract;
use metron_core::id::OperatorId;
use metron_core::operator::{Operator, OperatorSpec, SpecError};
use std::collections::BTreeMap;

/// Errors from registering an operator.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// An operator with this identifier is already registered.
    #[error("operator {0} is already registered")]
    Duplicate(OperatorId),
    /// The operator's spec is malformed.
    #[error(transparent)]
    Spec(#[from] SpecError),
}

/// Operators available to a runner, in registration order.
pub struct OperatorRegistry<W> {
    operators: Vec<Box<dyn Operator<W>>>,
    specs: Vec<OperatorSpec>,
    index: BTreeMap<OperatorId, usize>,
}

impl<W> Default for OperatorRegistry<W> {
    fn default() -> Self {
        Self {
            operators: Vec::new(),
            specs: Vec::new(),
            index: BTreeMap::new(),
        }
    }
}

impl<W> OperatorRegistry<W> {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an operator after validating its spec.
    pub fn register(&mut self, operator: impl Operator<W> + 'static) -> Result<(), RegistryError> {
        self.register_boxed(Box::new(operator))
    }

    /// Registers a boxed operator after validating its spec.
    pub fn register_boxed(&mut self, operator: Box<dyn Operator<W>>) -> Result<(), RegistryError> {
        let spec = operator.spec();
        spec.validate()?;
        if self.index.contains_key(&spec.id) {
            return Err(RegistryError::Duplicate(spec.id));
        }
        self.index.insert(spec.id.clone(), self.operators.len());
        self.specs.push(spec);
        self.operators.push(operator);
        Ok(())
    }

    /// Registers several operators.
    pub fn register_all(
        &mut self,
        operators: impl IntoIterator<Item = Box<dyn Operator<W>>>,
    ) -> Result<(), RegistryError> {
        for op in operators {
            self.register_boxed(op)?;
        }
        Ok(())
    }

    /// Looks up an operator.
    #[must_use]
    pub fn get(&self, id: &OperatorId) -> Option<&dyn Operator<W>> {
        self.index.get(id).map(|&i| self.operators[i].as_ref())
    }

    /// Looks up a spec.
    #[must_use]
    pub fn spec(&self, id: &OperatorId) -> Option<&OperatorSpec> {
        self.index.get(id).map(|&i| &self.specs[i])
    }

    /// All specs, in registration order.
    #[must_use]
    pub fn specs(&self) -> &[OperatorSpec] {
        &self.specs
    }

    /// All identifiers, in registration order.
    #[must_use]
    pub fn ids(&self) -> Vec<OperatorId> {
        self.specs.iter().map(|s| s.id.clone()).collect()
    }

    /// Every transform contract, with its operator.
    #[must_use]
    pub fn contracts(&self) -> Vec<(OperatorId, TransformContract)> {
        self.specs
            .iter()
            .filter_map(|s| s.kind.contract().map(|c| (s.id.clone(), c.clone())))
            .collect()
    }

    /// Number of operators.
    #[must_use]
    pub fn len(&self) -> usize {
        self.operators.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operators.is_empty()
    }
}
