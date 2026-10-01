//! An in-process fake DocsGPT server for tests (feature `mock`).
//!
//! [`MockDocsGpt::start`] binds a random local port on the current Tokio
//! runtime. Script its replies, point a [`crate::Client`] (or a bot) at
//! [`MockDocsGpt::url`], then assert on the requests in [`MockDocsGpt::rec`].
//!
//! ```no_run
//! # async fn run() {
//! use docsgpt::mock::{MockDocsGpt, reply_text};
//!
//! let docs = MockDocsGpt::start().await;
//! docs.on_stream(|_body| reply_text("Hello!", "conv-1"));
//! let client = docsgpt::Client::new(&docs.url).unwrap();
//! // … exercise the code under test …
//! assert_eq!(docs.rec.count("/stream"), 1);
//! # }
//! ```

#![allow(missing_docs)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequest, Multipart, Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use serde_json::{Value, json};

/// Default time [`Recorder::wait_any`] waits.
pub const WAIT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Recorder
// ---------------------------------------------------------------------------

/// A file part of a multipart request.
#[derive(Clone, Debug)]
pub struct UploadedFile {
    pub filename: String,
    pub content_type: Option<String>,
    pub bytes: Bytes,
}

/// One request seen by a mock.
#[derive(Clone, Debug)]
pub struct Call {
    /// Route, e.g. `/stream` or `/api/artifacts/download` (bots' own mocks use method names).
    pub method: String,
    /// JSON body, or the text fields of a multipart form (JSON-looking values parsed).
    pub body: Value,
    /// Query-string parameters.
    pub query: HashMap<String, String>,
    /// Multipart file parts by field name.
    pub files: HashMap<String, UploadedFile>,
    /// When the request arrived.
    pub at: Instant,
}

impl Call {
    pub fn new(method: &str, body: Value) -> Self {
        Self {
            method: method.to_string(),
            body,
            query: HashMap::new(),
            files: HashMap::new(),
            at: Instant::now(),
        }
    }

    /// A string field of the body, or `""`.
    pub fn str(&self, key: &str) -> String {
        self.body[key].as_str().unwrap_or("").to_string()
    }

    /// A multipart file by field name; panics with the available names when missing.
    pub fn file(&self, name: &str) -> &UploadedFile {
        self.files.get(name).unwrap_or_else(|| {
            panic!(
                "no multipart file {name:?} in {} call; file fields: {:?}",
                self.method,
                self.files.keys().collect::<Vec<_>>()
            )
        })
    }
}

/// Thread-safe log of the requests a mock received, with async waiting helpers.
#[derive(Default)]
pub struct Recorder {
    calls: Mutex<Vec<Call>>,
}

