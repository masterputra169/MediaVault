# Contributing to MediaVault (100% Pure Rust)

Thank you for your interest in contributing to **MediaVault**!

## Development Guidelines

- Keep MediaVault **100% Pure Rust**, local-first, single-user, and zero-external-dependency (no `yt-dlp`, `FFmpeg`, `ffprobe`, `gallery-dl`, `Node.js`, or WebView/JS runtime).
- Do not add DRM/paywall bypasses, login/cookie importers, or WAF/anti-bot bypasses.
- Keep URL/SSRF policy (`url_policy.rs`), pure-Rust MP4/M4A engine (`mp4_engine.rs`), extractors (`extractors.rs`), download engine (`engine.rs`), storage (`db.rs`), audio player (`audio_player.rs`), and native GUI (`ui.rs`) cleanly separated and covered by offline unit tests.
- Never commit `.env`, SQLite `.db` files, `target/`, or compiled `.exe`/`.msi` binaries.

## Running Tests Locally

Prasyarat: **Rust (stable)** saja.

```powershell
cd rust-desktop
cargo test --locked
```

## Pull Request Checklist

- [ ] `cargo test --locked` lulus tanpa warning maupun error.
- [ ] Seluruh invarian keamanan (URL/CDN allowlist, `PublicOnlyResolver`, validasi folder anti-UNC, batas ukuran file/gambar) tetap terjaga.
- [ ] `README.md` diperbarui bila ada perubahan fitur atau arsitektur.
