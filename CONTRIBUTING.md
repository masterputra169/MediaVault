# Contributing to MediaVault Desktop

Thank you for your interest in contributing to **MediaVault Desktop**!

## Development Guidelines

- Keep MediaVault local-first, single-user, and privacy-focused (no telemetry, no cloud APIs).
- Do not add DRM/paywall bypasses, login/cookie importers, or anti-bot bypasses.
- Keep URL/SSRF policy (`url_policy.rs`), extractors (`images.rs`, `engine.rs`), dependency installer (`installer.rs`), storage (`db.rs`), IPC/protocol (`lib.rs`), and UI (`ui/`) cleanly separated and covered by offline unit tests.
- Never commit `.env`, SQLite `.db` files, `target/`, `node_modules/`, or compiled `.exe`/`.msi` binaries.

## Running Tests Locally

Prasyarat: **Rust (stable)** dan **Node.js 22+**.

```powershell
# Verifikasi sintaks UI
node --check rust-desktop/ui/app.js

# Jalankan seluruh unit test offline (tanpa internet)
cd rust-desktop/src-tauri
cargo test --locked
```

## Pull Request Checklist

- [ ] `node --check rust-desktop/ui/app.js` dan `cargo test --locked` lulus tanpa error.
- [ ] Seluruh invarian keamanan (URL/CDN allowlist, `PublicOnlyResolver`, verifikasi SHA-256 installer, proteksi zip bomb, validasi folder anti-UNC, CSP ketat, dan rendering DOM bebas XSS) tetap terjaga.
- [ ] `README.md` diperbarui bila ada perubahan fitur, platform, atau alur build.
