#![forbid(unsafe_code)]
//! Local artifact verification and bounded worker-process transport.

pub mod artifact;
pub mod chunk_prefill;
pub mod framing;
pub mod resources;

pub mod driver;

pub mod tokenizer;

pub mod paired_history;

pub mod comparison;
pub mod qualification;
