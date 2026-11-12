//! Results supplied by an explicitly admitted effect port.
use metron_protocol::Receipt;

/// Failure is uncertainty: an effect may have occurred before the error.
pub enum DriverOutcome {
    Receipt(Box<Receipt>),
    Uncertain,
}
