# MediaVault Desktop

**A 100% Pure-Rust, local-first desktop downloader for public videos, audio, photos, and multi-slide albums — with zero external dependencies.**

[![CI](https://github.com/masterputra169/MediaVault/actions/workflows/ci.yml/badge.svg)](https://github.com/masterputra169/MediaVault/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/masterputra169/MediaVault?label=Download&color=6366f1)](https://github.com/masterputra169/MediaVault/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-10b981.svg)](LICENSE)

MediaVault Desktop (`v0.4.0`) is written **100% in Rust** — GUI, media extractors, ISO-BMFF MP4/fMP4 muxer & probe, in-app hardware-accelerated video player, audio decoder/player, image decoder, ZIP archiver, and embedded SQLite database. It does **not** require `yt-dlp`, `FFmpeg`, `ffprobe`, `gallery-dl`, `Node.js`, `Python`, or `WebView2`.

---

## Download (Windows x64)

| Package | Direct Download (`v0.4.0`) | Description |
|---|---|---|
| **Windows Installer (Recommended)** | [MediaVault_0.4.0_x64-setup.exe](https://github.com/masterputra169/MediaVault/releases/download/v0.4.0/MediaVault_0.4.0_x64-setup.exe) | Per-user NSIS installer (~4.7 MiB) with selectable install directory, Start Menu & Desktop shortcuts, and clean uninstaller. |
| **Portable Executable** | [MediaVault.exe](https://github.com/masterputra169/MediaVault/releases/download/v0.4.0/MediaVault.exe) | Single standalone `.exe` (~14 MiB) — run directly from any folder with zero installation. |
| **SHA-256 Checksums** | [SHA256SUMS.txt](https://github.com/masterputra169/MediaVault/releases/download/v0.4.0/SHA256SUMS.txt) | Verify integrity of every official release artifact. |

Browse all releases: **https://github.com/masterputra169/MediaVault/releases**

---

## Key Features

- **100% Pure Rust, Zero External Dependencies**
  - No `yt-dlp`, no `FFmpeg`/`ffprobe`, no `gallery-dl`, no `Node.js`, and no browser WebView runtime.
  - Sub-second URL analysis, low RAM footprint, and instant native GPU startup.
- **Modern Animated Native UI (Full English)**
  - Hardware-accelerated native UI (`eframe`/`egui`) with crisp Windows system typography (`Segoe UI`), custom anti-aliased vector icons (zero missing-glyph boxes), seamless Windows 11 dark/light DWM titlebar integration, animated sliding tab indicator, glowing shimmer progress bars, smooth hover/modal transitions, and one-click Dark/Light theme switching.
- **7 Supported Public Platforms**
  - **YouTube** — Native InnerTube client extraction (`avc1` H.264 up to 1080p + `mp4a` AAC) with 4 MiB HTTP `Range` chunked streaming to avoid 1× playback throttling, losslessly muxed in pure Rust.
  - **Facebook** — Public Reels, Watch videos, single photos, and multi-photo albums with validated CDN redirect handling (`fbcdn.net`).
  - **X (Twitter)** — Public video posts, multi-photo posts, and quoted media via syndication extraction.
  - **Instagram** — Public Reels, video posts, and multi-slide photo carousels.
  - **Threads** — Public video, photo, and quoted/reposted thread media.
  - **Pinterest** — Public video pins and high-resolution original image pins.
  - **TikTok** — Public video/photo posts (strictly respects anti-bot/WAF boundaries without bypass).
- **Built-in Pure-Rust Media Engine**
  - **ISO-BMFF MP4 / fMP4 Muxer & Probe (`mp4_engine.rs`)**: Parses standard `stbl` and fragmented DASH (`moof`/`trun`) streams, losslessly muxes adaptive H.264 video + AAC audio into interleaved `co64` fast-start `.mp4`, extracts lossless `.m4a`, transcodes 16-bit PCM `.wav`, and probes resolution/duration/codecs in `<1 ms`.
  - **In-App Video, Audio & Photo Preview**: Play downloaded `.mp4` videos **directly inside the app** with synchronized audio, interactive timeline seeking, volume/mute controls, keyboard shortcuts (`Space`, `←`/`→`, `M`), and a responsive cinema stage built for both **widescreen 16:9 landscape** and **9:16 portrait** videos (powered by built-in Windows Media Foundation + `Rodio` — zero FFmpeg/VLC). View photos & multi-slide ZIP albums with `←`/`→` navigation and play `.m4a`/`.wav` audio natively via `Symphonia` + `Rodio`.
- **Complete Download Manager**
  - Live progress bar with transfer speed (`MB/s`), byte count, and ETA.
  - Cancel active downloads cleanly, retry failed items, move completed files to another folder, filter by media type/platform/search query, and paginate download history persisted in local SQLite.

---

## Architecture

| Module | File | Responsibility |
|---|---|---|
| Native Entrypoint | [`rust-desktop/src/main.rs`](rust-desktop/src/main.rs) | Native window bootstrap, Tokio async runtime, bounded `crash.log` |
| Modern UI & Design System | [`rust-desktop/src/ui.rs`](rust-desktop/src/ui.rs), [`rust-desktop/src/ui_kit.rs`](rust-desktop/src/ui_kit.rs) | Animated native UI, custom vector icons, Windows 11 DWM titlebar sync, GPU thumbnail/album viewer |
| Pure-Rust MP4 & Media Players | [`rust-desktop/src/mp4_engine.rs`](rust-desktop/src/mp4_engine.rs), [`rust-desktop/src/video_player.rs`](rust-desktop/src/video_player.rs), [`rust-desktop/src/audio_player.rs`](rust-desktop/src/audio_player.rs) | ISO-BMFF muxer/demuxer/probe, lossless M4A extraction, WAV transcoding, real-time in-app MP4 video player (Windows Media Foundation + Rodio), and native audio player |
| 7-Platform Extractors | [`rust-desktop/src/extractors.rs`](rust-desktop/src/extractors.rs) | Native HTTP/JSON/HTML extractors, bounded GPU thumbnail `PreviewCache`, streaming ZIP album bundler |
| Download Queue & FS Ops | [`rust-desktop/src/engine.rs`](rust-desktop/src/engine.rs) | Concurrent job runner, cancelable streaming, validated CDN redirects, ranged YouTube streaming, safe Explorer integration |
| URL & SSRF Security Policy | [`rust-desktop/src/url_policy.rs`](rust-desktop/src/url_policy.rs) | Strict platform & CDN allowlists, `PublicOnlyResolver` (anti-SSRF & DNS rebinding), IPv4/IPv6 global unicast enforcement |
| Storage & Path Guard | [`rust-desktop/src/db.rs`](rust-desktop/src/db.rs) | Embedded SQLite (`rusqlite`), destination folder validation (anti-UNC, traversal, symlink, ADS, system directory blocks) |
| Structured Errors | [`rust-desktop/src/errors.rs`](rust-desktop/src/errors.rs) | Clean user-facing English error codes without raw stack traces |
| Windows Installer | [`rust-desktop/installer.nsi`](rust-desktop/installer.nsi) | Native per-user NSIS installer script |

---

## Privacy & Security Design

- **100% Local & Private**: No telemetry, no analytics, no accounts, and no cloud relay servers.
- **Anti-SSRF & DNS Rebinding Protection**: Every outbound HTTP request uses a custom `PublicOnlyResolver` that blocks loopback, RFC1918 private, CGNAT, link-local, documentation, and non-global IPv6 addresses at connect time, plus strict per-platform domain and CDN allowlists on every redirect hop.
- **Strict Filesystem Safety**: Rejects UNC network paths, symlink escapes, Alternate Data Streams, reserved DOS device names (`CON`, `NUL`, etc.), and protected Windows system directories.
- **Public Media Only**: Never bypasses DRM, paywalls, logins, cookies, or anti-bot/WAF protections.

---

## Local Data Locations (Windows)

- **Default Downloads Folder**: `%USERPROFILE%\Downloads\MediaVault` *(customizable globally in Settings or per download)*
- **App Database & Logs**: `%LOCALAPPDATA%\MediaVault\app.db` and `crash.log`

---

## Build from Source

### Prerequisites
- **Rust** (`rustup`, stable 1.80+ with MSVC toolchain on Windows)
- *(Optional, for installer)* **NSIS** (`makensis`)

### Run & Test
```powershell
cd rust-desktop

# Run all offline unit tests (no internet required)
cargo test --locked

# Build optimized standalone executable
cargo build --release --locked
# Output: rust-desktop\target\release\mediavault.exe

# Build Windows NSIS installer (optional)
makensis installer.nsi
# Output: rust-desktop\target\release\MediaVault_0.4.0_x64-setup.exe
```

---

## Known Limitations

- Platform extraction targets public, unauthenticated posts only. Platforms may occasionally change page markup or enforce IP-level bot checks (for example, TikTok may present a WAF challenge on certain networks, which MediaVault reports cleanly as `LOGIN_REQUIRED` without attempting bypass).
- In-app hardware-accelerated `.mp4` video playback uses Windows Media Foundation on Windows 10/11 (with an **External** button always available to open files in your system media player).

---

## License

Released under the [MIT License](LICENSE). See [SECURITY.md](SECURITY.md) for vulnerability reporting and [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines.
