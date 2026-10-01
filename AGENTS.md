# AGENTS.md

Cargo workspace of Rust crates for DocsGPT. `crates/docsgpt` is the API client; `crates/docsgpt-bot` (in progress) is the shared bot plumbing used by the Telegram and Slack bots, which live in their own repos.

## Commands

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
```

MSRV is 1.88 (let-chains); CI checks it. Lines up to 120 characters (`rustfmt.toml`).

## Rules

- Red/green: write or port the failing test first.
- These are libraries: `thiserror` errors, no `anyhow`, no panics on server input, docs on every public item (`missing_docs` warns, and CI denies warnings).
- Public enums that DocsGPT may extend (`Event`, `Error`) are `#[non_exhaustive]`.
- Test against `docsgpt::mock::MockDocsGpt`, not hand-rolled servers. Add routes or fault hooks there when a new endpoint is covered.
- When the DocsGPT API changes, check the server source (`docsgpt/api/answer/routes/`, `docsgpt/api/user/`) rather than guessing.
- Live tests (`tests/live.rs`) are `#[ignore]` and read `DOCSGPT_LIVE_KEY` / `DOCSGPT_LIVE_BASE`.
- Keep `CHANGELOG.md` updated. Releases are tag-driven (see README).
