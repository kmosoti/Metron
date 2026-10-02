//! Adapters: implementations of Metron ports against the outside world.
//!
//! Everything here is an effect: reading the clock, reading and writing
//! files. The core, the app, the lab and the operators never do these things
//! themselves; the composition root (`metron-cli`) hands them adapters.
//!
//! Model clients belong here too, when the experimental programme reaches
//! them. None exist yet, on purpose.

pub mod clock;
pub mod files;
pub mod results;

pub use clock::SystemClock;
pub use files::{FileError, load_json, read_jsonl, read_text, write_json_pretty, write_jsonl};
pub use results::{ResultsWriter, RunRecord};
