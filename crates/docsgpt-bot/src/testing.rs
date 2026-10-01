//! Test helpers (feature `testing`): a [`Surface`] that records every call.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use docsgpt::Download;

use crate::error::{Error, Result};
use crate::runtime::{CancelGuard, CancelRegistry};
use crate::surface::{Final, Progress, Surface, Turn};

/// One call a [`FakeSurface`] received.
#[derive(Debug, Clone, PartialEq)]
pub enum Recorded {
    /// `begin`, with the agent and question.
    Begin {
        /// Agent name.
        agent: String,
        /// Question sent to DocsGPT.
        question: String,
    },
    /// `update`.
    Update {
        /// Partial answer.
        answer: String,
        /// Tool status.
        status: Option<String>,
        /// Reasoning, no answer yet.
        thinking: bool,
    },
    /// `finish`.
    Finish(Final),
    /// `send_file`.
    File {
        /// File name.
        filename: String,
        /// MIME type.
        mime: Option<String>,
        /// Size in bytes.
        len: usize,
    },
    /// `notice`.
    Notice(String),
}

/// A [`Surface`] that records calls instead of talking to a platform.
pub struct FakeSurface {
    calls: Mutex<Vec<Recorded>>,
    interval: Duration,
    message_id: Option<String>,
    fail_files: bool,
    cancels: Option<(CancelRegistry, String)>,
}

impl Default for FakeSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSurface {
    /// Updates at most every 50 ms; `finish` reports message id `"msg-1"`.
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            interval: Duration::from_millis(50),
            message_id: Some("msg-1".into()),
            fail_files: false,
            cancels: None,
        }
    }

    /// Minimum time between updates.
    pub fn interval(mut self, d: Duration) -> Self {
        self.interval = d;
        self
    }

    /// Message id `finish` returns.
    pub fn message_id(mut self, id: Option<&str>) -> Self {
        self.message_id = id.map(str::to_string);
        self
    }

    /// Make `send_file` fail.
    pub fn fail_files(mut self) -> Self {
        self.fail_files = true;
        self
    }

    /// Register each turn's cancel token in `registry` under `key` on `begin`.
    pub fn stop_with(mut self, registry: CancelRegistry, key: &str) -> Self {
        self.cancels = Some((registry, key.to_string()));
        self
    }

    /// Every call so far.
    pub fn calls(&self) -> Vec<Recorded> {
        self.calls.lock().unwrap().clone()
    }

    /// The `update` calls as `(answer, status)`.
    pub fn updates(&self) -> Vec<(String, Option<String>)> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Recorded::Update { answer, status, .. } => Some((answer, status)),
                _ => None,
            })
            .collect()
    }

    /// The `finish` result; panics if `finish` wasn't called exactly once.
    pub fn finished(&self) -> Final {
        let finals: Vec<Final> = self
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                Recorded::Finish(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(finals.len(), 1, "expected one finish; calls: {:#?}", self.calls());
        finals.into_iter().next().unwrap()
    }

    /// Notices sent.
    pub fn notices(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Recorded::Notice(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    /// Files sent, by name.
    pub fn files(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Recorded::File { filename, .. } => Some(filename),
                _ => None,
            })
            .collect()
    }

    /// Wait until an update satisfies `pred`; panics after `timeout`.
    pub async fn wait_update(&self, pred: impl Fn(&str, Option<&str>) -> bool, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.updates().iter().any(|(a, s)| pred(a, s.as_deref())) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no matching update; calls: {:#?}",
                self.calls()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn record(&self, r: Recorded) {
        self.calls.lock().unwrap().push(r);
    }
}

/// The draft of a [`FakeSurface`]: holds the Stop registration for the turn.
pub struct FakeDraft {
    _cancel: Option<CancelGuard>,
}

#[async_trait]
impl Surface for FakeSurface {
    type Draft = FakeDraft;

    async fn begin(&self, turn: &Turn) -> Result<FakeDraft> {
        self.record(Recorded::Begin {
            agent: turn.agent.name.clone(),
            question: turn.question.clone(),
        });
        let cancel = self
            .cancels
            .as_ref()
            .map(|(reg, key)| reg.insert(key.clone(), turn.cancel.clone()));
        Ok(FakeDraft { _cancel: cancel })
    }

    async fn update(&self, _turn: &Turn, _draft: &mut FakeDraft, p: Progress<'_>) -> Result<()> {
        self.record(Recorded::Update {
            answer: p.answer.to_string(),
            status: p.status.map(str::to_string),
            thinking: p.thinking,
        });
        Ok(())
    }

    async fn finish(&self, _turn: &Turn, _draft: FakeDraft, result: &Final) -> Result<Option<String>> {
        self.record(Recorded::Finish(result.clone()));
        Ok(self.message_id.clone())
    }

    async fn send_file(&self, _turn: &Turn, file: Download) -> Result<()> {
        if self.fail_files {
            return Err(Error::platform("upload rejected"));
        }
        self.record(Recorded::File {
            filename: file.filename,
            mime: file.mime,
            len: file.bytes.len(),
        });
        Ok(())
    }

    async fn notice(&self, text: &str) -> Result<()> {
        self.record(Recorded::Notice(text.to_string()));
        Ok(())
    }

    fn update_interval(&self) -> Duration {
        self.interval
    }
}
