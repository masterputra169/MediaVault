# Security Policy

## Scope & Threat Model

MediaVault Desktop is designed as a **local-only, single-user** Windows desktop application:

- **Zero telemetry & zero cloud backend:** All history, preferences, and downloaded files stay strictly on the user's machine.
- **SSRF & DNS Rebinding Protection:** Target URLs are validated against an explicit platform/CDN allowlist, non-public IPv4/IPv6 ranges are blocked, and a custom `PublicOnlyResolver` pins validated public IPs directly inside `reqwest`.
- **Safe Subprocess Execution:** External tools (`yt-dlp`, `ffmpeg`, `ffprobe`, `gallery-dl`) are invoked strictly via argument arrays (never via shell interpolation), with `--ignore-config`, `YTDLP_NO_PLUGINS=1`, sanitized environment variables, timeouts, file size caps, and `kill_on_drop(true)`.
- **Cryptographically Verified Tool Installer:** Built-in dependency installation is **strictly user-initiated**, downloads exclusively over HTTPS from allowlisted official GitHub release hosts, verifies **SHA-256 digests** & `Content-Length`, protects against zip bombs & Zip-Slip, and replaces binaries atomically.
- **Strict WebView2 Sandbox & Local Preview Isolation:** Zero `innerHTML`/`eval` sinks, strict CSP blocking arbitrary remote `https:` fetches, and local media previews are served exclusively through the UUID-bound `mvmedia` custom protocol (never exposing raw filesystem paths).
- **Path & NTLM Protection:** Download folder settings reject UNC shares (`\\server\share`), traversal (`..`), symlinks, NTFS ADS, Windows reserved DOS names, and sensitive system/application binary folders.
- **No Bypass Features:** Does **not** bypass DRM, paywalls, anti-bot gates, or private/login-required content, and does **not** import browser cookies.

## Reporting a Vulnerability

If you discover a security vulnerability, please **do not** open a public issue.

Instead, report it privately using **GitHub Private Vulnerability Reporting** (*Security → Advisories → Report a vulnerability*) on [masterputra169/MediaVault](https://github.com/masterputra169/MediaVault) with:

1. A clear description of the issue and affected file/module in `rust-desktop/`.
2. Steps to reproduce or a minimal proof of concept.
3. Any suggested remediation if known.

We will acknowledge reports promptly and coordinate a fix before public disclosure.
