//! The DocsGPT HTTP client.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::{Error, Result};
use crate::events::{Event, Source, ToolCall};
use crate::sse::SseParser;

const STREAM: &str = "/stream";
const ANSWER: &str = "/api/answer";
const STORE_ATTACHMENT: &str = "/api/store_attachment";
const TASK_STATUS: &str = "/api/task_status";
const STT: &str = "/api/stt";
const TTS: &str = "/api/tts";
const ARTIFACT: &str = "/api/artifacts/{id}/download";
const FEEDBACK: &str = "/api/feedback";
const FETCH: &str = "fetch_url";

/// How often, and how patiently, failed requests are retried.
///
/// Requests that are safe to repeat (downloads, task polling, speech, feedback)
/// are retried on connection failures, timeouts and 429/502/503/504. Requests
/// that start work on the server (`/stream`, `/api/answer`, uploads) are only
/// retried when the request provably never reached DocsGPT: a failed connection
/// or a 503 from a proxy in front of it.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts, including the first. `1` disables retries.
    pub max_attempts: u32,
    /// Delay before the first retry; doubles on each further retry.
    pub initial_backoff: Duration,
    /// Upper bound for one delay.
    pub max_backoff: Duration,
}

impl RetryPolicy {
    /// Never retry.
    pub fn none() -> Self {
        Self {
            max_attempts: 1,
            ..Self::default()
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(300),
            max_backoff: Duration::from_secs(3),
        }
    }
}

/// A question for an agent, sent to `/stream` or `/api/answer`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AskRequest {
    /// The user's question.
    pub question: String,
    /// The agent's API key (DocsGPT → Agents → your agent → API key).
    pub api_key: String,
    /// Conversation to continue; `None` starts a new one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Attachment ids from [`Client::upload_attachment`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// Model override, when the agent allows one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Set `false` to answer without storing the turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_conversation: Option<bool>,
    /// Values passed through to the agent's prompt template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passthrough: Option<Value>,
    /// Any other request fields, sent as is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AskRequest {
    /// A question for the agent identified by `api_key`.
    pub fn new(api_key: impl Into<String>, question: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    /// Continue an existing conversation.
    pub fn conversation_id(mut self, id: impl Into<String>) -> Self {
        self.conversation_id = Some(id.into());
        self
    }

    /// Attach previously uploaded files.
    pub fn attachments(mut self, ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.attachments = ids.into_iter().map(Into::into).collect();
        self
    }
}

/// The reply from the non-streaming `/api/answer` endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Answer {
    /// The answer text.
    #[serde(deserialize_with = "null_as_default")]
    pub answer: String,
    /// Conversation to pass on the next turn.
    pub conversation_id: Option<String>,
    /// Sources, as raw JSON objects; see [`Answer::sources`].
    #[serde(rename = "sources", deserialize_with = "null_as_default")]
    pub raw_sources: Vec<Value>,
    /// Tool calls made while answering.
    #[serde(deserialize_with = "null_as_default")]
    pub tool_calls: Vec<ToolCall>,
    /// Reasoning text, from models with visible thinking.
    pub thought: Option<String>,
}

impl Answer {
    /// Sources in the same shape as [`Event::Source`].
    pub fn sources(&self) -> Vec<Source> {
        let ev = json!({"type": "source", "source": self.raw_sources});
        match Event::parse(&ev.to_string()) {
            Some(Event::Source(s)) => s,
            _ => Vec::new(),
        }
    }
}

fn null_as_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A file to upload.
#[derive(Debug, Clone)]
pub struct Upload {
    /// File name, including the extension DocsGPT uses to pick a parser.
    pub filename: String,
    /// File contents.
    pub bytes: Bytes,
    /// MIME type, when known.
    pub mime: Option<String>,
}

impl Upload {
    /// A file with an unknown MIME type.
    pub fn new(filename: impl Into<String>, bytes: impl Into<Bytes>) -> Self {
        Self {
            filename: filename.into(),
            bytes: bytes.into(),
            mime: None,
        }
    }

