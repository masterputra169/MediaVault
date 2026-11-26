# MediaVault (100% Pure Rust)

**MediaVault** adalah aplikasi desktop Windows lokal yang ditulis **100% dalam bahasa Rust** — mulai dari **GUI native (`eframe`/`egui`)**, **ekstraktor 7 platform**, **MP4/fMP4 lossless muxer & probe**, **konverter & pemutar audio (`M4A`/`WAV`)**, hingga **database SQLite**.

**Tanpa `yt-dlp`, tanpa `FFmpeg`, tanpa `ffprobe`, tanpa `gallery-dl`, tanpa `Node.js`, tanpa WebView2/HTML/JS, dan tanpa cloud.** Cukup unduh satu file, jalankan, dan langsung pakai dengan konsumsi RAM sangat rendah (~25–45 MB) serta analisis cepat (~0,4–1,3 detik).

[![Release](https://img.shields.io/github/v/release/masterputra169/MediaVault?style=flat-square&label=Download%20Terbaru)](https://github.com/masterputra169/MediaVault/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/masterputra169/MediaVault/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/masterputra169/MediaVault/actions/workflows/ci.yml)
[![Language: 100% Rust](https://img.shields.io/badge/Language-100%25%20Rust-orange?style=flat-square)](rust-desktop/Cargo.toml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)

---

## Unduh MediaVault (Windows x64)

Unduh langsung dari halaman [**GitHub Releases Terbaru**](https://github.com/masterputra169/MediaVault/releases/latest):

| Paket | Deskripsi | Tautan Unduh (v0.2.0) |
| --- | --- | --- |
| **Windows Installer (Direkomendasikan)** | Installer NSIS ringan, bebas pilih lokasi instalasi, tanpa hak Administrator, lengkap dengan shortcut Start Menu & Uninstaller. | [**`MediaVault_0.2.0_x64-setup.exe`**](https://github.com/masterputra169/MediaVault/releases/download/v0.2.0/MediaVault_0.2.0_x64-setup.exe) |
| **Portable EXE** | Satu file `.exe` mandiri — langsung klik & jalankan tanpa instalasi dan tanpa memasang komponen tambahan apa pun. | [**`MediaVault.exe`**](https://github.com/masterputra169/MediaVault/releases/download/v0.2.0/MediaVault.exe) |
| **Checksum SHA-256** | Verifikasi integritas file unduhan sebelum dijalankan. | [**`SHA256SUMS.txt`**](https://github.com/masterputra169/MediaVault/releases/download/v0.2.0/SHA256SUMS.txt) |

> **Verifikasi Checksum (PowerShell):**
> ```powershell
> Get-FileHash .\MediaVault_0.2.0_x64-setup.exe -Algorithm SHA256
> ```
> Cocokkan hasil hash dengan [`SHA256SUMS.txt`](https://github.com/masterputra169/MediaVault/releases/download/v0.2.0/SHA256SUMS.txt).

> **Catatan Windows SmartScreen:** Karena installer open-source ini belum memakai sertifikat Code Signing berbayar, Windows SmartScreen mungkin menampilkan peringatan pada pemakaian pertama. Klik **More info → Run anyway** setelah memverifikasi checksum dari rilis resmi di repositori ini.

---

## Mengapa 100% Pure Rust (Zero External Dependencies)?

| Aspek | Versi Lama (v0.1.0 — Tauri + WebView2 + yt-dlp + FFmpeg) | **MediaVault v0.2.0 (100% Pure Rust)** |
| --- | --- | --- |
| **Konsumsi RAM Idle** | ~130–250 MB (proses Edge WebView2) | **~25–45 MB** (native `eframe`/`egui`) |
| **Dependensi Eksternal** | Wajib pasang `yt-dlp`, `FFmpeg`, `ffprobe`, `gallery-dl`, `Node.js` (~150+ MB) | **0 dependensi** — semua sudah tertanam di dalam 1 file `.exe` |
| **Penggabungan Video + Audio** | Spawn subprocess `ffmpeg.exe` | **Pure-Rust ISO-BMFF Muxer (`mp4_engine.rs`)** — lossless, streaming, <0,3 detik |
| **Ekstraksi Metadata & URL** | Fallback spawn Python `yt-dlp` / `gallery-dl` (3–15 detik) | **100% Pure-Rust async HTTP** (~0,4–1,3 detik) |
| **Bahasa Kode Sumber** | Rust + HTML + CSS + JavaScript + Node/pnpm | **100% Rust** (`cargo build --release`) |

---

## Platform yang Didukung

| Platform | Jenis Tautan Publik | Video & Audio (`MP4` / `M4A` / `WAV`) | Foto & Album (`JPG` / `PNG` / `WebP` / `ZIP`) |
| --- | --- | :---: | :---: |
| **YouTube** | `youtube.com/watch`, `youtu.be`, Shorts, Live VOD | Ya (InnerTube `ANDROID_VR`/`IOS` + Pure-Rust MP4 Muxer) | — |
| **Instagram** | `/p/`, `/reel/`, `/tv/`, `/share/` | Ya | Ya (tunggal & carousel ZIP) |
| **X / Twitter** | `x.com`, `twitter.com`, `t.co`, quote-tweet | Ya | Ya (tunggal & multi-foto ZIP) |
| **Threads** | `threads.com`, `threads.net`, `/share/`, `/t/` | Ya | Ya (tunggal, carousel ZIP, quote-post) |
| **Facebook** | `watch`, `/videos/`, `/reel/`, `/photo/`, `/posts/`, `/share/`, `fb.watch` | Ya | Ya (foto tunggal & album ZIP) |
| **Pinterest** | `/pin/`, `pin.it` | Ya | Ya |
| **TikTok** | `tiktok.com`, `vt`/`vm`, `/video/`, `/photo/` | Ya *(bila tidak dibatasi WAF)* | Ya *(bila tidak dibatasi WAF)* |

> **Disclaimer:** Gunakan MediaVault hanya untuk konten publik yang berhak Anda unduh dan patuhi ketentuan masing-masing platform. MediaVault **tidak** mendukung bypass DRM, paywall, konten privat/wajib login, bypass anti-bot/WAF, atau impor cookie browser.

---

## Fitur Utama

- **100% Mandiri Tanpa Instalasi Tambahan:** Begitu MediaVault dibuka, langsung berstatus **✓ Siap · Zero External Deps** — tidak perlu repot mengunduh `yt-dlp`, `FFmpeg`, `gallery-dl`, maupun `Node.js`.
- **Pure-Rust MP4/fMP4 Muxer & Probe (`mp4_engine.rs`):**
  - Menggabungkan aliran video adaptif YouTube H.264 (`avc1`) + audio AAC (`mp4a`) menjadi file `.mp4` *fast-start* standar yang langsung kompatibel dengan Windows Media Player, HP, WhatsApp, dan editor video — tanpa re-encoding dan tanpa FFmpeg.
  - Mengekstrak audio lossless **`.m4a` (AAC)** dan **`.wav` (PCM 16-bit via Symphonia + Hound)** sepenuhnya di Rust.
- **Pratinjau Media Terunduh Langsung di Aplikasi:**
  - **Foto & Album ZIP:** Viewer gambar native GPU-accelerated langsung di dalam aplikasi (mendukung tombol panah `←`/`→` untuk menelusuri slide album `.zip`).
  - **Audio (`M4A` / `WAV`):** Pemutar audio bawaan pure-Rust (`Rodio` + `Symphonia`) dengan tombol *Putar / Jeda / Berhenti*.
  - **Video (`MP4`):** Inspeksi detail teknis instan `<1 ms` (resolusi, durasi, codec `avc1`/`mp4a`, ukuran file) + thumbnail + tombol **Putar di Pemutar Video**.
- **Kontrol Folder Simpan Fleksibel:** Ubah folder default di **Pengaturan**, pilih folder khusus per unduhan (**Ubah…**), pindahkan file selesai (**Pindahkan**), dan buka langsung lokasi file di Windows Explorer (**Buka folder**).
- **UI Native Ringan (`eframe`/`egui`):** Tema **Gelap / Terang**, tab filter (`Semua`, `Aktif`, `Video`, `Audio`, `Foto`), pencarian judul, filter platform, paginasi, progres real-time (`MB/s` + ETA), pembatalan instan, dan hapus riwayat/file aman.

---

## Lokasi Penyimpanan Data Lokal (Windows)

- **Database SQLite & preferensi:** `%LOCALAPPDATA%\MediaVault\app.db`
- **Folder unduhan default:** `C:\Users\<Username>\Downloads\MediaVault` *(dapat diubah kapan saja di Pengaturan)*

---

## Build dari Source (Developer — Windows)

Prasyarat: **Rust (stable)** saja (https://rustup.rs). Untuk membangun installer `.exe`, tambahkan **NSIS (`makensis`)**.

```powershell
# 1. Clone repositori
git clone https://github.com/masterputra169/MediaVault.git
cd MediaVault\rust-desktop

# 2. Jalankan seluruh unit test offline (100% pure Rust)
cargo test --locked

# 3. Build binary release native (~10 MB, 0 dependensi eksternal)
cargo build --release --locked

# 4. (Opsional) Bangun Windows Installer NSIS
makensis installer.nsi
```

Hasil build tersedia di:
- `rust-desktop/target/release/mediavault.exe`
- `rust-desktop/target/release/MediaVault_0.2.0_x64-setup.exe`

---

## Struktur Repositori (100% Rust)

```text
MediaVault/
├── .github/
│   ├── dependabot.yml             # Pembaruan Cargo & GitHub Actions mingguan
│   └── workflows/
│       ├── ci.yml                 # CI murni Rust: cargo test --locked
│       └── release-windows.yml    # Build release EXE + NSIS installer + SHA256SUMS
├── rust-desktop/
│   ├── Cargo.toml & Cargo.lock    # 100% Rust crate (eframe/egui, reqwest, rusqlite, symphonia, rodio)
│   ├── build.rs                   # Embed ikon Windows ke binary
│   ├── installer.nsi              # Script installer NSIS native (tanpa Node/Tauri)
│   ├── assets/
│   │   ├── icon.ico
│   │   └── icon-128.png
│   └── src/
│       ├── main.rs                # Entrypoint aplikasi native Windows + Tokio runtime
│       ├── ui.rs                  # GUI native 100% Rust (eframe/egui) + modal pratinjau
│       ├── engine.rs              # Mesin analisis, antrean unduhan, & ranged YouTube streamer
│       ├── extractors.rs          # Ekstraktor pure-Rust 7 platform + image/ZIP engine
│       ├── mp4_engine.rs          # Pure-Rust MP4/fMP4 probe, demuxer, fast-start muxer & M4A/WAV
│       ├── audio_player.rs        # Pemutar audio in-app pure-Rust (Rodio + Symphonia)
│       ├── url_policy.rs          # Allowlist URL/CDN & PublicOnlyResolver (anti-SSRF/DNS rebinding)
│       ├── db.rs                  # SQLite persistence & validasi folder anti-UNC
│       └── errors.rs              # Pemetaan error terstruktur
├── CONTRIBUTING.md
├── LICENSE
├── README.md
└── SECURITY.md
```

## Keamanan, Kontribusi, & Lisensi

- Detail arsitektur keamanan & pelaporan kerentanan: [SECURITY.md](SECURITY.md)
- Panduan kontribusi: [CONTRIBUTING.md](CONTRIBUTING.md)
- Lisensi: [MIT](LICENSE)
