//! Host-private versioned intent contracts: schemas, descriptor-safe external I/O,
//! host authentication/validation, immutable freezing, read-only audit, native proposal,
//! closed-registry verifier, bounded runner, and 2x2 experiment gate.

mod cli;
mod experiment;
mod io;
mod propose;
mod runner;
#[cfg(test)]
mod tests;
mod types;
mod validation;
mod verifier;

pub(crate) use cli::run;