    /// Set the MIME type.
    pub fn mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = Some(mime.into());
        self
    }

    fn part(&self) -> reqwest::multipart::Part {
        let make = || {
            reqwest::multipart::Part::stream_with_length(
                reqwest::Body::from(self.bytes.clone()),
                self.bytes.len() as u64,
            )
            .file_name(self.filename.clone())
        };
        match &self.mime {
            Some(m) => make().mime_str(m).unwrap_or_else(|_| make()),
            None => make(),
        }
    }
}

/// An uploaded attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Id to pass in [`AskRequest::attachments`].
    pub id: String,
    /// Background task processing the file, if the server started one.
    pub task_id: Option<String>,
}

/// How waiting for an attachment's processing task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskWait {
    /// The task finished successfully.
    Done,
    /// The server could not report the task's status; the caller may proceed.
    Unavailable,
    /// The task was still running when the wait ran out.
    StillRunning,
}

/// Feedback on an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feedback {
    /// Thumbs up.
    Like,
    /// Thumbs down.
    Dislike,
    /// Remove earlier feedback.
    Clear,
}

/// A downloaded file.
#[derive(Debug, Clone)]
pub struct Download {
    /// File name, from `Content-Disposition` or the URL.
    pub filename: String,
    /// File contents.
    pub bytes: Bytes,
    /// MIME type from `Content-Type`, without parameters.
    pub mime: Option<String>,
}

/// The parsed event stream of one `/stream` turn.
///
/// Yields [`Event`]s until the server closes the stream. A transport failure
/// or a silence longer than the idle timeout yields one `Err` and then ends.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<Event>> + Send>>,
}

impl EventStream {
    /// Wrap any stream of events, e.g. to replay a recorded turn in tests.
    pub fn from_stream(s: impl Stream<Item = Result<Event>> + Send + 'static) -> Self {
        Self { inner: Box::pin(s) }
    }
}

impl Stream for EventStream {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventStream")
    }
}

/// Configures a [`Client`].
#[derive(Debug)]
pub struct ClientBuilder {
    base: String,
    http: Option<reqwest::Client>,
    user_agent: String,
    retry: RetryPolicy,
    stream_idle_timeout: Duration,
    request_timeout: Duration,
    max_download: usize,
}

impl ClientBuilder {
    /// Use an existing `reqwest` client (shares its connection pool and settings).
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// User agent for the client this builder creates; ignored with [`Self::http_client`].
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// Retry behaviour; see [`RetryPolicy`].
    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// Longest silence tolerated on `/stream` before it fails with [`Error::Timeout`].
    /// DocsGPT sends keepalives while an agent works, so this only trips on a dead
    /// connection. Default: 150 seconds.
    pub fn stream_idle_timeout(mut self, d: Duration) -> Self {
        self.stream_idle_timeout = d;
        self
    }

    /// Timeout for every request other than `/stream`. Default: 120 seconds.
    pub fn request_timeout(mut self, d: Duration) -> Self {
        self.request_timeout = d;
        self
    }

    /// Largest file [`Client::download_artifact`] and [`Client::fetch_url`] accept.
    /// Default: 50 MiB.
    pub fn max_download_bytes(mut self, n: usize) -> Self {
        self.max_download = n;
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<Client> {
        let base = self.base.trim_end_matches('/').to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(Error::Config(format!(
                "base URL must start with http(s)://, got {base:?}"
            )));
        }
        let http = match self.http {
            Some(h) => h,
            None => reqwest::Client::builder()
                .user_agent(self.user_agent)
                .connect_timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| Error::Config(e.to_string()))?,
        };
        Ok(Client {
            inner: Arc::new(Inner {
                http,
                base,
                retry: self.retry,
                stream_idle_timeout: self.stream_idle_timeout,
                request_timeout: self.request_timeout,
                max_download: self.max_download,
            }),
        })
    }
}

