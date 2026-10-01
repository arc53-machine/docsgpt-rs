//! Events emitted by the `/stream` endpoint, and the tool results inside them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::repr::python_repr_to_json;

/// One server-sent event from `/stream`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// First event of a turn: the ids the server reserved for it.
    MessageId {
        /// Id of the message row reserved for this answer.
        message_id: String,
        /// Conversation the turn belongs to (absent on servers that assign it later).
        conversation_id: Option<String>,
        /// Server-side request id, useful when reporting problems.
        request_id: Option<String>,
    },
    /// A delta of answer text.
    Answer(String),
    /// A delta of reasoning text, from models with visible thinking.
    Thought(String),
    /// Sources retrieved for the answer (sent once, before the end).
    Source(Vec<Source>),
    /// Incremental state of one tool call (`pending` → `completed`).
    ToolCall(ToolCall),
    /// Summary of every tool call made during the turn.
    ToolCalls(Vec<ToolCall>),
    /// Conversation id to pass on the next turn.
    ConversationId(String),
    /// The full structured (JSON-schema) answer, for agents configured to return one.
    StructuredAnswer(String),
    /// An informational notice from the server.
    Notice(String),
    /// The server failed while answering.
    Error(String),
    /// The stream is complete.
    End,
    /// An event type this crate does not model yet (guardrail, workflow_run, …),
    /// with its raw JSON.
    Other(String, Value),
}

/// A document or page the answer drew on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Source {
    /// Display title; `"Source"` when the server sent none.
    pub title: String,
    /// Link to the source, when it has an `http(s)` one.
    #[serde(default)]
    pub url: Option<String>,
}

/// A file a tool produced, downloadable with [`crate::Client::download_artifact`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ArtifactRef {
    /// Artifact id.
    pub id: String,
    /// File name to show and save as (the id when the server sent none).
    pub filename: String,
    /// MIME type, when known.
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// One tool call made by the agent.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct ToolCall {
    /// Tool name, e.g. `code_executor`.
    #[serde(default)]
    pub tool_name: String,
    /// Id of this call, stable across its `pending` and `completed` events.
    #[serde(default)]
    pub call_id: String,
    /// Action within the tool, e.g. `run_code`.
    #[serde(default)]
    pub action_name: String,
    /// Arguments the model passed.
    #[serde(default)]
    pub arguments: Value,
    /// Tool result: a Python `repr` or JSON text.
    #[serde(default)]
    pub result: Option<String>,
    /// `pending`, `completed`, `error`, …
    #[serde(default)]
    pub status: String,
    /// Single artifact id, on servers that report only one.
    #[serde(default)]
    pub artifact_id: Option<String>,
    /// Artifacts listed explicitly by the server.
    #[serde(default)]
    pub artifacts: Vec<RawArtifact>,
}

/// An artifact entry as it appears in a tool call's `artifacts` list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct RawArtifact {
    /// Artifact id (`id` or `artifact_id`).
    #[serde(default, alias = "artifact_id")]
    pub id: String,
    /// File name.
    #[serde(default)]
    pub filename: String,
    /// MIME type.
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// Files and images a tool produced that a client should show the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutputs {
    /// Files to download with [`crate::Client::download_artifact`].
    pub artifacts: Vec<ArtifactRef>,
    /// Public image URLs (e.g. from image generation).
    pub image_urls: Vec<String>,
}

impl ToolOutputs {
    /// Add `other`'s artifacts and images, skipping ones already present.
    pub fn merge(&mut self, other: ToolOutputs) {
        for a in other.artifacts {
            if !self.artifacts.iter().any(|x| x.id == a.id) {
                self.artifacts.push(a);
            }
        }
        for u in other.image_urls {
            if !self.image_urls.contains(&u) {
                self.image_urls.push(u);
            }
        }
    }

    /// True when there is nothing to deliver.
    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty() && self.image_urls.is_empty()
    }

    fn add_artifact(&mut self, id: &str, filename: Option<&str>, mime_type: Option<&str>) {
        if id.is_empty() {
            return;
        }
        match self.artifacts.iter_mut().find(|x| x.id == id) {
            Some(existing) => {
                if existing.mime_type.is_none() {
                    existing.mime_type = mime_type.map(str::to_string);
                }
            }
            None => self.artifacts.push(ArtifactRef {
                id: id.to_string(),
                filename: filename.filter(|f| !f.is_empty()).unwrap_or(id).to_string(),
                mime_type: mime_type.map(str::to_string),
            }),
        }
    }

    fn add_image(&mut self, url: &str) {
        if url.starts_with("http") && !self.image_urls.iter().any(|x| x == url) {
            self.image_urls.push(url.to_string());
        }
    }
}

