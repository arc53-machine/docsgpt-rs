//! What a chat platform implements so [`crate::run_turn`] can answer on it.

use std::time::Duration;

use async_trait::async_trait;
use docsgpt::{Download, Source};
use tokio_util::sync::CancellationToken;

use crate::config::AgentConfig;
use crate::error::Result;
use crate::storage::Scope;

/// The turn being answered, as the [`Surface`] sees it.
#[derive(Debug, Clone)]
pub struct Turn {
    /// Bot name.
    pub bot: String,
    /// Where the conversation lives.
    pub scope: Scope,
    /// The agent answering.
    pub agent: AgentConfig,
    /// The question sent to DocsGPT (any `#agent` prefix removed).
    pub question: String,
    /// Cancelling this stops the turn. Register it under the id the user will
    /// press Stop on (see [`crate::CancelRegistry::insert`]).
    pub cancel: CancellationToken,
}

/// A partial answer for a live draft.
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    /// Answer so far, as Markdown: images removed and any open code fence closed.
    pub answer: &'a str,
    /// What a tool is doing right now ("Running code"), if anything.
    pub status: Option<&'a str>,
    /// The model is reasoning and no answer text has arrived yet.
    pub thinking: bool,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The answer is complete.
    Complete,
    /// The user stopped it; the answer may be partial or empty.
    Stopped,
    /// DocsGPT or the connection failed; the answer may be partial or empty.
    Failed {
        /// A short reason fit to show the user.
        detail: String,
    },
}

/// Everything to deliver at the end of a turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Final {
    /// The answer as Markdown with images removed (see `images`). Trimmed; may be empty.
    pub answer: String,
    /// Image URLs: from the answer's Markdown, then from tools.
    pub images: Vec<String>,
    /// Sources DocsGPT used.
    pub sources: Vec<Source>,
    /// How the turn ended.
    pub outcome: Outcome,
    /// The DocsGPT conversation, if one was created.
    pub conversation_id: Option<String>,
    /// The answer's position in the conversation (feedback's `question_index`).
    pub position: Option<u32>,
}

impl Final {
    /// The text to show: the answer, plus a note when it was cut short; or a
    /// short message when there is no answer at all.
    pub fn display_text(&self) -> String {
        match (&self.outcome, self.answer.is_empty()) {
            (Outcome::Complete, false) => self.answer.clone(),
            (Outcome::Complete, true) => "Sorry, I couldn't get an answer right now.\n\nNo answer was produced.".into(),
            (Outcome::Stopped, true) => "Stopped.".into(),
            (Outcome::Stopped, false) => format!("{}\n\n_Stopped._", self.answer),
            (Outcome::Failed { detail }, true) => format!("Sorry, I couldn't get an answer right now.\n\n{detail}"),
            (Outcome::Failed { detail }, false) => format!("{}\n\n_The answer was cut short: {detail}_", self.answer),
        }
    }

    /// True when a 👍/👎 on this answer can be sent to DocsGPT.
    pub fn can_rate(&self) -> bool {
        self.conversation_id.is_some() && self.position.is_some() && !self.answer.is_empty()
    }
}

/// One chat platform's way of showing a turn. Build one per incoming message,
/// holding where to reply (channel, thread, user).
///
/// [`crate::run_turn`] calls [`Surface::begin`] once, [`Surface::update`] as
/// the answer streams in (at most every [`Surface::update_interval`]), and
/// [`Surface::finish`] exactly once, then [`Surface::send_file`] for each
/// file a tool produced. Errors from `update` are logged and the turn goes on.
#[async_trait]
pub trait Surface: Send + Sync {
    /// The platform's handle on the message being drafted.
    type Draft: Send;

    /// Start showing the turn: a typing indicator, a placeholder message, a
    /// stream. Register [`Turn::cancel`] here if the platform has a Stop button.
    async fn begin(&self, turn: &Turn) -> Result<Self::Draft>;

    /// Show a partial answer.
    async fn update(&self, turn: &Turn, draft: &mut Self::Draft, progress: Progress<'_>) -> Result<()>;

    /// Deliver the end of the turn, whatever the [`Outcome`]. Returns the
    /// platform's id for the message holding the answer, if the user can rate it
    /// there; the bot stores which answer it holds for [`crate::submit_feedback`].
    async fn finish(&self, turn: &Turn, draft: Self::Draft, result: &Final) -> Result<Option<String>>;

    /// Send a file a tool produced (after `finish`).
    async fn send_file(&self, turn: &Turn, file: Download) -> Result<()>;

    /// Send a short message outside a turn (unknown `#agent`, agent switched, a
    /// file that couldn't be attached).
    async fn notice(&self, text: &str) -> Result<()>;

    /// Minimum time between two [`Surface::update`] calls.
    fn update_interval(&self) -> Duration {
        Duration::from_millis(500)
    }
}
