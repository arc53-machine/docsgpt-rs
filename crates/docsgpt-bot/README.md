# docsgpt-bot

Shared plumbing for chat bots that answer with [DocsGPT](https://www.docsgpt.cloud/) agents. The [Slack](https://github.com/arc53/slack-bot-docsgpt-extenstion) and [Telegram](https://github.com/arc53/tg-bot-docsgpt-extenstion) bots use it; a bot adds only what its platform needs.

| Module | What it does |
|---|---|
| `config` | `AgentConfig`, `[storage]` and `[server]` tables to embed in a bot's own config struct, `${VAR}` / `${VAR:-default}` expansion, `load_toml`, and the single-bot `API_KEY` / `API_KEY_<NAME>` layout. |
| `Agents` | Validates a bot's agents and routes a message: a `#name` prefix, then the chat's active agent, then the default. |
| `storage` | Which DocsGPT conversation each chat (and thread) is in for each agent, how many turns it has had (feedback needs an answer's position), per-chat state, and JSON records for platform-only state. SQLite or memory. |
| `runtime` | `ScopeLocks` (one turn at a time per chat), `CancelRegistry` (stoppable turns), `Shutdown` (signal handling that waits for turns in progress). |
| `markdown` | Splits long answers into messages without breaking code blocks, closes open fences in partial answers, extracts images. |

Coming next: a platform-neutral answer loop that each bot drives through a small `Surface` trait.

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