/// Client for one DocsGPT server. Cheap to clone.
///
/// ```no_run
/// # async fn run() -> docsgpt::Result<()> {
/// use docsgpt::{AskRequest, Client, Event};
/// use futures_util::StreamExt;
///
/// let client = Client::new("https://gptcloud.arc53.com")?;
/// let mut events = client.stream(&AskRequest::new("agent-key", "What is DocsGPT?")).await?;
/// while let Some(ev) = events.next().await {
///     if let Event::Answer(delta) = ev? {
///         print!("{delta}");
///     }
/// }
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    base: String,
    retry: RetryPolicy,
    stream_idle_timeout: Duration,
    request_timeout: Duration,
    max_download: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Repeat {
    /// Repeating the request has no side effects.
    Safe,
    /// The request starts work; repeat only if it surely never arrived.
    Unsafe,
}

impl Client {
    /// A client for the DocsGPT server at `base` (e.g. `https://gptcloud.arc53.com`) with default settings.
    pub fn new(base: impl Into<String>) -> Result<Self> {
        Self::builder(base).build()
    }

    /// Start configuring a client for the server at `base`.
    pub fn builder(base: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            base: base.into(),
            http: None,
            user_agent: concat!("docsgpt-rs/", env!("CARGO_PKG_VERSION")).to_string(),
            retry: RetryPolicy::default(),
            stream_idle_timeout: Duration::from_secs(150),
            request_timeout: Duration::from_secs(120),
            max_download: 50 * 1024 * 1024,
        }
    }

    /// The server's base URL, without a trailing slash.
    pub fn base_url(&self) -> &str {
        &self.inner.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.inner.base, path)
    }

    /// Send a request built by `build`, retrying per the policy, and turn a
    /// non-success status into [`Error::Http`].
    async fn send(
        &self,
        endpoint: &'static str,
        repeat: Repeat,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let policy = &self.inner.retry;
        let mut delay = policy.initial_backoff;
        let mut attempt = 1;
        loop {
            let last = attempt >= policy.max_attempts.max(1);
            let outcome = build().send().await;
            let retry = match &outcome {
                Err(e) => e.is_connect() || (repeat == Repeat::Safe && e.is_timeout()),
                Ok(r) => match r.status().as_u16() {
                    503 => true,
                    429 | 502 | 504 => repeat == Repeat::Safe,
                    _ => false,
                },
            };
            if !retry || last {
                let resp = outcome.map_err(|e| Error::transport(endpoint, e))?;
                return check_status(endpoint, resp).await;
            }
            tracing::debug!(endpoint, attempt, ?delay, "retrying DocsGPT request");
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(policy.max_backoff);
            attempt += 1;
        }
    }

    async fn json(endpoint: &'static str, resp: reqwest::Response) -> Result<Value> {
        let text = resp.text().await.map_err(|e| Error::transport(endpoint, e))?;
        serde_json::from_str(&text).map_err(|e| Error::decode(endpoint, e.to_string()))
    }

    /// Ask a question and stream the answer as it is generated (`POST /stream`).
    pub async fn stream(&self, req: &AskRequest) -> Result<EventStream> {
        let resp = self
            .send(STREAM, Repeat::Unsafe, || {
                self.inner
                    .http
                    .post(self.url(STREAM))
                    .header(reqwest::header::ACCEPT, "text/event-stream")
                    .json(req)
            })
            .await?;
        let idle = self.inner.stream_idle_timeout;
        let state = (
            resp.bytes_stream(),
            SseParser::default(),
            std::collections::VecDeque::new(),
            false,
        );
        let stream =
            futures_util::stream::unfold(state, move |(mut bytes, mut parser, mut queue, mut done)| async move {
                loop {
                    if let Some(ev) = queue.pop_front() {
                        return Some((Ok(ev), (bytes, parser, queue, done)));
                    }
                    if done {
                        return None;
                    }
                    match tokio::time::timeout(idle, bytes.next()).await {
                        Err(_) => {
                            done = true;
                            return Some((Err(Error::Timeout { endpoint: STREAM }), (bytes, parser, queue, done)));
                        }
                        Ok(Some(Ok(chunk))) => {
                            queue.extend(parser.push(&chunk).iter().filter_map(|d| Event::parse(d)));
                        }
                        Ok(Some(Err(e))) => {
                            done = true;
                            return Some((Err(Error::transport(STREAM, e)), (bytes, parser, queue, done)));
                        }
                        Ok(None) => {
                            done = true;
                            queue.extend(parser.finish().as_deref().and_then(Event::parse));
                        }
                    }
                }
            });
        Ok(EventStream::from_stream(stream))
    }

    /// Ask a question and wait for the whole answer (`POST /api/answer`).
    pub async fn answer(&self, req: &AskRequest) -> Result<Answer> {
        let resp = self
            .send(ANSWER, Repeat::Unsafe, || {
                self.inner
                    .http
                    .post(self.url(ANSWER))
                    .json(req)
                    .timeout(self.inner.request_timeout)
            })
            .await?;
        let v = Self::json(ANSWER, resp).await?;
        serde_json::from_value(v).map_err(|e| Error::decode(ANSWER, e.to_string()))
    }

    /// Upload a file to reference in [`AskRequest::attachments`] (`POST /api/store_attachment`).
    ///
    /// The server may still be processing the file when this returns; see
    /// [`Client::wait_for_task`].
    pub async fn upload_attachment(&self, api_key: &str, file: &Upload) -> Result<Attachment> {
        let resp = self
            .send(STORE_ATTACHMENT, Repeat::Unsafe, || {
                let form = reqwest::multipart::Form::new()
                    .text("api_key", api_key.to_string())
                    .part("file", file.part());
                self.inner
                    .http
                    .post(self.url(STORE_ATTACHMENT))
                    .multipart(form)
                    .timeout(self.inner.request_timeout)
            })
            .await?;
        let v = Self::json(STORE_ATTACHMENT, resp).await?;
        // Single-file shape {attachment_id, task_id}, or multi-file {tasks: [{…}]}.
        let entry = match v.get("attachment_id") {
            Some(_) => &v,
            None => v.get("tasks").and_then(|t| t.get(0)).unwrap_or(&Value::Null),
        };
        let id = entry.get("attachment_id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() {
            return Err(Error::decode(
                STORE_ATTACHMENT,
                format!("no attachment id in {}", truncate(&v.to_string(), 300)),
            ));
        }
        Ok(Attachment {
            id: id.to_string(),
            task_id: entry.get("task_id").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// Wait for an attachment's processing task (`GET /api/task_status`), polling
    /// with backoff for at most `max_wait`.
    ///
    /// Fails only when the server reports the task failed; when the status can't be
    /// read, returns [`TaskWait::Unavailable`] so the caller can go ahead and ask anyway.
    pub async fn wait_for_task(&self, task_id: &str, max_wait: Duration) -> Result<TaskWait> {
        let started = Instant::now();
        let mut delay = Duration::from_millis(700);
        loop {
            let resp = self
                .send(TASK_STATUS, Repeat::Safe, || {
                    self.inner
                        .http
                        .get(self.url(TASK_STATUS))
                        .query(&[("task_id", task_id)])
                        .timeout(Duration::from_secs(20))
                })
                .await;
            let v = match resp {
                Ok(r) => Self::json(TASK_STATUS, r).await.unwrap_or(Value::Null),
                Err(e) => {
                    tracing::debug!(error = %e, "task status unavailable");
                    return Ok(TaskWait::Unavailable);
                }
            };
            let status = v
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_uppercase();
            match status.as_str() {
                "SUCCESS" => return Ok(TaskWait::Done),
                "FAILURE" | "REVOKED" => {
                    return Err(Error::TaskFailed {
                        task_id: task_id.to_string(),
                        detail: truncate(&v.to_string(), 300),
                    });
                }
                _ => {}
            }
            if started.elapsed() + delay > max_wait {
                return Ok(TaskWait::StillRunning);
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 3 / 2).min(Duration::from_secs(3));
        }
    }

    /// Transcribe audio to text (`POST /api/stt`).
    pub async fn stt(&self, api_key: &str, audio: &Upload) -> Result<String> {
        let resp = self
            .send(STT, Repeat::Safe, || {
                let form = reqwest::multipart::Form::new()
                    .text("api_key", api_key.to_string())
                    .part("file", audio.part());
                self.inner
                    .http
                    .post(self.url(STT))
                    .query(&[("api_key", api_key)])
                    .multipart(form)
                    .timeout(self.inner.request_timeout)
            })
            .await?;
        let v = Self::json(STT, resp).await?;
        let text = v.get("text").and_then(Value::as_str).unwrap_or("").trim();
        if text.is_empty() {
            return Err(Error::decode(STT, "speech recognition returned no text"));
        }
        Ok(text.to_string())
    }

    /// Synthesize speech (`POST /api/tts`); returns the audio bytes (usually MP3).
    pub async fn tts(&self, api_key: &str, text: &str) -> Result<Bytes> {
        let resp = self
            .send(TTS, Repeat::Safe, || {
                self.inner
                    .http
                    .post(self.url(TTS))
                    .json(&json!({"text": text, "api_key": api_key}))
                    .timeout(self.inner.request_timeout)
            })
            .await?;
        let v = Self::json(TTS, resp).await?;
        let b64 = v
            .get("audio_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::decode(TTS, "no audio_base64 in response"))?;
        use base64::Engine;
        let audio = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| Error::decode(TTS, e.to_string()))?;
        Ok(Bytes::from(audio))
    }

    /// Download a file a tool produced (`GET /api/artifacts/{id}/download`).
    ///
    /// Agent keys can only read artifacts of conversations they created, so the
    /// conversation is required.
    pub async fn download_artifact(&self, api_key: &str, conversation_id: &str, artifact_id: &str) -> Result<Download> {
        let url = self.url(&format!("/api/artifacts/{artifact_id}/download"));
        let resp = self
            .send(ARTIFACT, Repeat::Safe, || {
                self.inner
                    .http
                    .get(&url)
                    .query(&[("api_key", api_key), ("conversation_id", conversation_id)])
                    .timeout(self.inner.request_timeout)
            })
            .await?;
        self.read_download(ARTIFACT, resp, artifact_id).await
    }

    /// Download a public URL, such as an image-generation result.
    pub async fn fetch_url(&self, url: &str) -> Result<Download> {
        let resp = self
            .send(FETCH, Repeat::Safe, || {
                self.inner.http.get(url).timeout(self.inner.request_timeout)
            })
            .await?;
        let fallback = url
            .rsplit('/')
            .next()
            .unwrap_or("file")
            .split('?')
            .next()
            .unwrap_or("file");
        let fallback = if fallback.is_empty() { "file" } else { fallback };
        self.read_download(FETCH, resp, fallback).await
    }

    /// Rate an answer (`POST /api/feedback`).
    ///
    /// `question_index` is the answer's 0-based position in the conversation.
    pub async fn feedback(
        &self,
        api_key: &str,
        conversation_id: &str,
        question_index: u32,
        feedback: Feedback,
    ) -> Result<()> {
        let value = match feedback {
            Feedback::Like => json!("like"),
            Feedback::Dislike => json!("dislike"),
            Feedback::Clear => Value::Null,
        };
        let body = json!({
            "feedback": value,
            "conversation_id": conversation_id,
            "question_index": question_index,
            "api_key": api_key,
        });
        self.send(FEEDBACK, Repeat::Safe, || {
            self.inner
                .http
                .post(self.url(FEEDBACK))
                .json(&body)
                .timeout(self.inner.request_timeout)
        })
        .await?;
        Ok(())
    }

    async fn read_download(
        &self,
        endpoint: &'static str,
        resp: reqwest::Response,
        fallback_name: &str,
    ) -> Result<Download> {
        let header = |name| {
            resp.headers()
                .get(name)
                .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        };
        let mime = header(reqwest::header::CONTENT_TYPE).map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
        let filename = header(reqwest::header::CONTENT_DISPOSITION)
            .and_then(content_disposition_filename)
            .unwrap_or_else(|| fallback_name.to_string());
        let limit = self.inner.max_download;
        if resp.content_length().is_some_and(|n| n as usize > limit) {
            return Err(Error::TooLarge { limit });
        }
        let mut body = resp.bytes_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| Error::transport(endpoint, e))?;
            if buf.len() + chunk.len() > limit {
                return Err(Error::TooLarge { limit });
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(Download {
            filename,
            bytes: Bytes::from(buf),
            mime,
        })
    }
}

/// Turn a non-success response into an error. A speech endpoint whose
/// provider is switched off answers 404 `{"success": false, "message": …}`.
async fn check_status(endpoint: &'static str, resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    if status.as_u16() == 404
        && (endpoint == STT || endpoint == TTS)
        && let Ok(v) = serde_json::from_str::<Value>(&body)
        && v.get("success") == Some(&Value::Bool(false))
    {
        let message = v.get("message").and_then(Value::as_str).unwrap_or("").to_string();
        return Err(Error::FeatureDisabled { endpoint, message });
    }
    Err(Error::Http {
        endpoint,
        status: status.as_u16(),
        body: truncate(&body, 500),
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// File name from a `Content-Disposition` header, preferring the RFC 5987 `filename*` form.
fn content_disposition_filename(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').map(str::trim) {
        if let Some(v) = part.strip_prefix("filename*=") {
            let v = v.trim_matches('"');
            let v = v.rsplit("''").next().unwrap_or(v);
            return Some(percent_decode(v));
        }
        if let Some(v) = part.strip_prefix("filename=") {
            plain = Some(v.trim_matches('"').to_string());
        }
    }
    plain
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition() {
        assert_eq!(
            content_disposition_filename("attachment; filename=\"a b.txt\"").as_deref(),
            Some("a b.txt")
        );
        assert_eq!(
            content_disposition_filename("attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf").as_deref(),
            Some("résumé.pdf")
        );
        // filename* wins even when it comes second.
        assert_eq!(
            content_disposition_filename("attachment; filename=\"x.pdf\"; filename*=UTF-8''y%20z.pdf").as_deref(),
            Some("y z.pdf")
        );
        assert_eq!(content_disposition_filename("inline"), None);
    }

    #[test]
    fn percent_decode_edges() {
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%41%zz"), "A%zz");
    }

    #[test]
    fn request_body_shape() {
        let req = AskRequest::new("k", "q").attachments(["a"]);
        let b = serde_json::to_value(&req).unwrap();
        assert_eq!(b, json!({"question": "q", "api_key": "k", "attachments": ["a"]}));
        let mut req = AskRequest::new("k", "q").conversation_id("c");
        req.extra.insert("prompt_id".into(), json!("p"));
        let b = serde_json::to_value(&req).unwrap();
        assert_eq!(
            b,
            json!({"question": "q", "api_key": "k", "conversation_id": "c", "prompt_id": "p"})
        );
    }

    #[test]
    fn answer_tolerates_nulls() {
        let a: Answer = serde_json::from_value(
            json!({"answer": null, "conversation_id": "c", "sources": null, "tool_calls": null, "thought": null}),
        )
        .unwrap();
        assert_eq!(a.answer, "");
        assert!(a.sources().is_empty());
        let a: Answer =
            serde_json::from_value(json!({"answer": "x", "sources": [{"title": "T", "link": "https://t"}]})).unwrap();
        assert_eq!(
            a.sources(),
            vec![Source {
                title: "T".into(),
                url: Some("https://t".into())
            }]
        );
    }

    #[test]
    fn rejects_bad_base() {
        assert!(matches!(Client::new("localhost:7091"), Err(Error::Config(_))));
        assert_eq!(Client::new("http://h:1/").unwrap().base_url(), "http://h:1");
    }
}
