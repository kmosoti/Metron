#![forbid(unsafe_code)]
//! Local artifact verification and bounded worker-process transport.

pub mod artifact;
pub mod chunk_prefill;
pub mod framing;

pub mod driver;

pub mod tokenizer;
