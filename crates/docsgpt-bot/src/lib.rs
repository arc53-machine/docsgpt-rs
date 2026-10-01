//! Shared plumbing for chat bots that answer with [DocsGPT](https://www.docsgpt.cloud/) agents.
//!
//! - [`config`]: building blocks for a bot's TOML config, with `${VAR}` expansion.
//! - [`Agents`]: a bot's agents and `#name` routing.
//! - [`storage`]: which DocsGPT conversation each chat is in, turn counts for
//!   feedback, per-chat state; SQLite or memory.
//! - [`runtime`]: one turn at a time per chat, stoppable turns, graceful shutdown.
//! - [`markdown`]: splitting long answers into messages without breaking code blocks.
//!
//! The DocsGPT API client is the [`docsgpt`] crate, re-exported here.

pub mod agents;
pub mod config;
mod error;
pub mod markdown;
pub mod runtime;
pub mod storage;
pub mod util;

pub use agents::{Agents, Routed};
pub use config::AgentConfig;
pub use docsgpt;
pub use error::{BoxError, Error, Result};
pub use runtime::{CancelGuard, CancelRegistry, ScopeLocks, Shutdown};
pub use storage::{ChatState, Conversation, MessageRef, Scope, StatePatch, Storage};
