# Contributing to MediaVault

Thank you for your interest in contributing to MediaVault!

## Development Guidelines

- Keep MediaVault local-first and privacy-focused (no telemetry, no cloud dependencies).
- Do not add DRM/paywall bypasses, login/cookie importers, or anti-bot bypasses.
- Keep extractors, URL policy, storage/SQLite, and UI cleanly separated and covered by offline unit tests.
- Never commit `.env`, SQLite `.db` files, `target/`, `node_modules/`, or compiled `.exe` binaries.

## Running Tests Locally

### Desktop App (Rust + Tauri v2)

```powershell
node --check rust-desktop/ui/app.js
cd rust-desktop/src-tauri
cargo test
```

### Web App (FastAPI + Next.js)

```powershell
# Backend
cd backend
.\.venv\Scripts\python.exe -m pytest -q

# Frontend
cd frontend
pnpm lint
pnpm typecheck
pnpm build
```

## Pull Request Checklist

- [ ] All relevant offline tests pass without requiring live internet.
- [ ] Security invariants (URL allowlist, SSRF public-IP checks, argument-array subprocesses, XSS-safe DOM rendering) are preserved.
- [ ] Documentation is updated if behavior or configuration changes.
