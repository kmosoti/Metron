//! Frames the reference operators read and write.

use metron_core::frame::Frame;

/// Raw observations, as the core defines it.
pub const OBSERVATIONS: &str = metron_core::frame::OBSERVATIONS;
/// A truth table with some rows unknown.
pub const TRUTH_TABLE_PARTIAL: &str = "truth-table/partial";
/// A truth table with every row filled.
pub const TRUTH_TABLE_COMPLETE: &str = "truth-table/complete";

/// Descriptions of every frame used here.
#[must_use]
pub fn frames() -> Vec<Frame> {
    vec![
        Frame::new(
            OBSERVATIONS,
            "Probe/result pairs exactly as the oracle returned them.",
        ),
        Frame::new(
            TRUTH_TABLE_PARTIAL,
            "One entry per row of the function: 0, 1, or null when unobserved.",
        ),
        Frame::new(
            TRUTH_TABLE_COMPLETE,
            "One bit per row of the function; bit i is f at assignment i (x_j = bit j of i).",
        ),
    ]
}
