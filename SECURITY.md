# Security Policy

## Scope & Threat Model

MediaVault is designed as a **local-only, single-user** media download application:

- No telemetry, analytics, or external cloud backend.
- Strictly validates target URLs against an explicit platform allowlist and rejects private, loopback, link-local, CGNAT, and metadata IP addresses (SSRF protection).
- Executes external tools (`yt-dlp`, `ffmpeg`, `ffprobe`, `gallery-dl`) strictly via argument arrays without shell interpolation.
- Built-in desktop dependency installation is **strictly user-initiated**, downloads exclusively over HTTPS from allowlisted official GitHub release hosts, enforces size caps, and rejects Zip-Slip path traversal entries.
- Does **not** bypass DRM, paywalls, anti-bot gates, or private/login-required content, and does **not** import browser cookies.

## Reporting a Vulnerability

If you discover a security vulnerability, please **do not** open a public issue.

Instead, report it privately using **GitHub Private Vulnerability Reporting** (*Security → Advisories → Report a vulnerability*) on this repository with:

1. A clear description of the issue and affected component (`rust-desktop` or `backend`/`frontend`).
2. Steps to reproduce or a minimal proof of concept.
3. Any suggested remediation if known.

We will acknowledge reports promptly and coordinate a fix before public disclosure.
