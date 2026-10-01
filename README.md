# docsgpt-rs

Rust crates for [DocsGPT](https://github.com/arc53/DocsGPT).

| Crate | What it is |
|---|---|
| [`docsgpt`](crates/docsgpt) | API client: streaming answers, attachments, speech, artifacts, feedback, and a mock server for tests. |
| [`docsgpt-bot`](crates/docsgpt-bot) | Shared plumbing for chat bots built on DocsGPT agents: config, agent routing, storage, concurrency helpers, Markdown splitting, and an answer loop that works on any platform. |

The [Telegram](https://github.com/arc53/tg-bot-docsgpt-extenstion) and [Slack](https://github.com/arc53/slack-bot-docsgpt-extenstion) bots are built on these crates.

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Live tests against a real DocsGPT are ignored by default:

```bash
DOCSGPT_LIVE_KEY=<agent api key> DOCSGPT_LIVE_BASE=http://localhost:7091 \
  cargo test -p docsgpt --test live -- --ignored --nocapture
```

`protocol_smoke` passes with any model, including `docsgpt dev --mock-llm`. The other tests check answer content and need a real model.

Stream an answer in the terminal:

```bash
DOCSGPT_BASE=http://localhost:7091 DOCSGPT_KEY=<agent api key> cargo run --example ask -- "What is DocsGPT?"
```

## Releasing

1. Bump `version` in the root `Cargo.toml` and update `CHANGELOG.md`.
2. Push a tag `vX.Y.Z`. The `Release` workflow checks that the tag matches the version, runs the tests, and publishes every crate to crates.io in dependency order.

The workflow authenticates with crates.io Trusted Publishing. The first release of a new crate needs a `CARGO_REGISTRY_TOKEN` repository secret. After that release, configure Trusted Publishing for the crate (repo `arc53/docsgpt-rs`, workflow `release.yml`, environment `release`) and delete the secret.

## License

MIT