impl Recorder {
    pub fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    pub fn all(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub fn calls(&self, method: &str) -> Vec<Call> {
        self.all().into_iter().filter(|c| c.method == method).collect()
    }

    pub fn count(&self, method: &str) -> usize {
        self.calls(method).len()
    }

    pub fn last(&self, method: &str) -> Option<Call> {
        self.calls(method).pop()
    }

    /// One line per recorded call, for assertion messages.
    pub fn summary(&self) -> String {
        self.all()
            .iter()
            .map(|c| format!("  {} {}", c.method, truncate(&c.body.to_string(), 200)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Wait until a call to `method` matching `pred` is recorded and return it.
    pub async fn wait_for(&self, method: &str, pred: impl Fn(&Call) -> bool, timeout: Duration) -> Call {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(c) = self.calls(method).into_iter().find(|c| pred(c)) {
                return c;
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {timeout:?} waiting for {method}; recorded calls:\n{}",
                self.summary()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn wait_any(&self, method: &str) -> Call {
        self.wait_for(method, |_| true, WAIT).await
    }

    /// Wait until at least `n` calls to `method` exist and return all of them.
    pub async fn wait_count(&self, method: &str, n: usize, timeout: Duration) -> Vec<Call> {
        let deadline = Instant::now() + timeout;
        loop {
            let calls = self.calls(method);
            if calls.len() >= n {
                return calls;
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {timeout:?} waiting for {n} {method} calls (have {}); recorded calls:\n{}",
                calls.len(),
                self.summary()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Sleep for `within`, then assert `method` was never called.
    pub async fn assert_none(&self, method: &str, within: Duration) {
        tokio::time::sleep(within).await;
        let n = self.count(method);
        assert!(
            n == 0,
            "expected no {method} calls, got {n}; recorded calls:\n{}",
            self.summary()
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Parse multipart text fields that look like JSON (numbers, booleans, objects).
pub fn loose_json(s: &str) -> Value {
    let t = s.trim();
    let looks_json = t.starts_with('{')
        || t.starts_with('[')
        || t == "true"
        || t == "false"
        || t == "null"
        || t.parse::<i64>().is_ok();
    if looks_json {
        serde_json::from_str(t).unwrap_or_else(|_| Value::String(s.to_string()))
    } else {
        Value::String(s.to_string())
    }
}

/// Split a multipart body into its text fields (as JSON) and its files.
pub async fn read_multipart(mut mp: Multipart) -> (Value, HashMap<String, UploadedFile>) {
    let mut body = serde_json::Map::new();
    let mut files = HashMap::new();
    while let Some(field) = mp.next_field().await.expect("multipart field") {
        let name = field.name().unwrap_or("").to_string();
        let filename = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(str::to_string);
        let bytes = field.bytes().await.expect("multipart field bytes");
        match filename {
            Some(filename) => {
                files.insert(
                    name,
                    UploadedFile {
                        filename,
                        content_type,
                        bytes,
                    },
                );
            }
            None => {
                body.insert(name, loose_json(&String::from_utf8_lossy(&bytes)));
            }
        }
    }
    (Value::Object(body), files)
}

/// Read a JSON or multipart request body.
pub async fn parse_body(req: Request) -> (Value, HashMap<String, UploadedFile>) {
    let ct = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.starts_with("multipart/form-data") {
        let mp = Multipart::from_request(req, &()).await.expect("multipart request");
        read_multipart(mp).await
    } else {
        let bytes = axum::body::to_bytes(req.into_body(), 64 << 20)
            .await
            .unwrap_or_default();
        (serde_json::from_slice(&bytes).unwrap_or(Value::Null), HashMap::new())
    }
}

// ---------------------------------------------------------------------------
// Scripted /stream replies
// ---------------------------------------------------------------------------

/// One piece of a scripted `/stream` response.
#[derive(Debug, Clone)]
pub enum Step {
    /// `data: <json>\n\n`
    Event(Value),
    /// Raw text, e.g. `": keepalive\n\n"` or half an event.
    Raw(String),
    /// Stall the stream; a keepalive comment is sent afterwards.
    Sleep(Duration),
    /// Stall without sending anything afterwards (to trip idle timeouts).
    Hang(Duration),
}

/// What `/stream` replies with.
#[derive(Debug, Clone)]
pub enum StreamReply {
    /// An event stream.
    Sse(Vec<Step>),
    /// A plain HTTP status and body instead of a stream.
    Http(u16, String),
}

pub fn sse(steps: Vec<Step>) -> StreamReply {
    StreamReply::Sse(steps)
}

/// Builders for the JSON of individual stream events.
pub mod ev {
    use serde_json::{Value, json};

    use super::Step;

    /// Wrap an event as a [`Step`].
    pub fn step(v: Value) -> Step {
        Step::Event(v)
    }
    pub fn message_id(message_id: &str, conversation_id: &str) -> Value {
        json!({"type": "message_id", "message_id": message_id, "conversation_id": conversation_id, "request_id": "req-1"})
    }
    pub fn answer(delta: &str) -> Value {
        json!({"type": "answer", "answer": delta})
    }
    pub fn thought(t: &str) -> Value {
        json!({"type": "thought", "thought": t})
    }
    pub fn source(list: &[(&str, &str)]) -> Value {
        json!({"type": "source", "source": list.iter().map(|(title, link)| json!({"title": title, "link": link})).collect::<Vec<_>>()})
    }
    pub fn tool_call(data: Value) -> Value {
        json!({"type": "tool_call", "data": data})
    }
    pub fn id(conversation_id: &str) -> Value {
        json!({"type": "id", "id": conversation_id})
    }
    pub fn end() -> Value {
        json!({"type": "end"})
    }
    pub fn error(message: &str) -> Value {
        json!({"type": "error", "error": message})
    }
}

/// A realistic answer: message_id, word-sized deltas, optional sources, a
/// keepalive comment, the conversation id, end.
pub fn answer_steps(text: &str, conversation_id: &str, sources: &[(&str, &str)]) -> Vec<Step> {
    let mut steps = vec![ev::step(ev::message_id("m1", conversation_id))];
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if ch == ' ' || ch == '\n' {
            steps.push(ev::step(ev::answer(&cur)));
            cur.clear();
        }
    }
    if !cur.is_empty() {
        steps.push(ev::step(ev::answer(&cur)));
    }
    if !sources.is_empty() {
        steps.push(ev::step(ev::source(sources)));
    }
    steps.push(Step::Raw(": keepalive\n\n".into()));
    steps.push(ev::step(ev::id(conversation_id)));
    steps.push(ev::step(ev::end()));
    steps
}

/// [`answer_steps`] without sources, as a reply.
pub fn reply_text(text: &str, conversation_id: &str) -> StreamReply {
    sse(answer_steps(text, conversation_id, &[]))
}

fn sse_response(steps: Vec<Step>) -> Response {
    let stream = futures_util::stream::unfold(steps.into_iter(), |mut it| async move {
        let chunk = loop {
            match it.next()? {
                Step::Event(v) => break format!("data: {v}\n\n"),
                Step::Raw(s) => break s,
                Step::Sleep(d) => {
                    tokio::time::sleep(d).await;
                    break ": keepalive\n\n".to_string();
                }
                Step::Hang(d) => tokio::time::sleep(d).await,
            }
        };
        Some((Ok::<Bytes, std::convert::Infallible>(Bytes::from(chunk)), it))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

type StreamFn = Box<dyn FnMut(&Value) -> StreamReply + Send + 'static>;
type AnswerFn = Box<dyn FnMut(&Value) -> (u16, Value) + Send + 'static>;

struct Artifact {
    filename: String,
    mime: String,
    bytes: Bytes,
}

/// The fake DocsGPT. Routes: `/stream`, `/api/answer`, `/api/store_attachment`,
/// `/api/task_status`, `/api/stt`, `/api/tts`, `/api/feedback`,
/// `/api/artifacts/{id}/download` (recorded as `/api/artifacts/download`), and
/// `/images/{name}` for public image URLs.
pub struct MockDocsGpt {
    /// Base URL to point clients at.
    pub url: String,
    /// Every request received, by route.
    pub rec: Recorder,
    stream: Mutex<StreamFn>,
    answer: Mutex<AnswerFn>,
    store: Mutex<(u16, Value)>,
    task_status: Mutex<VecDeque<String>>,
    stt: Mutex<(u16, Value)>,
    tts: Mutex<(u16, Value)>,
    feedback: Mutex<(u16, Value)>,
    artifacts: Mutex<HashMap<String, Artifact>>,
    images: Mutex<HashMap<String, Bytes>>,
    faults: Mutex<HashMap<String, VecDeque<u16>>>,
}

impl MockDocsGpt {
    /// Start on a random local port, on the current Tokio runtime.
    pub async fn start() -> Arc<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock docsgpt");
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let m = Arc::new(Self {
            url,
            rec: Recorder::default(),
            stream: Mutex::new(Box::new(|_: &Value| {
                reply_text("Hello from the mock assistant.", "conv-1")
            })),
            answer: Mutex::new(Box::new(|_: &Value| {
                (200, json!({"answer": "X is y", "conversation_id": "c"}))
            })),
            store: Mutex::new((
                200,
                json!({"success": true, "attachment_id": "att-1", "task_id": "task-1"}),
            )),
            task_status: Mutex::new(VecDeque::new()),
            stt: Mutex::new((200, json!({"success": true, "text": "transcribed speech"}))),
            // base64 of "ID3"
            tts: Mutex::new((200, json!({"success": true, "audio_base64": "SUQz", "lang": "en"}))),
            feedback: Mutex::new((200, json!({"success": true}))),
            artifacts: Mutex::new(HashMap::new()),
            images: Mutex::new(HashMap::new()),
            faults: Mutex::new(HashMap::new()),
        });
        let app = Router::new()
            .route("/stream", post(stream))
            .route("/api/answer", post(answer))
            .route("/api/store_attachment", post(store))
            .route("/api/task_status", get(task))
            .route("/api/stt", post(stt))
            .route("/api/tts", post(tts))
            .route("/api/feedback", post(feedback))
            .route("/api/artifacts/{id}/download", get(artifact))
            .route("/images/{name}", get(image))
            .with_state(m.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock docsgpt server");
        });
        m
    }

    /// Script `/stream`: the closure sees the request body and returns the reply.
    pub fn on_stream(&self, f: impl FnMut(&Value) -> StreamReply + Send + 'static) {
        *self.stream.lock().unwrap() = Box::new(f);
    }

    /// Script `/api/answer`: the closure returns a status and JSON body.
    pub fn on_answer(&self, f: impl FnMut(&Value) -> (u16, Value) + Send + 'static) {
        *self.answer.lock().unwrap() = Box::new(f);
    }

    pub fn set_store_attachment(&self, status: u16, body: Value) {
        *self.store.lock().unwrap() = (status, body);
    }

    /// Statuses for successive `/api/task_status` calls; `SUCCESS` once exhausted.
    pub fn queue_task_status(&self, statuses: &[&str]) {
        self.task_status
            .lock()
            .unwrap()
            .extend(statuses.iter().map(|s| s.to_string()));
    }

    pub fn set_stt(&self, text: &str) {
        self.set_stt_reply(200, json!({"success": true, "text": text}));
    }

    pub fn set_stt_reply(&self, status: u16, body: Value) {
        *self.stt.lock().unwrap() = (status, body);
    }

    pub fn set_tts_reply(&self, status: u16, body: Value) {
        *self.tts.lock().unwrap() = (status, body);
    }

    pub fn set_feedback_reply(&self, status: u16, body: Value) {
        *self.feedback.lock().unwrap() = (status, body);
    }

    pub fn add_artifact(&self, id: &str, filename: &str, mime: &str, bytes: impl Into<Bytes>) {
        self.artifacts.lock().unwrap().insert(
            id.to_string(),
            Artifact {
                filename: filename.to_string(),
                mime: mime.to_string(),
                bytes: bytes.into(),
            },
        );
    }

    pub fn add_image(&self, name: &str, bytes: impl Into<Bytes>) {
        self.images.lock().unwrap().insert(name.to_string(), bytes.into());
    }

    /// Public URL of an image added with [`Self::add_image`].
    pub fn image_url(&self, name: &str) -> String {
        format!("{}/images/{name}", self.url)
    }

    /// Make the next `times` requests to `route` (as recorded, e.g. `/stream`)
    /// fail with `status` before reaching the scripted reply. Failed requests are
    /// recorded too.
    pub fn fail_next(&self, route: &str, status: u16, times: usize) {
        self.faults
            .lock()
            .unwrap()
            .entry(route.to_string())
            .or_default()
            .extend(std::iter::repeat_n(status, times));
    }

    fn fault(&self, route: &str) -> Option<Response> {
        let status = self.faults.lock().unwrap().get_mut(route)?.pop_front()?;
        Some(reply(status, json!({"error": "injected fault"})))
    }
}

fn reply(status: u16, body: Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(body),
    )
        .into_response()
}

async fn stream(State(m): State<Arc<MockDocsGpt>>, req: Request) -> Response {
    let (body, _) = parse_body(req).await;
    m.rec.record(Call::new("/stream", body.clone()));
    if let Some(r) = m.fault("/stream") {
        return r;
    }
    let reply = (m.stream.lock().unwrap())(&body);
    match reply {
        StreamReply::Http(code, text) => (
            StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            text,
        )
            .into_response(),
        StreamReply::Sse(steps) => sse_response(steps),
    }
}

async fn answer(State(m): State<Arc<MockDocsGpt>>, req: Request) -> Response {
    let (body, _) = parse_body(req).await;
    m.rec.record(Call::new("/api/answer", body.clone()));
    if let Some(r) = m.fault("/api/answer") {
        return r;
    }
    let (code, v) = (m.answer.lock().unwrap())(&body);
    reply(code, v)
}

async fn store(State(m): State<Arc<MockDocsGpt>>, mp: Multipart) -> Response {
    let (body, files) = read_multipart(mp).await;
    let mut call = Call::new("/api/store_attachment", body);
    call.files = files;
    m.rec.record(call);
    if let Some(r) = m.fault("/api/store_attachment") {
        return r;
    }
    let (code, v) = m.store.lock().unwrap().clone();
    reply(code, v)
}

async fn task(State(m): State<Arc<MockDocsGpt>>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut call = Call::new("/api/task_status", Value::Null);
    call.query = q;
    m.rec.record(call);
    if let Some(r) = m.fault("/api/task_status") {
        return r;
    }
    let status = m
        .task_status
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| "SUCCESS".to_string());
    axum::Json(json!({"status": status})).into_response()
}

async fn stt(State(m): State<Arc<MockDocsGpt>>, Query(q): Query<HashMap<String, String>>, mp: Multipart) -> Response {
    let (body, files) = read_multipart(mp).await;
    let mut call = Call::new("/api/stt", body);
    call.query = q;
    call.files = files;
    m.rec.record(call);
    if let Some(r) = m.fault("/api/stt") {
        return r;
    }
    let (code, v) = m.stt.lock().unwrap().clone();
    reply(code, v)
}

async fn tts(State(m): State<Arc<MockDocsGpt>>, req: Request) -> Response {
    let (body, _) = parse_body(req).await;
    m.rec.record(Call::new("/api/tts", body));
    if let Some(r) = m.fault("/api/tts") {
        return r;
    }
    let (code, v) = m.tts.lock().unwrap().clone();
    reply(code, v)
}

async fn feedback(State(m): State<Arc<MockDocsGpt>>, req: Request) -> Response {
    let (body, _) = parse_body(req).await;
    m.rec.record(Call::new("/api/feedback", body));
    if let Some(r) = m.fault("/api/feedback") {
        return r;
    }
    let (code, v) = m.feedback.lock().unwrap().clone();
    reply(code, v)
}

async fn artifact(
    State(m): State<Arc<MockDocsGpt>>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let mut call = Call::new("/api/artifacts/download", json!({"id": id}));
    call.query = q;
    m.rec.record(call);
    if let Some(r) = m.fault("/api/artifacts/download") {
        return r;
    }
    let found = m
        .artifacts
        .lock()
        .unwrap()
        .get(&id)
        .map(|a| (a.filename.clone(), a.mime.clone(), a.bytes.clone()));
    match found {
        Some((filename, mime, bytes)) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            )
            .body(Body::from(bytes))
            .unwrap(),
        None => (StatusCode::NOT_FOUND, "no such artifact").into_response(),
    }
}

async fn image(State(m): State<Arc<MockDocsGpt>>, Path(name): Path<String>) -> Response {
    m.rec.record(Call::new("/images", json!({"name": name})));
    if let Some(r) = m.fault("/images") {
        return r;
    }
    let found = m.images.lock().unwrap().get(&name).cloned();
    match found {
        Some(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/png")
            .body(Body::from(bytes))
            .unwrap(),
        None => (StatusCode::NOT_FOUND, "no such image").into_response(),
    }
}
