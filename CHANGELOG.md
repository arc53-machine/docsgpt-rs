# Changelog

## Unreleased

### docsgpt-bot 0.1.0

First release. Generalized from the Telegram bot:

- `config`: `AgentConfig`, `StorageConfig`, `ServerConfig`, `${VAR}` expansion, `load_toml`, `agents_from_env`.
- `Agents`: validation and `#name` / active / default routing.
- `storage`: string-keyed `Scope`, conversations with a turn counter (feedback's `question_index`), chat state, JSON records, and message refs. SQLite and memory backends share one contract test suite. Reads Telegram bot v2 SQLite files.
- `runtime`: `ScopeLocks`, `CancelRegistry` (guards unregister on drop), `Shutdown` that drains turns in progress.
- `markdown`: fence-aware block splitting and clamping (the Telegram bot could cut inside a code block), image extraction, monospace tables.

### docsgpt 0.1.0

First release. Ported from the Telegram bot's DocsGPT client, with these changes:

- Typed `Error`s (HTTP status, feature disabled, timeout, transport, decode, too large, task failed).
- Retries: requests that are safe to repeat retry on connection errors, timeouts and 429/502/503/504; `/stream`, `/api/answer` and uploads retry only when the request never reached DocsGPT.
- `/stream` fails with `Error::Timeout` after a configurable silence (default 150 s).
- New `Client::feedback` (`POST /api/feedback`).
- `tts` sends the agent key.
- Fix: a multi-byte character split across two network chunks no longer turns into `�`.
- The Python-repr reader handles `\x` and `\U` escapes and control characters.
- `mock` feature: an in-process fake DocsGPT with request recording and fault injection.
