# docsgpt-bot

Shared plumbing for chat bots that answer with [DocsGPT](https://www.docsgpt.cloud/) agents. The [Slack](https://github.com/arc53/slack-bot-docsgpt-extenstion) and [Telegram](https://github.com/arc53/tg-bot-docsgpt-extenstion) bots use it; a bot adds only what its platform needs.

| Module | What it does |
|---|---|
| `config` | `AgentConfig`, `[storage]` and `[server]` tables to embed in a bot's own config struct, `${VAR}` / `${VAR:-default}` expansion, `load_toml`, and the single-bot `API_KEY` / `API_KEY_<NAME>` layout. |
| `Agents` | Validates a bot's agents and routes a message: a `#name` prefix, then the chat's active agent, then the default. |
| `storage` | Which DocsGPT conversation each chat (and thread) is in for each agent, how many turns it has had (feedback needs an answer's position), per-chat state, and JSON records for platform-only state. SQLite or memory. |
| `runtime` | `ScopeLocks` (one turn at a time per chat), `CancelRegistry` (stoppable turns), `Shutdown` (signal handling that waits for turns in progress). |
| `markdown` | Splits long answers into messages without breaking code blocks, closes open fences in partial answers, extracts images. |
| `run_turn` | The answer loop. It routes to an agent, streams DocsGPT's answer, shows tool status, handles Stop, records the conversation and the answer's position, downloads tool files, and maps the answer's message to it for `submit_feedback`. |

## The answer loop

A platform implements `Surface` once. Build one per incoming message, holding where to reply:

```rust,ignore
#[async_trait]
impl Surface for SlackReply {
    type Draft = StreamHandle;
    async fn begin(&self, turn: &Turn) -> Result<StreamHandle> { /* start a stream / typing */ }
    async fn update(&self, turn: &Turn, d: &mut StreamHandle, p: Progress<'_>) -> Result<()> { /* show p.answer, p.status */ }
    async fn finish(&self, turn: &Turn, d: StreamHandle, f: &Final) -> Result<Option<String>> { /* f.display_text(), f.sources, f.images */ }
    async fn send_file(&self, turn: &Turn, file: Download) -> Result<()> { /* upload */ }
    async fn notice(&self, text: &str) -> Result<()> { /* plain message */ }
}

let report = run_turn(&core, &SlackReply::new(channel, thread_ts), Ask::new(scope, text)).await?;
```

`run_turn` calls `begin` once, then `update` as the answer streams in. Updates are throttled to `Surface::update_interval`, but tool status changes are shown at once and a draft is refreshed when the stream goes quiet. `finish` is called exactly once whatever happened: `Final::outcome` says whether the answer is complete, stopped or failed, and `Final::display_text` gives the standard wording. A partial answer that ends in an error shows both.

For Stop, register `Turn::cancel` under the platform's id in a `CancelRegistry` inside `begin`, and call `CancelRegistry::cancel` when the user presses Stop.

With the `testing` feature, `testing::FakeSurface` records every call, so a bot can test its routing without a platform. Pair it with `docsgpt`'s `mock` feature.

```rust,no_run
use docsgpt_bot::{AgentConfig, Agents, Routed, Scope, Storage, storage};

# async fn run() -> docsgpt_bot::Result<()> {
let agents = Agents::new(vec![AgentConfig::new("support", "key-1"), AgentConfig::new("sales", "key-2")])?;
let store = storage::open(&Default::default(), "data/bot.db").await?;

let scope = Scope::new("my-bot", "C0123", "1712345678.000100");
let active = store.chat_state(&scope).await?.active_agent;
if let Routed::Agent { agent, question, .. } = agents.route("#sales what does it cost?", active.as_deref()) {
    let conversation = store.conversation(&scope, &agent.name).await?;
    // … ask DocsGPT with `question`, continuing `conversation` …
}
# Ok(()) }
```

The DocsGPT client is the [`docsgpt`](https://crates.io/crates/docsgpt) crate, re-exported as `docsgpt_bot::docsgpt`.

## Storage compatibility

The SQLite schema reads files written by the Telegram bot v2: conversations and chat state carry over, and the turn counter is added on first open.

## License

MIT
