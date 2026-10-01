//! `run_turn` against a real DocsGPT. Ignored by default:
//!
//! ```text
//! DOCSGPT_LIVE_KEY=<agent api key> DOCSGPT_LIVE_BASE=http://localhost:7091 \
//!   cargo test -p docsgpt-bot --all-features --test live -- --ignored --nocapture
//! ```
//!
//! Passes with any model, including `docsgpt dev --mock-llm`.

use std::sync::Arc;

use docsgpt::Feedback;
use docsgpt_bot::storage::memory::MemoryStorage;
use docsgpt_bot::testing::FakeSurface;
use docsgpt_bot::{AgentConfig, Agents, Ask, BotCore, Outcome, Scope, TurnReport, run_turn, submit_feedback};

#[tokio::test]
#[ignore]
async fn two_turns_and_feedback() {
    let Some(key) = std::env::var("DOCSGPT_LIVE_KEY").ok().filter(|k| !k.trim().is_empty()) else {
        return;
    };
    let base = std::env::var("DOCSGPT_LIVE_BASE").unwrap_or_else(|_| "https://gptcloud.arc53.com".into());
    let core = BotCore::new(
        "live",
        docsgpt::Client::new(base).unwrap(),
        Agents::new(vec![AgentConfig::new("default", key)]).unwrap(),
        Arc::new(MemoryStorage::default()),
    );
    let scope = Scope::new("live", "chat", "thread");
    let mut conversation = None;
    for (i, q) in ["first question", "second question"].into_iter().enumerate() {
        let surface = FakeSurface::new().message_id(Some(&format!("msg-{i}")));
        match run_turn(&core, &surface, Ask::new(scope.clone(), q)).await.unwrap() {
            TurnReport::Answered {
                outcome,
                conversation_id,
                position,
                ..
            } => {
                assert_eq!(outcome, Outcome::Complete, "{:?}", surface.finished());
                assert_eq!(position, Some(i as u32));
                if i == 1 {
                    assert_eq!(conversation_id, conversation, "the conversation continues");
                }
                conversation = conversation_id;
            }
            other => panic!("{other:?}"),
        }
        println!(
            "turn {i}: {} chars, {} updates",
            surface.finished().answer.len(),
            surface.updates().len()
        );
    }
    assert!(submit_feedback(&core, "msg-1", Feedback::Like).await.unwrap());
    println!("liked turn 1 of {}", conversation.unwrap());
}
