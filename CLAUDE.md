# ClawX Development Guide

See root `../CLAUDE.md` for architecture overview and reference project context.

## Quick Start
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Adding a New Crate
1. Create `crates/clawx-<name>/` with `Cargo.toml` and `src/lib.rs`
2. Add to `[workspace.members]` in root `Cargo.toml`
3. Add to `[workspace.dependencies]` for cross-crate use
4. Follow dependency order: core → llm/memory/tools → agent → channels → cli

## Common Gotchas
- Rust edition 2021, MSRV 1.85
- `rusqlite` uses `bundled` feature — no system SQLite needed
- `reqwest` uses `rustls-tls` — no OpenSSL needed
- All async code uses tokio runtime with `features = ["full"]`
