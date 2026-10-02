//! Adapters: implementations of Metron ports against the outside world.
//!
//! Everything here is an effect: reading the clock, reading and writing
//! files, waiting for a person or an agent to answer. The core, the app, the
//! lab and the operators never do these things themselves; the composition
//! root (`metron-cli`) hands them adapters.
//!
//! * [`SystemClock`] — the operating-system clock.
//! * [`files`] and [`results`] — JSON and JSON Lines persistence, run
//!   directories.
//! * [`claude_code::ClaudeCodeBridge`] — the language-model service,
//!   implemented as a file-based request/response protocol serviced by a
//!   Claude Code session. There is no API client and none is planned: the
//!   inference available to this project is the agent session that runs
//!   the experiment.
//! * [`replay::ReplayService`] — answers consultations from a recorded
//!   journal, so an episode can be re-run without the agent.
//! * [`stub::StubService`] — a scripted service for tests.

pub mod claude_code;
pub mod clock;
pub mod files;
pub mod replay;
pub mod results;
pub mod service;
pub mod stub;

pub use claude_code::ClaudeCodeBridge;
pub use clock::SystemClock;
pub use files::{FileError, load_json, read_jsonl, read_text, write_json_pretty, write_jsonl};
pub use replay::ReplayService;
pub use results::{ResultsWriter, RunRecord};
pub use service::ServiceBackend;
pub use stub::StubService;
