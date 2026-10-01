//! Shared plumbing for chat bots that answer with [DocsGPT](https://www.docsgpt.cloud/) agents.
//!
//! - [`config`]: building blocks for a bot's TOML config, with `${VAR}` expansion.
//! - [`Agents`]: a bot's agents and `#name` routing.
//! - [`storage`]: which DocsGPT conversation each chat is in, turn counts for
//!   feedback, per-chat state; SQLite or memory.
//! - [`runtime`]: one turn at a time per chat, stoppable turns, graceful shutdown.
//! - [`markdown`]: splitting long answers into messages without breaking code blocks.
//! - [`run_turn`]: the answer loop. A platform implements [`Surface`] (begin,
//!   update, finish, send a file) and gets streaming, tool status, sources,
//!   files, Stop, and feedback bookkeeping.
//!
//! The DocsGPT API client is the [`docsgpt`] crate, re-exported here.

pub mod agents;
pub mod config;
mod error;
pub mod markdown;
pub mod runtime;
pub mod storage;
mod surface;
#[cfg(feature = "testing")]
pub mod testing;
mod turn;
pub mod util;

pub use agents::{Agents, Routed};
pub use config::AgentConfig;
pub use docsgpt;
pub use error::{BoxError, Error, Result};
pub use runtime::{CancelGuard, CancelRegistry, ScopeLocks, Shutdown};
pub use storage::{ChatState, Conversation, MessageRef, Scope, StatePatch, Storage};
pub use surface::{Final, Outcome, Progress, Surface, Turn};
pub use turn::{Ask, BotCore, MESSAGE_REF_TTL, TurnOptions, TurnReport, run_turn, submit_feedback};
