# MediaVault

**MediaVault** adalah aplikasi lokal untuk mengunduh **video (MP4), audio (MP3/M4A), dan foto/album (JPG/PNG/WebP/ZIP)** dari tautan media publik secara cepat, privat, dan tanpa layanan cloud pihak ketiga.

Tersedia dalam dua varian di repositori ini:
1. **MediaVault Desktop (`rust-desktop/`)** — Aplikasi Windows native ringan (**Rust + Tauri v2 + WebView2 + SQLite**), lengkap dengan installer Windows (NSIS), extractor pure-Rust berkecepatan tinggi, pemilih folder native, dan tombol pemasangan komponen 1 klik di menu Pengaturan.
2. **MediaVault Web (`frontend/` + `backend/`)** — Aplikasi browser lokal (**Next.js App Router + FastAPI + SQLite**) untuk Windows, Linux, atau WSL2.

> **Disclaimer:** Gunakan MediaVault hanya untuk konten publik yang berhak Anda unduh. Patuhi hak cipta dan ketentuan masing-masing platform. MediaVault tidak mendukung konten privat, DRM, paywall, bypass login/anti-bot, atau impor cookie browser.

---

## Platform yang Didukung (MediaVault Desktop)

| Platform | Video & Audio (MP4 / MP3 / M4A) | Foto & Album (JPG / PNG / WebP / ZIP) |
| --- | --- | --- |
| **YouTube** (`youtube.com`, `youtu.be`, Shorts) | Ya | — |
| **Instagram** (`/p/`, `/reel/`, `/tv/`, `/share/`) | Ya | Ya (tunggal & carousel ZIP) |
| **TikTok** (`tiktok.com`, `vt`/`vm`, `/photo/`) | Ya | Ya (slideshow ZIP via `gallery-dl`) |
| **X / Twitter** (`x.com`, `twitter.com`, `t.co`, quote-tweet) | Ya | Ya (tunggal & multi-foto ZIP) |
| **Threads** (`threads.com`, `threads.net`, `/share/`, `/t/`) | Ya | Ya (tunggal, carousel ZIP, quote-post) |
| **Facebook** (`watch`, `/videos/`, `/reel/`, `/photo/`, `/posts/`, `/share/`, `fb.watch`) | Ya | Ya (foto tunggal & album ZIP) |
| **Pinterest** (`/pin/`, `pin.it`) | Ya | Ya |

---

## 1. MediaVault Desktop (Windows — Direkomendasikan)

### Instalasi untuk Pengguna Baru
1. Unduh **`MediaVault_0.1.0_x64-setup.exe`** (atau versi portabel **`MediaVault.exe`**) dari halaman **GitHub Releases**.
2. Jalankan installer dan pilih lokasi instalasi yang diinginkan (tidak memerlukan hak Administrator).
3. Buka MediaVault → klik **Pengaturan** → pada panel **Dependency & Penyimpanan**, klik **Pasang yang belum ada** untuk mengunduh otomatis `yt-dlp`, `FFmpeg + ffprobe`, dan `gallery-dl` dari rilis resmi GitHub ke `%LOCALAPPDATA%\MediaVault\bin\`.

### Fitur Utama Desktop
- **Tampilan 1 halaman tanpa scroll** mengikuti standar Google Chrome Modern Web Guidance (`light-dark()` + `oklch()`, tema Gelap/Terang/Sistem, native `<dialog>`, top-layer Popover toast, custom `<select>`).
- **Custom lokasi simpan**: atur folder default di Pengaturan (**Pilih…**), pilih folder khusus per unduhan sebelum mengunduh (**Ubah…**), atau pindahkan file yang sudah selesai (**Pindahkan**) langsung dari daftar riwayat.
- **Data lokal & privat**: riwayat disimpan di `%LOCALAPPDATA%\MediaVault\app.db`, hasil unduhan default di `C:\Users\<User>\Downloads\MediaVault`.

### Build Desktop dari Source (Windows)

Prasyarat: **Rust (stable)**, **Node.js 22+**, **pnpm 10+**, dan **WebView2** (sudah terpasang bawaan di Windows 10/11).

```powershell
# Jalankan pengujian unit
node --check rust-desktop/ui/app.js
cd rust-desktop/src-tauri
cargo test

# Build installer Windows NSIS & binary rilis
cd ..
pnpm dlx @tauri-apps/cli@2 build --bundles nsis
```

Hasil build tersedia di:
- `rust-desktop/src-tauri/target/release/mediavault-desktop.exe`
- `rust-desktop/src-tauri/target/release/bundle/nsis/MediaVault_*_x64-setup.exe`

---

## 2. MediaVault Web (Next.js + FastAPI — Windows / WSL2 / Linux)

### Prasyarat
- Node.js 22+ dan pnpm 10+
- Python 3.11+
- FFmpeg & ffprobe pada `PATH` (atau atur `FFMPEG_PATH` di `.env`)

### Windows PowerShell

```powershell
Copy-Item .env.example .env
py -3 -m venv backend/.venv
.\backend\.venv\Scripts\python.exe -m pip install -r backend/requirements.txt
pnpm --dir frontend install --frozen-lockfile
.\scripts\dev.ps1
```

### Linux / WSL2

```bash
cp .env.example .env
python3 -m venv backend/.venv
backend/.venv/bin/python -m pip install -r backend/requirements.txt
pnpm --dir frontend install --frozen-lockfile
bash scripts/dev.sh
```

Buka dashboard di `http://127.0.0.1:3000` dan endpoint kesehatan backend di `http://127.0.0.1:8000/api/health`.

### Self-Host Privat via SSH Tunnel (Opsional)

Pertahankan backend dan frontend pada `127.0.0.1`, lalu akses melalui SSH port forwarding dari mesin lokal:

```powershell
ssh -N -L 3000:127.0.0.1:3000 -L 8000:127.0.0.1:8000 my-vps
```

Jika menggunakan origin HTTPS publik di `FRONTEND_ORIGINS`, backend otomatis mewajibkan konfigurasi `AUTH_USERNAME`, `AUTH_PASSWORD_HASH`, dan `AUTH_SESSION_SECRET` (lihat `.env.example` dan `scripts/set-owner-auth.py`).

### Pengujian Web App

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

---

## Rilis Otomatis (GitHub Actions)

Menandai rilis semver baru akan otomatis menjalankan pengujian, membangun installer Windows + `MediaVault.exe` + `SHA256SUMS.txt`, dan mempublikasikannya ke GitHub Releases:

```bash
git tag v0.1.0
git push origin v0.1.0
```

## Keamanan & Kontribusi

- Laporkan celah keamanan secara privat mengikuti panduan di [SECURITY.md](SECURITY.md).
- Panduan kontribusi tersedia di [CONTRIBUTING.md](CONTRIBUTING.md).

## Lisensi

Proyek ini dirilis di bawah lisensi [MIT](LICENSE).
