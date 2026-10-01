//! Stream one answer to the terminal.
//!
//! ```sh
//! DOCSGPT_KEY=<agent key> cargo run --example ask -- "What is DocsGPT?"
//! ```
//!
//! `DOCSGPT_BASE` picks the server (default `http://localhost:7091`) and
//! `DOCSGPT_CONVERSATION` continues an earlier conversation.

use std::io::Write;

use docsgpt::{AskRequest, Client, Event};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("DOCSGPT_BASE").unwrap_or_else(|_| "http://localhost:7091".into());
    let key = std::env::var("DOCSGPT_KEY").map_err(|_| "set DOCSGPT_KEY to an agent API key")?;
    let question = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if question.is_empty() {
        return Err("usage: ask <question>".into());
    }
    let mut req = AskRequest::new(key, question);
    if let Ok(conv) = std::env::var("DOCSGPT_CONVERSATION") {
        req = req.conversation_id(conv);
    }

    let client = Client::new(base)?;
    let mut events = client.stream(&req).await?;
    let mut out = std::io::stdout();
    let mut conversation = None;
    while let Some(ev) = events.next().await {
        match ev? {
            Event::Answer(delta) => {
                write!(out, "{delta}")?;
                out.flush()?;
            }
            Event::ToolCall(call) if !call.is_completed() => eprintln!("\n[{}…]", call.label()),
            Event::Source(sources) if !sources.is_empty() => {
                writeln!(out, "\n\nSources:")?;
                for s in sources {
                    writeln!(
                        out,
                        "  - {}{}",
                        s.title,
                        s.url.map(|u| format!(" <{u}>")).unwrap_or_default()
                    )?;
                }
            }
            Event::MessageId { conversation_id, .. } => conversation = conversation_id.or(conversation),
            Event::ConversationId(id) => conversation = Some(id),
            Event::Error(e) => eprintln!("\n[error] {e}"),
            _ => {}
        }
    }
    writeln!(out)?;
    if let Some(c) = conversation {
        eprintln!("conversation: {c}  (DOCSGPT_CONVERSATION={c} to continue)");
    }
    Ok(())
}
