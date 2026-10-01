# docsgpt

Rust client for the [DocsGPT](https://www.docsgpt.cloud/) API.

- Stream an agent's answer as typed events, or wait for the whole reply.
- Upload attachments, transcribe speech and synthesize it.
- Download files that the agent's tools produced (code execution, document and image generation).
- Rate answers with 👍 or 👎.
- Retries requests that are safe to repeat, returns typed errors and fails a stream that goes silent.
- A `mock` feature with an in-process fake DocsGPT for your own tests.

```toml
[dependencies]
docsgpt = "0.1"
```

```rust,no_run
use docsgpt::{AskRequest, Client, Event};
use futures_util::StreamExt;

# async fn run() -> docsgpt::Result<()> {
let client = Client::new("https://gptcloud.arc53.com")?;
let req = AskRequest::new("<agent api key>", "How do I add a data source?");
let mut events = client.stream(&req).await?;
while let Some(event) = events.next().await {
    match event? {
        Event::Answer(delta) => print!("{delta}"),
        Event::Source(sources) => println!("\n{} sources", sources.len()),
        Event::ConversationId(id) => println!("\ncontinue with conversation {id}"),
        _ => {}
    }
}
# Ok(()) }
```

Agent API keys are under **Agents → your agent → API key** in DocsGPT.

## Endpoints

| Method | Endpoint |
|---|---|
| `Client::stream` | `POST /stream` |
| `Client::answer` | `POST /api/answer` |
| `Client::upload_attachment`, `Client::wait_for_task` | `POST /api/store_attachment`, `GET /api/task_status` |
| `Client::stt`, `Client::tts` | `POST /api/stt`, `POST /api/tts` |
| `Client::download_artifact` | `GET /api/artifacts/{id}/download` |
| `Client::feedback` | `POST /api/feedback` |

`Client::feedback` takes the answer's `question_index`: its 0-based position in the conversation (the first answer is 0).

## Testing with the mock

```toml
[dev-dependencies]
docsgpt = { version = "0.1", features = ["mock"] }
```

```rust,ignore
use docsgpt::mock::{MockDocsGpt, reply_text};

let docs = MockDocsGpt::start().await;
docs.on_stream(|_body| reply_text("Hello!", "conv-1"));
docs.fail_next("/stream", 503, 1); // exercise retries
// point the code under test at docs.url, then:
assert_eq!(docs.rec.count("/stream"), 2);
```

## Minimum Rust version

1.88.

## License

MIT
