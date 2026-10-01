//! The client against the in-process mock DocsGPT.

use std::time::Duration;

use docsgpt::mock::{MockDocsGpt, Step, StreamReply, answer_steps, ev, sse};
use docsgpt::{AskRequest, Client, Error, Event, Feedback, RetryPolicy, Source, TaskWait, Upload};
use futures_util::StreamExt;
use serde_json::json;

fn client(docs: &MockDocsGpt) -> Client {
    Client::builder(&docs.url)
        .retry(RetryPolicy {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(20),
        })
        .build()
        .unwrap()
}

async fn collect(c: &Client, req: &AskRequest) -> Vec<Result<Event, Error>> {
    c.stream(req).await.unwrap().collect().await
}

#[tokio::test]
async fn streams_a_turn() {
    let docs = MockDocsGpt::start().await;
    docs.on_stream(|_| sse(answer_steps("Hello there", "conv-9", &[("Doc", "https://d")])));
    let events = collect(&client(&docs), &AskRequest::new("key", "hi").conversation_id("conv-9")).await;
    let events: Vec<Event> = events.into_iter().map(Result::unwrap).collect();

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            Event::Answer(d) => Some(d.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello there");
    assert!(matches!(&events[0], Event::MessageId { conversation_id: Some(c), .. } if c == "conv-9"));
    assert!(events.contains(&Event::Source(vec![Source {
        title: "Doc".into(),
        url: Some("https://d".into())
    }])));
    assert_eq!(events.last(), Some(&Event::End));

    let call = docs.rec.last("/stream").unwrap();
    assert_eq!(
        call.body,
        json!({"question": "hi", "api_key": "key", "conversation_id": "conv-9"})
    );
}

#[tokio::test]
async fn event_split_across_chunks_and_trailing_event_without_blank_line() {
    let docs = MockDocsGpt::start().await;
    docs.on_stream(|_| {
        sse(vec![
            Step::Raw("data: {\"type\": \"answer\", \"answer\": \"Gr".into()),
            Step::Raw("üße\"}\n\n".into()),
            Step::Raw("data: {\"type\": \"end\"}".into()),
        ])
    });
    let events: Vec<Event> = collect(&client(&docs), &AskRequest::new("k", "q"))
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(events, vec![Event::Answer("Grüße".into()), Event::End]);
}

#[tokio::test]
async fn idle_stream_times_out() {
    let docs = MockDocsGpt::start().await;
    docs.on_stream(|_| {
        sse(vec![
            ev::step(ev::answer("partial")),
            Step::Hang(Duration::from_secs(5)),
        ])
    });
    let c = Client::builder(&docs.url)
        .stream_idle_timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let events = collect(&c, &AskRequest::new("k", "q")).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].as_ref().unwrap(), &Event::Answer("partial".into()));
    assert!(matches!(events[1], Err(Error::Timeout { endpoint: "/stream" })));
}

#[tokio::test]
async fn http_error_is_typed() {
    let docs = MockDocsGpt::start().await;
    docs.on_stream(|_| StreamReply::Http(401, "{\"error\": \"Unauthorized\"}".into()));
    let err = client(&docs).stream(&AskRequest::new("bad", "q")).await.unwrap_err();
    assert_eq!(err.status(), Some(401));
    assert!(err.to_string().contains("Unauthorized"), "{err}");
    assert_eq!(docs.rec.count("/stream"), 1, "4xx must not be retried");
}

#[tokio::test]
async fn stream_retries_503_but_not_502() {
    let docs = MockDocsGpt::start().await;
    docs.fail_next("/stream", 503, 2);
    let events = collect(&client(&docs), &AskRequest::new("k", "q")).await;
    assert!(events.iter().all(Result::is_ok));
    assert_eq!(docs.rec.count("/stream"), 3);

    // A 502 may come from a proxy after DocsGPT already started the turn.
    let docs = MockDocsGpt::start().await;
    docs.fail_next("/stream", 502, 1);
    let err = client(&docs).stream(&AskRequest::new("k", "q")).await.unwrap_err();
    assert_eq!(err.status(), Some(502));
    assert_eq!(docs.rec.count("/stream"), 1);
}

#[tokio::test]
async fn retries_give_up_after_max_attempts() {
    let docs = MockDocsGpt::start().await;
    docs.fail_next("/api/feedback", 503, 5);
    let err = client(&docs).feedback("k", "c", 0, Feedback::Like).await.unwrap_err();
    assert_eq!(err.status(), Some(503));
    assert_eq!(docs.rec.count("/api/feedback"), 3);
}

