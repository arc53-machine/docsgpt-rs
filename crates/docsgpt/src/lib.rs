//! Rust client for the [DocsGPT](https://www.docsgpt.cloud/) API.
//!
//! - [`Client::stream`] asks an agent a question and yields the answer as
//!   [`Event`]s while it is generated; [`Client::answer`] waits for the whole reply.
//! - [`Client::upload_attachment`], [`Client::stt`] and [`Client::tts`] handle
//!   files and speech; [`Client::download_artifact`] fetches files that tools
//!   produced (see [`ToolCall::outputs`]).
//! - [`Client::feedback`] rates an answer.
//!
//! Every call authenticates with an agent API key (DocsGPT → Agents → your
//! agent → API key).
//!
//! The `mock` feature adds [`mock`], an in-process fake DocsGPT server for
//! testing code built on this crate.

mod client;
mod error;
pub mod events;
pub mod repr;
pub mod sse;

#[cfg(feature = "mock")]
pub mod mock;

pub use client::{
    Answer, AskRequest, Attachment, Client, ClientBuilder, Download, EventStream, Feedback, RetryPolicy, TaskWait,
    Upload,
};
pub use error::{Error, Result};
pub use events::{ArtifactRef, Event, Source, ToolCall, ToolOutputs};
