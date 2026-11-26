# MediaVault Desktop

**MediaVault Desktop** adalah aplikasi desktop Windows lokal yang cepat, ringan, dan privat untuk mengunduh **video (MP4), audio (MP3/M4A), serta foto & album carousel (JPG/PNG/WebP/ZIP)** dari tautan media publik — **100% berjalan di komputer Anda tanpa server cloud, tanpa akun, dan tanpa telemetri**.

[![Release](https://img.shields.io/github/v/release/masterputra169/MediaVault?style=flat-square&label=Download%20Terbaru)](https://github.com/masterputra169/MediaVault/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/masterputra169/MediaVault/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/masterputra169/MediaVault/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)

---

## Unduh MediaVault (Windows x64)

Unduh langsung dari halaman [**GitHub Releases Terbaru**](https://github.com/masterputra169/MediaVault/releases/latest):

| Paket | Deskripsi | Tautan Unduh (v0.1.0) |
| --- | --- | --- |
| **Windows Installer (Direkomendasikan)** | Installer NSIS ringan (~4,5 MB), bisa memilih lokasi instalasi, tidak memerlukan hak Administrator, lengkap dengan shortcut Start Menu & Uninstaller. | [**`MediaVault_0.1.0_x64-setup.exe`**](https://github.com/masterputra169/MediaVault/releases/download/v0.1.0/MediaVault_0.1.0_x64-setup.exe) |
| **Portable EXE** | Langsung dijalankan tanpa instalasi (~17 MB). | [**`MediaVault.exe`**](https://github.com/masterputra169/MediaVault/releases/download/v0.1.0/MediaVault.exe) |
| **Checksum SHA-256** | Verifikasi integritas file unduhan sebelum dijalankan. | [**`SHA256SUMS.txt`**](https://github.com/masterputra169/MediaVault/releases/download/v0.1.0/SHA256SUMS.txt) |

> **Verifikasi Checksum (Opsional — PowerShell):**
> ```powershell
> Get-FileHash .\MediaVault_0.1.0_x64-setup.exe -Algorithm SHA256
> ```
> Cocokkan hasil hash dengan isi [`SHA256SUMS.txt`](https://github.com/masterputra169/MediaVault/releases/download/v0.1.0/SHA256SUMS.txt).

> **Catatan Windows SmartScreen:** Karena installer open-source ini belum memakai sertifikat Code Signing berbayar, Windows SmartScreen mungkin menampilkan peringatan *"Windows protected your PC"* pada pemakaian pertama. Klik **More info → Run anyway** setelah memverifikasi checksum dari rilis resmi di repositori ini.

---

## Platform yang Didukung

Semua platform berikut mendukung **ekstraktor cepat pure-Rust** dan telah diuji secara langsung untuk konten publik:

| Platform | Jenis Tautan yang Didukung | Video & Audio (`MP4` / `MP3` / `M4A`) | Foto & Album (`JPG` / `PNG` / `WebP` / `ZIP`) |
| --- | --- | :---: | :---: |
| **YouTube** | `youtube.com/watch`, `youtu.be`, Shorts | Ya | — |
| **Instagram** | `/p/`, `/reel/`, `/tv/`, `/share/` | Ya | Ya (tunggal & carousel ZIP) |
| **TikTok** | `tiktok.com`, `vt`/`vm`, `/photo/` | Ya | Ya (slideshow ZIP via `gallery-dl`) |
| **X / Twitter** | `x.com`, `twitter.com`, `t.co`, quote-tweet | Ya | Ya (tunggal & multi-foto ZIP) |
| **Threads** | `threads.com`, `threads.net`, `/share/`, `/t/` | Ya | Ya (tunggal, carousel ZIP, quote-post) |
| **Facebook** | `watch`, `/videos/`, `/reel/`, `/photo/`, `/posts/`, `/share/`, `fb.watch` | Ya | Ya (foto tunggal & album ZIP) |
| **Pinterest** | `/pin/`, `pin.it` | Ya | Ya |

> **Disclaimer:** Gunakan MediaVault hanya untuk konten publik yang berhak Anda unduh dan patuhi ketentuan hak cipta masing-masing platform. MediaVault **tidak** mendukung bypass DRM, paywall, konten privat/wajib login, bypass anti-bot, atau impor cookie browser.

---

## Fitur Utama

- **Analisis Super Cepat (Pure-Rust Fast Path):** Analisis tautan YouTube, X, Instagram, Threads, Facebook, Pinterest, dan TikTok dilakukan langsung oleh parser Rust berkecepatan tinggi (~0,4–1,5 detik) sebelum fallback ke `yt-dlp`.
- **Kompatibilitas Video Universal (H.264 + AAC):** Mode kualitas **Terbaik** otomatis memprioritaskan resolusi & bitrate tertinggi dalam codec `avc1` (H.264) + `mp4a` (AAC) kontainer `.mp4` agar langsung bisa diputar di Windows Media Player, HP, WhatsApp, dan editor video tanpa perlu ekstensi codec AV1.
- **Pratinjau Media Terunduh Langsung di Aplikasi:** Klik tombol **Pratinjau** (atau klik thumbnail item yang selesai) di daftar unduhan untuk langsung memutar video MP4 (mendukung seeking/HTTP Range), melihat foto, menelusuri slide album `.zip` dengan tombol panah `←`/`→`, atau memutar audio MP3/M4A tanpa keluar aplikasi.
- **Installer Komponen 1-Klik + Verifikasi SHA-256:** Di menu **Pengaturan → Dependency & Penyimpanan**, pasang atau perbarui `yt-dlp`, `FFmpeg + ffprobe`, dan `gallery-dl` langsung dari rilis resmi GitHub tanpa terminal. Aplikasi mengecek versi terlebih dahulu agar tidak mengunduh ulang bila sudah terbaru, lalu memverifikasi digest **SHA-256** sebelum memasang.
- **Kontrol Folder Simpan Fleksibel:**
  - Atur folder default di **Pengaturan** (**Pilih…** dengan dialog folder native Windows).
  - Pilih folder tujuan khusus **per unduhan** sebelum menekan *Unduh sekarang* (**Ubah…**).
  - Pindahkan file yang sudah selesai ke folder lain langsung dari daftar riwayat (**Pindahkan**).
  - Tombol **Buka folder** membuka Windows Explorer dan menyorot langsung file hasil unduhan.
- **Progres Real-Time & Antrean Aman:** Menampilkan persentase, ukuran unduhan, kecepatan (`MB/s`), estimasi waktu (ETA), pembatalan instan (*Batalkan*), dan *Ulangi* untuk unduhan yang gagal/terhenti.
- **UI Modern & Aksesibel:** Tampilan 1 halaman rapi tanpa scroll berlebih dengan tema **Gelap / Terang / Sistem** (`light-dark()` + `oklch()`), native `<dialog>`, top-layer toast Popover, serta filter tab (`Semua`, `Aktif`, `Video`, `Audio`, `Foto`), pencarian judul, dan filter platform.

---

## Cara Pakai Cepat

1. **Pasang atau Jalankan MediaVault:** Jalankan `MediaVault_0.1.0_x64-setup.exe` (atau buka langsung `MediaVault.exe`).
2. **Pasang Komponen (Sekali di Awal):** Klik **Pengaturan** → pada panel **Dependency & Penyimpanan**, klik **Pasang yang belum ada**. Tunggu hingga `yt-dlp`, `FFmpeg`, `ffprobe`, dan `gallery-dl` berstatus `✓`.
3. **Unduh Media:**
   - Tempel tautan publik di kotak URL (atau klik **Tempel**) lalu klik **Analisis URL**.
   - Pilih mode **Video**, **Audio**, atau **Foto**, atur kualitas/format/slide dan folder tujuan bila perlu, lalu klik **Unduh sekarang**.
4. **Pratinjau & Kelola File:** Setelah status `✓ Selesai`, klik **Pratinjau** untuk melihat video/foto langsung, **Buka folder** untuk membuka di Explorer, atau **Pindahkan** untuk memindahkan file ke folder lain.

---

## Arsitektur & Tech Stack

Aplikasi Desktop ini dibangun dengan core **100% Rust (Tauri v2)** tanpa runtime Python maupun Node.js saat dijalankan oleh pengguna:

| Lapisan | Teknologi | Peran |
| --- | --- | --- |
| **Core Backend & Engine** | **Rust** + **Tauri v2** + **Tokio** | Validasi URL/SSRF, ekstraktor cepat 7 platform, antrean unduhan, pembatalan proses, streaming `mvmedia`, dan installer SHA-256. |
| **Database Lokal** | **SQLite** (`rusqlite` bundled) | Menyimpan riwayat unduhan dan preferensi secara persisten di mesin lokal. |
| **Antarmuka (UI)** | **HTML5 + CSS3 + Vanilla JS** di **Windows WebView2** | Tampilan native ringan tanpa bundler framework berat; seluruh data diambil dari command Rust via IPC Tauri. |
| **Tool Media Eksternal (Lokal)** | `yt-dlp`, `FFmpeg`, `ffprobe`, `gallery-dl` | Dijalankan secara terisolasi (argument array, tanpa shell) untuk ekstraksi situs kompleks, penggabungan video/audio, dan konversi MP3/M4A. |

### Lokasi Penyimpanan Data Lokal (Windows)
- **Database SQLite & preferensi:** `%LOCALAPPDATA%\MediaVault\app.db`
- **Komponen terpasang (`yt-dlp`, `ffmpeg`, `ffprobe`, `gallery-dl`):** `%LOCALAPPDATA%\MediaVault\bin\`
- **Folder unduhan default:** `C:\Users\<Username>\Downloads\MediaVault` *(dapat diubah kapan saja di Pengaturan)*

---

## Keamanan & Privasi

Kode sumber telah diaudit menyeluruh sebelum publikasi (detail lengkap di [SECURITY.md](SECURITY.md)):
- **Zero Telemetry & Zero Cloud:** Tidak ada pelacakan, analitik, atau pengiriman URL ke server pihak ketiga.
- **Anti-SSRF & DNS Rebinding:** Custom `PublicOnlyResolver` pada `reqwest`, pemblokiran seluruh IP non-publik IPv4/IPv6, serta allowlist domain platform & CDN resmi.
- **Anti-Command Injection & Binary Hijacking:** Eksekusi subprocess murni lewat argument array dengan `--ignore-config`, `YTDLP_NO_PLUGINS=1`, timeout global, batas ukuran file (2 GiB), dan pencarian biner hanya dari jalur tepercaya.
- **Verifikasi Kriptografis Installer:** Setiap unduhan `yt-dlp`, `FFmpeg`, dan `gallery-dl` diverifikasi hash **SHA-256** + `Content-Length`, dilindungi dari Zip Bomb & Zip-Slip, serta diganti secara atomik.
- **WebView2 & Path Hardening:** CSP ketat tanpa `unsafe-inline` / remote `https:` fallback, nol sink `innerHTML`/`eval`, pratinjau lokal terisolasi lewat protokol `mvmedia` berbasis UUID, serta pemblokiran path UNC (`\\server\share`) untuk mencegah kebocoran hash Net-NTLMv2.

---

## Build dari Source (Developer — Windows)

### Prasyarat
- **Windows 10 / 11 (x64)** dengan Microsoft Edge WebView2 (sudah bawaan di Windows 10/11)
- **Rust (stable)** — https://rustup.rs
- **Node.js 22+** & **pnpm 10+** (hanya diperlukan saat build CLI Tauri, tidak dipakai saat aplikasi dijalankan)

### Menjalankan Pengujian & Build

```powershell
# 1. Clone repositori
git clone https://github.com/masterputra169/MediaVault.git
cd MediaVault

# 2. Verifikasi sintaks UI & jalankan seluruh unit test Rust (offline)
node --check rust-desktop/ui/app.js
cd rust-desktop/src-tauri
cargo test --locked

# 3. Pasang Tauri CLI versi ter-pin & build installer NSIS + portable EXE
cd ..
pnpm install --frozen-lockfile
node node_modules/@tauri-apps/cli/tauri.js build --bundles nsis -- --locked
```

Hasil kompilasi tersedia di:
- `rust-desktop/src-tauri/target/release/mediavault-desktop.exe`
- `rust-desktop/src-tauri/target/release/bundle/nsis/MediaVault_0.1.0_x64-setup.exe`

---

## Rilis Otomatis (GitHub Actions)

Menandai tag semver baru otomatis menjalankan seluruh tes, membangun installer NSIS + `MediaVault.exe` + `SHA256SUMS.txt` di runner Windows terisolasi (`contents: read`), lalu mempublikasikan rilis melalui job terpisah (`contents: write`):

```bash
git tag -a v0.1.1 -m "MediaVault v0.1.1"
git push origin v0.1.1
```

## Struktur Repositori

```text
MediaVault/
├── .github/
│   ├── dependabot.yml                  # Pembaruan dependensi mingguan (Cargo, npm CLI, Actions)
│   └── workflows/
│       ├── ci.yml                      # CI otomatis: cek sintaks UI & cargo test --locked
│       └── release-windows.yml         # Build NSIS + portable EXE + SHA256SUMS & GitHub Release
├── rust-desktop/
│   ├── package.json & pnpm-lock.yaml   # Versi ter-pin @tauri-apps/cli
│   ├── src-tauri/
│   │   ├── Cargo.toml & Cargo.lock
│   │   ├── tauri.conf.json             # Konfigurasi Tauri v2, CSP ketat, & bundler NSIS
│   │   └── src/
│   │       ├── lib.rs                  # Tauri IPC commands & custom protocol mvmedia
│   │       ├── engine.rs               # Analisis cepat, antrean unduhan, yt-dlp & FFmpeg
│   │       ├── images.rs               # Ekstraktor foto/video pure-Rust & PreviewCache
│   │       ├── installer.rs            # Installer 1-klik yt-dlp/FFmpeg/gallery-dl + SHA-256
│   │       ├── url_policy.rs           # Allowlist URL/CDN & PublicOnlyResolver (anti-SSRF)
│   │       ├── tools.rs                # Deteksi tool aman & isolasi environment subprocess
│   │       ├── db.rs                   # SQLite persistence & validasi folder anti-UNC
│   │       └── errors.rs               # Pemetaan kode error terstruktur
│   └── ui/
│       ├── index.html                  # Markup semantik & native <dialog> / Popover API
│       ├── styles.css                  # Design system oklch() + light-dark()
│       └── app.js                      # Logika UI WebView2 & pemutar pratinjau media
├── CONTRIBUTING.md
├── LICENSE
├── README.md
└── SECURITY.md
```

## Kontribusi, Keamanan, & Lisensi

- Laporkan celah keamanan secara privat mengikuti panduan di [SECURITY.md](SECURITY.md).
- Panduan kontribusi tersedia di [CONTRIBUTING.md](CONTRIBUTING.md).
- Dirilis di bawah lisensi [MIT](LICENSE).