#[tokio::test]
async fn connection_refused_is_transport_error() {
    let c = Client::builder("http://127.0.0.1:9")
        .retry(RetryPolicy::none())
        .build()
        .unwrap();
    let err = c.stream(&AskRequest::new("k", "q")).await.unwrap_err();
    assert!(
        matches!(
            err,
            Error::Transport {
                endpoint: "/stream",
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn answer_endpoint() {
    let docs = MockDocsGpt::start().await;
    docs.on_answer(|body| {
        assert_eq!(body["question"], "q");
        (200, json!({"answer": "A", "conversation_id": "c1", "sources": [{"title": "S", "link": "https://s"}], "tool_calls": [], "thought": null}))
    });
    let a = client(&docs).answer(&AskRequest::new("k", "q")).await.unwrap();
    assert_eq!(a.answer, "A");
    assert_eq!(a.conversation_id.as_deref(), Some("c1"));
    assert_eq!(a.sources()[0].url.as_deref(), Some("https://s"));
}

#[tokio::test]
async fn feedback_body() {
    let docs = MockDocsGpt::start().await;
    let c = client(&docs);
    c.feedback("k", "conv", 3, Feedback::Dislike).await.unwrap();
    c.feedback("k", "conv", 3, Feedback::Clear).await.unwrap();
    let calls = docs.rec.calls("/api/feedback");
    assert_eq!(
        calls[0].body,
        json!({"feedback": "dislike", "conversation_id": "conv", "question_index": 3, "api_key": "k"})
    );
    assert!(calls[1].body["feedback"].is_null());

    docs.set_feedback_reply(404, json!({"success": false, "message": "Not found"}));
    assert_eq!(
        c.feedback("k", "other", 0, Feedback::Like).await.unwrap_err().status(),
        Some(404)
    );
}

#[tokio::test]
async fn upload_and_wait_for_task() {
    let docs = MockDocsGpt::start().await;
    let c = client(&docs);
    let file = Upload::new("notes.md", "# hi").mime("text/markdown");
    let att = c.upload_attachment("k", &file).await.unwrap();
    assert_eq!(att.id, "att-1");
    assert_eq!(att.task_id.as_deref(), Some("task-1"));
    let call = docs.rec.last("/api/store_attachment").unwrap();
    assert_eq!(call.body["api_key"], "k");
    let f = call.file("file");
    assert_eq!(
        (f.filename.as_str(), f.content_type.as_deref(), &f.bytes[..]),
        ("notes.md", Some("text/markdown"), &b"# hi"[..])
    );

    docs.queue_task_status(&["PENDING", "PROGRESS"]);
    assert_eq!(
        c.wait_for_task("task-1", Duration::from_secs(10)).await.unwrap(),
        TaskWait::Done
    );
    assert_eq!(docs.rec.count("/api/task_status"), 3);

    docs.queue_task_status(&["FAILURE"]);
    assert!(matches!(
        c.wait_for_task("task-1", Duration::from_secs(10)).await,
        Err(Error::TaskFailed { .. })
    ));

    docs.queue_task_status(&["PENDING"; 10]);
    assert_eq!(
        c.wait_for_task("t", Duration::from_millis(100)).await.unwrap(),
        TaskWait::StillRunning
    );
}

#[tokio::test]
async fn upload_multi_file_shape_and_missing_id() {
    let docs = MockDocsGpt::start().await;
    let c = client(&docs);
    docs.set_store_attachment(
        200,
        json!({"success": true, "tasks": [{"task_id": "t2", "attachment_id": "a2"}]}),
    );
    let att = c.upload_attachment("k", &Upload::new("a.txt", "x")).await.unwrap();
    assert_eq!((att.id.as_str(), att.task_id.as_deref()), ("a2", Some("t2")));

    docs.set_store_attachment(200, json!({"success": true}));
    assert!(matches!(
        c.upload_attachment("k", &Upload::new("a.txt", "x")).await,
        Err(Error::Decode { .. })
    ));
}

#[tokio::test]
async fn speech() {
    let docs = MockDocsGpt::start().await;
    let c = client(&docs);
    docs.set_stt("  hello world ");
    assert_eq!(
        c.stt("k", &Upload::new("v.ogg", vec![1u8, 2, 3]).mime("audio/ogg"))
            .await
            .unwrap(),
        "hello world"
    );
    let call = docs.rec.last("/api/stt").unwrap();
    assert_eq!(call.query["api_key"], "k");
    assert_eq!(call.file("file").filename, "v.ogg");

    assert_eq!(&c.tts("k", "hi").await.unwrap()[..], b"ID3");
    assert_eq!(
        docs.rec.last("/api/tts").unwrap().body,
        json!({"text": "hi", "api_key": "k"})
    );

    docs.set_tts_reply(404, json!({"success": false, "message": "Text-to-speech is disabled"}));
    match c.tts("k", "hi").await {
        Err(Error::FeatureDisabled {
            endpoint: "/api/tts",
            message,
        }) => assert!(message.contains("disabled")),
        other => panic!("{other:?}"),
    }
    docs.set_stt_reply(200, json!({"success": true, "text": ""}));
    assert!(matches!(
        c.stt("k", &Upload::new("v.ogg", vec![1u8])).await,
        Err(Error::Decode { .. })
    ));
}

#[tokio::test]
async fn downloads() {
    let docs = MockDocsGpt::start().await;
    let c = client(&docs);
    docs.add_artifact("a1", "report.pdf", "application/pdf", &b"%PDF"[..]);
    let d = c.download_artifact("k", "conv", "a1").await.unwrap();
    assert_eq!(
        (d.filename.as_str(), d.mime.as_deref(), &d.bytes[..]),
        ("report.pdf", Some("application/pdf"), &b"%PDF"[..])
    );
    let call = docs.rec.last("/api/artifacts/download").unwrap();
    assert_eq!(
        (call.query["api_key"].as_str(), call.query["conversation_id"].as_str()),
        ("k", "conv")
    );

    // Safe to repeat, so a 502 is retried.
    docs.fail_next("/api/artifacts/download", 502, 1);
    assert!(c.download_artifact("k", "conv", "a1").await.is_ok());

    assert_eq!(
        c.download_artifact("k", "conv", "nope").await.unwrap_err().status(),
        Some(404)
    );

    docs.add_image("cat.png", vec![0u8; 64]);
    let img = c
        .fetch_url(&format!("{}?sig=1", docs.image_url("cat.png")))
        .await
        .unwrap();
    assert_eq!((img.filename.as_str(), img.bytes.len()), ("cat.png", 64));

    let small = Client::builder(&docs.url).max_download_bytes(10).build().unwrap();
    assert!(matches!(
        small.fetch_url(&docs.image_url("cat.png")).await,
        Err(Error::TooLarge { limit: 10 })
    ));
}
