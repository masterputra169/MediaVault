# Contributing to MediaVault (100% Pure Rust)

Thank you for your interest in contributing to **MediaVault**!

## Development Guidelines

- Keep MediaVault **100% Pure Rust**, local-first, single-user, and zero-external-dependency (no `yt-dlp`, `FFmpeg`, `ffprobe`, `gallery-dl`, `Node.js`, or WebView/JS runtime).
- Keep URL/SSRF policy (`url_policy.rs`), pure-Rust MP4/M4A engine (`mp4_engine.rs`), extractors (`extractors.rs`), download engine (`engine.rs`), storage (`db.rs`), audio player (`audio_player.rs`), and native GUI (`ui.rs`) cleanly separated and covered by offline unit tests.
- Never commit `.env`, SQLite `.db` files, `target/`, or compiled `.exe`/`.msi` binaries.

## Running Tests Locally

Prerequisite: **Rust (stable)** only.

```powershell
cd rust-desktop
cargo test --locked
```

## Pull Request Checklist

- [ ] `cargo test --locked` passes with zero errors.
- [ ] All security invariants (URL/CDN allowlists, `PublicOnlyResolver`, anti-UNC folder validation, file/image decode limits) remain intact.
- [ ] `README.md` is updated when features or architecture change.
