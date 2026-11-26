# Security Policy

## Scope & Threat Model

MediaVault Desktop is a **100% Pure-Rust, local-only, single-user** Windows desktop application with **zero external runtime dependencies** (no `yt-dlp`, no `FFmpeg`, no `ffprobe`, no `gallery-dl`, no `Node.js`, no WebView2/HTML/JS):

- **Zero Telemetry & Zero Cloud Backend:** All history, preferences, and downloaded media stay strictly on the user's machine.
- **Zero Untrusted Subprocess / Tool Execution:** Media extraction, MP4/fMP4 muxing, M4A/WAV audio conversion, and media probing execute directly in compiled memory-safe Rust without spawning `yt-dlp`, `FFmpeg`, `gallery-dl`, Python, or Node.js.
- **SSRF & DNS Rebinding Protection:** Target URLs are validated against an explicit 7-platform & CDN allowlist, all non-public IPv4/IPv6 ranges are blocked, and a custom `PublicOnlyResolver` pins validated public IPs directly inside `reqwest`.
- **Memory & Archive Safety:** Strict `image::Limits` (`16384×16384`, `128 MiB` decode cap), 2 GiB download size cap, disk-streamed ZIP creation, and symlink/size-bounded ZIP album reading.
- **Path & NTLM Protection:** Download folder settings reject UNC shares (`\\server\share`), traversal (`..`), symlinks, NTFS ADS (`:`), Windows reserved DOS device names, and sensitive system directories.
- **No Bypass Features:** Does **not** bypass DRM, paywalls, anti-bot/WAF gates, or private/login-required content, and does **not** import browser cookies.

## Reporting a Vulnerability

If you discover a security vulnerability, please **do not** open a public issue.

Instead, report it privately using **GitHub Private Vulnerability Reporting** (*Security → Advisories → Report a vulnerability*) on [masterputra169/MediaVault](https://github.com/masterputra169/MediaVault).
