//! Checks against a real DocsGPT. Ignored by default:
//!
//! ```text
//! DOCSGPT_LIVE_KEY=<agent api key> cargo test -p docsgpt --test live -- --ignored --nocapture
//! ```
//!
//! `DOCSGPT_LIVE_BASE` picks the server (default `https://gptcloud.arc53.com`).
//! The agent needs a real model for the content checks, and the code-execution
//! tool for `artifact_download`.

use std::time::Duration;

use docsgpt::{AskRequest, Client, Event, Feedback, ToolOutputs, Upload};
use futures_util::StreamExt;

fn live() -> Option<(Client, String)> {
    let key = std::env::var("DOCSGPT_LIVE_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())?;
    let base = std::env::var("DOCSGPT_LIVE_BASE").unwrap_or_else(|_| "https://gptcloud.arc53.com".into());
    Some((Client::new(base).expect("client"), key))
}

#[derive(Default)]
struct Turn {
    answer: String,
    conversation_id: Option<String>,
    outputs: ToolOutputs,
    error: Option<String>,
}

async fn turn(client: &Client, req: &AskRequest) -> Turn {
    let mut stream = client.stream(req).await.expect("open stream");
    let mut t = Turn::default();
    while let Some(ev) = stream.next().await {
        match ev.expect("stream item") {
            Event::Answer(d) => t.answer.push_str(&d),
            Event::MessageId {
                conversation_id: Some(c),
                ..
            }
            | Event::ConversationId(c) => t.conversation_id = Some(c),
            Event::ToolCall(tc) if tc.is_completed() => t.outputs.merge(tc.outputs()),
            Event::ToolCalls(list) => list.into_iter().for_each(|tc| t.outputs.merge(tc.outputs())),
            Event::Error(e) => t.error = Some(e),
            Event::End => break,
            _ => {}
        }
    }
    t
}

/// Protocol checks that hold with any model, including `docsgpt dev --mock-llm`.
#[tokio::test]
#[ignore]
async fn protocol_smoke() {
    let Some((client, key)) = live() else { return };
    let t1 = turn(&client, &AskRequest::new(&key, "first question")).await;
    assert!(t1.error.is_none(), "{:?}", t1.error);
    assert!(!t1.answer.is_empty());
    let conv = t1.conversation_id.expect("conversation id");
    let t2 = turn(
        &client,
        &AskRequest::new(&key, "second question").conversation_id(&conv),
    )
    .await;
    assert_eq!(
        t2.conversation_id.as_deref(),
        Some(conv.as_str()),
        "the conversation continues"
    );

    // Answers are numbered from 0 in the order they were asked.
    client
        .feedback(&key, &conv, 0, Feedback::Like)
        .await
        .expect("feedback on turn 0");
    client
        .feedback(&key, &conv, 1, Feedback::Dislike)
        .await
        .expect("feedback on turn 1");
    println!("conversation {conv}: liked turn 0, disliked turn 1");

    let a = client
        .answer(&AskRequest::new(&key, "non-streaming"))
        .await
        .expect("answer");
    assert!(!a.answer.is_empty() && a.conversation_id.is_some());

    let att = client
        .upload_attachment(&key, &Upload::new("memo.txt", "hello").mime("text/plain"))
        .await
        .expect("upload");
    if let Some(task) = &att.task_id {
        let waited = client.wait_for_task(task, Duration::from_secs(60)).await.expect("task");
        println!("attachment {} task {task}: {waited:?}", att.id);
    }
    let t3 = turn(&client, &AskRequest::new(&key, "about the file").attachments([att.id])).await;
    assert!(t3.error.is_none(), "{:?}", t3.error);

    match client.tts(&key, "hello").await {
        Ok(audio) => println!("tts: {} bytes", audio.len()),
        Err(e) => println!("tts: {e}"),
    }
}

#[tokio::test]
#[ignore]
async fn stream_multi_turn_and_feedback() {
    let Some((client, key)) = live() else { return };
    let t1 = turn(
        &client,
        &AskRequest::new(&key, "My favourite colour is teal. Reply with just OK."),
    )
    .await;
    assert!(t1.error.is_none(), "{:?}", t1.error);
    let conv = t1.conversation_id.expect("conversation id");
    let t2 = turn(
        &client,
        &AskRequest::new(&key, "What is my favourite colour? One word.").conversation_id(&conv),
    )
    .await;
    println!("turn 2: {}", t2.answer);
    assert!(t2.answer.to_lowercase().contains("teal"));

    client.feedback(&key, &conv, 1, Feedback::Like).await.expect("feedback");
    client
        .feedback(&key, &conv, 1, Feedback::Clear)
        .await
        .expect("clear feedback");
}

#[tokio::test]
#[ignore]
async fn non_streaming_answer() {
    let Some((client, key)) = live() else { return };
    let a = client
        .answer(&AskRequest::new(&key, "Reply with just the word pong."))
        .await
        .expect("answer");
    println!("answer: {}", a.answer);
    assert!(a.answer.to_lowercase().contains("pong"));
    assert!(a.conversation_id.is_some());
}

#[tokio::test]
#[ignore]
async fn attachment_roundtrip() {
    let Some((client, key)) = live() else { return };
    let memo = Upload::new(
        "memo.txt",
        "Internal memo.\nThe secret launch code is 4471.\nDo not share.\n",
    )
    .mime("text/plain");
    let att = client.upload_attachment(&key, &memo).await.expect("upload");
    if let Some(task) = &att.task_id {
        let waited = client
            .wait_for_task(task, Duration::from_secs(120))
            .await
            .expect("task");
        println!("task {task}: {waited:?}");
    }
    let t = turn(
        &client,
        &AskRequest::new(
            &key,
            "What is the secret launch code in the attached memo? Reply with just the number.",
        )
        .attachments([att.id]),
    )
    .await;
    println!("answer: {}", t.answer);
    assert!(
        t.answer.contains("4471"),
        "answer did not use the attachment: {}",
        t.answer
    );
}

#[tokio::test]
#[ignore]
async fn artifact_download() {
    let Some((client, key)) = live() else { return };
    let t = turn(
        &client,
        &AskRequest::new(
            &key,
            "Use your code execution tool to write a file named numbers.txt containing the numbers 1 to 5, \
             one per line (pass outputs=['numbers.txt']). Then reply with just: done",
        ),
    )
    .await;
    println!("answer: {} | artifacts: {:?}", t.answer, t.outputs.artifacts);
    let conv = t.conversation_id.expect("conversation id");
    let art = t.outputs.artifacts.first().expect("an artifact was produced");
    let d = client.download_artifact(&key, &conv, &art.id).await.expect("download");
    let text = String::from_utf8_lossy(&d.bytes);
    assert!(
        text.contains('1') && text.contains('5'),
        "unexpected artifact content: {text}"
    );
    assert!(d.filename.ends_with(".txt"), "{}", d.filename);
}

#[tokio::test]
#[ignore]
async fn tts_then_stt() {
    let Some((client, key)) = live() else { return };
    let audio = client
        .tts(
            &key,
            "Hello from the docsgpt-rs integration test. The magic word is pineapple.",
        )
        .await
        .expect("tts");
    assert!(audio.len() > 1000, "tts returned {} bytes", audio.len());
    let text = client
        .stt(&key, &Upload::new("speech.mp3", audio).mime("audio/mpeg"))
        .await
        .expect("stt");
    println!("stt: {text}");
    assert!(text.to_lowercase().contains("pineapple"));
}