impl ToolCall {
    /// True once the call has finished.
    pub fn is_completed(&self) -> bool {
        self.status == "completed"
    }

    /// Short label for progress indicators ("Running code", "Generating image").
    pub fn label(&self) -> String {
        match (self.tool_name.as_str(), self.action_name.as_str()) {
            ("code_executor", _) => "Running code".into(),
            ("imagegen", _) | (_, "imagegen_generate") => "Generating image".into(),
            (t, a) if !a.is_empty() => format!("Using {} ({})", pretty(t), pretty(a)),
            (t, _) => format!("Using {}", pretty(t)),
        }
    }

    /// Artifacts and image URLs from the explicit fields and from the result payload.
    pub fn outputs(&self) -> ToolOutputs {
        let mut out = ToolOutputs::default();
        for a in &self.artifacts {
            out.add_artifact(&a.id, Some(&a.filename), a.mime_type.as_deref());
        }
        if let Some(v) = self.result.as_deref().and_then(python_repr_to_json) {
            for a in v.get("artifacts").and_then(Value::as_array).into_iter().flatten() {
                let id = a
                    .get("artifact_id")
                    .or_else(|| a.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                out.add_artifact(
                    id,
                    a.get("filename").and_then(Value::as_str),
                    a.get("mime_type").and_then(Value::as_str),
                );
            }
            for key in ["image_urls", "urls"] {
                for u in v.get(key).and_then(Value::as_array).into_iter().flatten() {
                    if let Some(u) = u.as_str() {
                        out.add_image(u);
                    }
                }
            }
            if let Some(u) = v.get("image_url").and_then(Value::as_str) {
                out.add_image(u);
            }
        }
        if let Some(id) = &self.artifact_id {
            out.add_artifact(id, None, None);
        }
        out
    }
}

fn pretty(s: &str) -> String {
    s.replace('_', " ")
}

impl Event {
    /// Parse the JSON payload of one `data:` line. Returns `None` for non-JSON payloads.
    pub fn parse(data: &str) -> Option<Event> {
        let v: Value = serde_json::from_str(data).ok()?;
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("").to_string();
        Some(match ty.as_str() {
            "answer" => Event::Answer(str_field(&v, "answer")),
            "thought" => Event::Thought(str_field(&v, "thought")),
            "message_id" => Event::MessageId {
                message_id: str_field(&v, "message_id"),
                conversation_id: opt_str(&v, "conversation_id"),
                request_id: opt_str(&v, "request_id"),
            },
            "id" => Event::ConversationId(str_field(&v, "id")),
            "end" => Event::End,
            "error" => Event::Error(match v.get("error") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "unknown error".into(),
            }),
            "notice" => Event::Notice(str_field(&v, "notice")),
            "structured_answer" => Event::StructuredAnswer(match v.get("answer") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            }),
            "source" | "sources" => {
                let list = v.get("source").or_else(|| v.get("sources")).and_then(Value::as_array);
                Event::Source(list.map(|l| l.iter().map(parse_source).collect()).unwrap_or_default())
            }
            "tool_call" => {
                let data = v.get("data").cloned().unwrap_or(Value::Null);
                Event::ToolCall(serde_json::from_value(data).unwrap_or_default())
            }
            "tool_calls" => {
                let list = v.get("tool_calls").cloned().unwrap_or(Value::Array(vec![]));
                Event::ToolCalls(serde_json::from_value(list).unwrap_or_default())
            }
            _ => Event::Other(ty, v),
        })
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn parse_source(v: &Value) -> Source {
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| v.get("source").and_then(Value::as_str))
        .unwrap_or("Source")
        .to_string();
    let url = ["link", "url", "source"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_str))
        .find(|s| s.starts_with("http://") || s.starts_with("https://"))
        .map(str::to_string);
    Source { title, url }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_core_events() {
        assert_eq!(
            Event::parse(r#"{"type": "answer", "answer": "Hi"}"#),
            Some(Event::Answer("Hi".into()))
        );
        assert_eq!(Event::parse(r#"{"type": "end"}"#), Some(Event::End));
        assert_eq!(
            Event::parse(r#"{"type": "id", "id": "c1"}"#),
            Some(Event::ConversationId("c1".into()))
        );
        assert_eq!(
            Event::parse(r#"{"type": "message_id", "message_id": "m", "conversation_id": "c", "request_id": "r"}"#),
            Some(Event::MessageId {
                message_id: "m".into(),
                conversation_id: Some("c".into()),
                request_id: Some("r".into()),
            })
        );
        assert_eq!(
            Event::parse(r#"{"type": "error", "error": {"code": 1}}"#),
            Some(Event::Error(r#"{"code":1}"#.into()))
        );
        assert_eq!(Event::parse("not json"), None);
    }

    #[test]
    fn parses_sources() {
        match Event::parse(
            r#"{"type": "source", "source": [{"title": "Doc", "link": "https://x"}, {"title": ""}, {"title": " ", "source": "https://y/z"}]}"#,
        ) {
            Some(Event::Source(s)) => {
                assert_eq!(
                    s[0],
                    Source {
                        title: "Doc".into(),
                        url: Some("https://x".into())
                    }
                );
                assert_eq!(
                    s[1],
                    Source {
                        title: "Source".into(),
                        url: None
                    }
                );
                assert_eq!(
                    s[2],
                    Source {
                        title: "https://y/z".into(),
                        url: Some("https://y/z".into())
                    }
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn extracts_artifacts_from_tool_call() {
        let raw = r#"{"type": "tool_call", "data": {"tool_name": "code_executor", "call_id": "c", "action_name": "run_code", "arguments": {"code": "x"}, "artifact_id": "dfb9", "artifacts": [{"id": "dfb9", "filename": "hello.txt", "ref": "A1"}], "result": "{'status': 'ok', 'stdout_tail': '', 'artifacts': [{'artifact_id': 'dfb9', 'version': 1, 'filename': 'hello.txt', 'mime_type': 'text/plain', 'size': 18, 'ref': 'A1'}]}", "status": "completed"}}"#;
        let Some(Event::ToolCall(tc)) = Event::parse(raw) else {
            panic!()
        };
        assert!(tc.is_completed());
        let out = tc.outputs();
        assert_eq!(out.artifacts.len(), 1);
        assert_eq!(out.artifacts[0].filename, "hello.txt");
        assert_eq!(out.artifacts[0].mime_type.as_deref(), Some("text/plain"));
        assert_eq!(tc.label(), "Running code");
    }

    #[test]
    fn extracts_image_urls() {
        let raw = r#"{"type": "tool_call", "data": {"tool_name": "imagegen", "call_id": "c", "action_name": "imagegen_generate", "arguments": {"prompt": "cat"}, "result": "{'status_code': 200, 'image_urls': ['https://artefacts.docsgpt.cloud/images/a.png', 'ftp://no'], 'image_url': 'https://artefacts.docsgpt.cloud/images/a.png', 'message': 'respond like this: ![image]({image_url})'}", "status": "completed"}}"#;
        let Some(Event::ToolCall(tc)) = Event::parse(raw) else {
            panic!()
        };
        assert_eq!(
            tc.outputs().image_urls,
            vec!["https://artefacts.docsgpt.cloud/images/a.png".to_string()]
        );
        assert_eq!(tc.label(), "Generating image");
    }

    #[test]
    fn artifact_id_only_uses_id_as_filename() {
        let tc = ToolCall {
            artifact_id: Some("a1".into()),
            tool_name: "x_tool".into(),
            ..Default::default()
        };
        assert_eq!(
            tc.outputs().artifacts,
            vec![ArtifactRef {
                id: "a1".into(),
                filename: "a1".into(),
                mime_type: None
            }]
        );
        assert_eq!(tc.label(), "Using x tool");
    }

    #[test]
    fn merge_dedupes() {
        let mut a = ToolOutputs {
            artifacts: vec![],
            image_urls: vec!["https://i".into()],
        };
        a.merge(ToolOutputs {
            artifacts: vec![ArtifactRef {
                id: "1".into(),
                filename: "f".into(),
                mime_type: None,
            }],
            image_urls: vec!["https://i".into()],
        });
        assert_eq!(a.image_urls.len(), 1);
        assert_eq!(a.artifacts.len(), 1);
        assert!(!a.is_empty());
    }

    #[test]
    fn unknown_event_is_other() {
        match Event::parse(r#"{"type": "guardrail", "guardrail": {"x": 1}}"#) {
            Some(Event::Other(t, v)) => {
                assert_eq!(t, "guardrail");
                assert_eq!(v["guardrail"]["x"], 1);
            }
            other => panic!("{other:?}"),
        }
    }
}
