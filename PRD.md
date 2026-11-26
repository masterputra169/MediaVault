# Product Requirements Document (PRD)
# MediaVault — Personal Multi-Platform Media Downloader

**Versi PRD:** 1.0  
**Tanggal:** 25 November 2026  
**Status:** Ready for implementation by Codex  
**Target penggunaan:** Pribadi, satu pengguna, lokal (localhost)  
**Prioritas:** Aplikasi berfungsi end-to-end, bukan sekadar mockup  
**Platform utama:** Desktop browser pada Windows 10/11 atau Linux/WSL2  
**Bahasa UI:** Indonesia (default), struktur siap untuk i18n di masa depan

---

## 1. Ringkasan Eksekutif

MediaVault adalah aplikasi web lokal untuk menyimpan video dan audio dari URL media sosial ke folder pribadi di komputer pengguna. Pengguna cukup menempel URL, memeriksa informasi media, memilih format/kualitas yang tersedia, lalu memulai unduhan dan memantau progres. Riwayat unduhan dan metadata disimpan di SQLite lokal.

**Keputusan desain:**
- Local-first: tidak perlu hosting, login, akun, database cloud, subscription, atau telemetry.
- Frontend modern: Next.js App Router + TypeScript + Tailwind CSS.
- Backend: Python 3.11+ + FastAPI.
- Engine utama: yt-dlp; FFmpeg/ffprobe untuk penggabungan dan konversi, beserta komponen yt-dlp-ejs dan JS runtime kompatibel untuk dukungan YouTube yang lengkap.
- Data: SQLite; file media disimpan di direktori lokal yang dapat dikonfigurasi melalui environment.
- Satu backend worker process dengan antrean terkontrol. Tidak perlu Celery, Redis, atau Docker untuk MVP.
- Tanpa integrasi Cobalt sebagai dependency MVP; evaluasi adapter alternatif hanya jika dibutuhkan dan lisensinya diperiksa.

## 2. Latar Belakang dan Masalah

Pengunduh media sosial berbasis web publik sering mengandung iklan, redirect, batas kualitas, atau harus mengunggah URL ke server pihak ketiga. Untuk kebutuhan pribadi, pengguna menginginkan alternatif yang dapat dioperasikan sendiri dan menyimpan berkas secara lokal.

Masalah utama yang diselesaikan:
1. Proses download dari beberapa platform menggunakan antarmuka yang konsisten.
2. Memilih keluaran video atau audio dengan resolusi/format sesuai sumber.
3. Mengetahui status dan menyimpan riwayat unduhan lokal.
4. Mengurangi ketergantungan pada situs downloader publik.

## 3. Tujuan, KPI, dan Bukan Tujuan

### 3.1 Tujuan produk

- **G1:** URL publik yang didukung dapat dianalisis sehingga pengguna melihat metadata ringkas sebelum mengunduh.
- **G2:** Video/audio dapat disimpan ke folder lokal melalui proses unduh end-to-end.
- **G3:** Download memiliki status jelas: queued, downloading, processing, completed, failed, cancelled, interrupted.
- **G4:** Aplikasi dapat dimulai oleh satu orang dengan instruksi setup sederhana.
- **G5:** Aplikasi tidak melakukan upload file, analitik, atau mengirim riwayat ke server milik pembuat aplikasi.

### 3.2 Indikator keberhasilan (pengujian, bukan janji kompatibilitas)

| ID | Ukuran | Target |
|---|---|---|
| KPI-01 | Frontend + backend startup lokal | Lulus pada instalasi bersih sesuai README |
| KPI-02 | Health check dependency | Memperlihatkan status yt-dlp, FFmpeg, JS runtime, dan direktori unduhan |
| KPI-03 | Happy path download | 1 URL media publik yang legal dan kompatibel berhasil tersimpan dengan file valid |
| KPI-04 | Error handling | Invalid URL, situs tidak didukung, file tidak tersedia, dan jaringan gagal menghasilkan pesan yang dapat dimengerti |
| KPI-05 | Riwayat | Tetap ada sesudah browser refresh dan restart server |
| KPI-06 | Cancel queued / running | Berhenti tanpa meninggalkan job hidup; temp file dibersihkan atau ditandai dengan aman |
| KPI-07 | Keamanan lokal | Server hanya bind `127.0.0.1` secara default; tidak menerima akses origin arbitrer |
| KPI-08 | Quality gate | Lint, typecheck, unit/integration test, dan production build frontend lulus |

### 3.3 Bukan tujuan (out of scope MVP)

- Mengunduh konten DRM, paywalled, private tanpa izin, menghindari autentikasi, atau mengakali pembatasan akses.
- Mendukung **semua** situs tanpa pengecualian; ketersediaan ditentukan extractor dan perubahan situs.
- Menjalankan aplikasi sebagai layanan publik, multi-user, atau expose ke internet.
- Dukungan playlist/channel massal, batch dari file CSV, live-stream recording, penjadwalan, extension browser, aplikasi mobile native, cloud sync, dan AI classification.
- Mengunduh cerita yang menghilang, private account, atau media yang membutuhkan login pada MVP.
- Menghapus watermark secara paksa atau mengklaim bahwa semua platform mendukung keluaran bebas watermark.

## 4. Target Pengguna dan Lingkungan

**Persona tunggal:** pengembang yang ingin mengunduh media yang berhak ia simpan untuk referensi pribadi, dengan kenyamanan UI web lokal.

**Perangkat:** desktop/laptop pengguna. Mendukung Windows PowerShell serta Linux/WSL2. Browser modern (Chrome, Edge, Firefox).

**Koneksi:** internet diperlukan hanya untuk mengambil metadata/media dari platform asal; interaksi aplikasi dan penyimpanan berlangsung lokal.

**Privasi:** aplikasi bukan layanan remote. Tidak ada akun, pengukuran perilaku, iklan, atau ketergantungan API downloader berbayar.

## 5. Dukungan Platform dan Batasan

Dukungan berikut adalah **target berbasis kemampuan yt-dlp**, bukan jaminan bahwa setiap URL akan berhasil pada semua waktu:

| Platform | Tipe media target MVP | Catatan |
|---|---|---|
| YouTube | Video tunggal, Shorts; audio extraction | Perubahan sisi platform dapat menuntut update yt-dlp, yt-dlp-ejs, dan runtime JS |
| TikTok | Video publik | Ketersediaan regional/autentikasi bisa membatasi |
| Instagram | Reels dan video post publik | Sebagian konten mungkin meminta login atau diblokir platform |
| Facebook | Video publik | Link share/redirect dapat berubah; jangan menjanjikan semua tipe |
| X (Twitter) | Video post publik | Konten login-gated tidak dijamin |
| Reddit | Video post publik | Track audio/video dapat perlu merge via FFmpeg |
| Pinterest | Video pin publik | Tergantung extractor dan tipe pin |
| Vimeo | Video publik | Pembatasan pemilik video dihormati |
| Threads | Video post publik (best effort) | Validasi nyata sebelum diklaim supported |

**Ketentuan kompatibilitas:**
- Backend memetakan platform yang dikenali untuk label/ikon; engine tetap harus melaporkan hasil sebenarnya.
- Jika extractor tidak menyediakan media/format, UI menyatakan ketidaktersediaan, bukan memaksa.
- Default `noplaylist=true`; URL playlist/channel ditolak dengan pesan eksplisit, atau hanya video tunggal jika diketahui secara pasti.
- Konten publik belum tentu bebas hak cipta. Aplikasi digunakan hanya untuk media yang pengguna miliki atau berhak unduh, sesuai aturan platform.
- Tidak membypass DRM, login, akun privat, paywall, atau pembatasan akses.

## 6. Alur Pengguna

### 6.1 Alur utama

1. Pengguna membuka `http://127.0.0.1:3000`.
2. Dashboard menampilkan status backend dan kolom untuk menempelkan URL.
3. Pengguna menempel URL dari platform target.
4. Frontend melakukan validasi sederhana (tidak ada permintaan sampai URL masuk akal).
5. Pengguna klik **Analisis URL**.
6. Backend memvalidasi URL, memeriksa domain/protokol, lalu meminta metadata via yt-dlp tanpa download.
7. UI menampilkan thumbnail jika aman/tersedia, judul, pembuat, durasi, platform, dan kualitas yang benar-benar tersedia.
8. Pengguna memilih **Video** atau **Audio**, kualitas, dan format yang didukung.
9. Pengguna klik **Unduh Sekarang**.
10. Backend membuat job `queued`; worker memulai download dan menyiarkan status melalui polling.
11. UI menampilkan persentase, ukuran, kecepatan, ETA jika tersedia, dan status post-processing.
12. Setelah selesai, file disimpan di direktori lokal; UI menyediakan **Unduh ke Browser** dan **Lihat Lokasi File** (lokasi ditampilkan dan disalin, tidak mencoba membuka folder via browser yang tidak didukung).
13. Riwayat dapat dilihat, dicari, difilter, diulang, atau dihapus.

### 6.2 Alur alternatif

- URL invalid: tetap di input, tampilkan alasan singkat.
- Metadata tidak tersedia: tampilkan kode error dan saran perbaikan, jangan menampilkan kualitas palsu.
- Kualitas yang dipilih hilang pada saat download: fallback ke kualitas lebih rendah **hanya** jika pengguna mengaktifkan fallback; default berikan pesan dan opsi ulang.
- Unduhan terputus: job ditandai `failed`; pengguna bisa retry.
- Restart aplikasi saat job berjalan: job lama ditandai `interrupted`; tidak dianggap selesai; retry manual.
- Disk penuh: job gagal secara aman, file sementara tidak ditampilkan sebagai output berhasil.

## 7. Fitur Fungsional dan Acceptance Criteria

### FR-01 — URL input dan validasi (P0)

**Kebutuhan:** input URL dengan tombol paste, clear, dan state loading. Terima hanya URL HTTPS/HTTP dari domain media sosial yang secara eksplisit didukung.

**Acceptance:**
- URL kosong/invalid ditolak tanpa request.
- URL `file:`, `ftp:`, `javascript:`, `data:`, kredensial di URL, localhost, IP literal, internal host, atau domain tak dikenal ditolak.
- Domain dicek dengan parser hostname yang benar, bukan `includes('youtube.com')`; subdomain harus sesuai aturan suffix per domain.
- Alias short-link resmi seperti `youtu.be`/`vm.tiktok.com` dapat diterima bila terdaftar, tetapi redirect ke target terlarang harus diblokir sebisa mungkin.
- Batas panjang URL misalnya 2.048 karakter; error jelas dan aksesibilitas label input baik.

### FR-02 — Analisis metadata sebelum download (P0)

**Kebutuhan:** ekstraksi tanpa mengunduh file media.

**Tampilkan bila tersedia:** judul, uploader/channel, durasi, thumbnail, platform, source URL, tipe media, jumlah format yang ditemukan, estimasi ukuran (opsional dan diberi label estimasi).

**Acceptance:**
- Metadata ter-normalisasi; internal extractor payload tidak disalurkan mentah ke browser.
- Tidak menampilkan informasi sensitif seperti cookie, signed URL, headers autentikasi, path sistem, atau traceback.
- Jika thumbnail rusak/null, tampilkan fallback.
- Durasi tidak tersedia dinyatakan `Tidak diketahui`, bukan 00:00.
- Timeout metadata dapat dikonfigurasi, default 45 detik.

### FR-03 — Pilih output video atau audio (P0)

**Video:** format container default MP4 jika kompatibel; opsi kualitas `Best`, `1080p`, `720p`, `480p`, `360p` ditampilkan hanya jika tersedia/layak. Jangan upscale sumber. Jika pilihan MP4 tidak mungkin tanpa transcode, berikan fallback yang jelas atau penjelasan format aktual.

**Audio:** MP3 default (melalui FFmpeg), pilihan M4A bila didukung; bitrate MP3 default 192 kbps, bersifat pengaturan encode, **bukan** jaminan kualitas sumber.

**Acceptance:**
- Pilihan di UI dibatasi hasil metadata; backend juga validasi ulang.
- Tidak mengiklankan 1080p jika sumber hanya 720p.
- Bisa menggabungkan video/audio terpisah jika FFmpeg tersedia.
- UI menyatakan `processing` saat merge/convert setelah unduhan selesai.
- Operasi dengan FFmpeg tidak tersedia harus gagal dengan pesan actionable jika memang diperlukan, bukan file rusak yang dianggap sukses.

### FR-04 — Job queue, progress, cancel, retry (P0)

**Kebutuhan:** antrean in-process persisten metadata dengan default maksimal 2 job berjalan serentak; maksimal 20 queued. Perubahan setting concurrency dari env. Download dijalankan sebagai subprocess terisolasi agar benar-benar dapat dihentikan.

**Acceptance:**
- Status valid: `queued`, `downloading`, `processing`, `completed`, `failed`, `cancelled`, `interrupted`.
- ID UUID; status/progress tersimpan di SQLite.
- Persentase 0–100 hanya jika total bytes diketahui; jika tidak, indeterminate.
- Kecepatan dan ETA ditampilkan hanya jika ada data; progress tidak boleh mundur tiba-tiba hanya karena fase merge.
- `Cancel` pada job running menghentikan process group subprocess dengan mekanisme lintas platform; pada queued langsung membatalkan.
- Retry membuat **job baru** yang mereferensi `retry_of` pada job sebelumnya; job lama tidak diubah menjadi sukses.
- Backend tidak menjalankan beberapa Uvicorn worker untuk job queue in-memory.
- Setelah restart, job yang semula running/processing/queued ditandai `interrupted`, dan bisa diulang secara manual.

### FR-05 — Berkas output lokal (P0)

**Kebutuhan:** file hasil berada di `DOWNLOAD_DIR` dari environment; default `<project>/storage/downloads`.

**Acceptance:**
- Setiap job memiliki folder staging sendiri `<project>/storage/tmp/<job_id>`.
- Hasil akhir dipindahkan/rename secara atomik jika memungkinkan ke download directory setelah post-processing sukses.
- Nama output aman dan deterministik: `<sanitized-title>_<media-id>_<short-job-id>.<ext>`.
- Nama tidak mengandung path traversal/karakter terlarang Windows; panjang nama dibatasi.
- Duplikasi nama tidak overwrite berkas lama.
- Berkas `.part` dan temporary tidak ditampilkan sebagai `completed`.
- Endpoint hasil hanya membaca file yang tercatat pada job dan berada di bawah direktori output yang diizinkan.
- Delete history menyediakan pilihan **hapus hanya catatan** (default) atau **hapus catatan dan file** dengan konfirmasi; jika file hilang di luar aplikasi, UI menampilkan `File tidak ditemukan`.

### FR-06 — Dashboard dan riwayat (P0)

**Kebutuhan:** satu halaman dashboard utama serta halaman history/settings sederhana.

**Acceptance:**
- Dashboard menampilkan input URL, media preview, pilihan format, antrean download terbaru.
- History menunjukkan thumbnail opsional, judul, platform, tipe, ukuran final, waktu, status, dan tindakan.
- Pencarian judul, filter status/platform, sorting terbaru dulu.
- Refresh browser tidak menghapus status/history.
- Empty/loading/error/success states jelas.
- Histori dipaginate atau dibatasi 50 per halaman agar performa terjaga.

### FR-07 — Pengaturan lokal dan kesehatan dependency (P0)

**Kebutuhan:** status dependency, lokasi penyimpanan, default quality/audio, dan concurrency (bila dibuka melalui config).

**Acceptance:**
- `GET /api/health` mengecek versi Python, yt-dlp, FFmpeg/ffprobe, JS runtime dan ketersediaan write storage.
- Nilai path download ditampilkan; perubahan root directory cukup dilakukan melalui `.env` dan restart pada MVP (tidak perlu native folder picker).
- Data seperti absolute filesystem path hanya ditampilkan di localhost, tidak diekspos ke jaringan.
- `Update yt-dlp` melalui petunjuk manual di README; **tidak** memberi backend hak mengeksekusi arbitrary package manager command dari UI.
- Tidak perlu restart server hanya untuk mengganti default kualitas UI bila tersimpan sebagai setting lokal yang aman.

### FR-08 — Pesan error yang berguna (P0)

**Kebutuhan:** error yang singkat, dikategorikan, dan tidak membocorkan stack trace.

**Acceptance:**
- Mapping error code: `INVALID_URL`, `UNSUPPORTED_PLATFORM`, `UNAVAILABLE_MEDIA`, `LOGIN_REQUIRED`, `GEO_RESTRICTED`, `FORMAT_UNAVAILABLE`, `EXTRACTOR_FAILED`, `NETWORK_ERROR`, `TIMEOUT`, `FFMPEG_MISSING`, `DISK_FULL`, `JOB_NOT_FOUND`, `FILE_NOT_FOUND`, `QUEUE_FULL`, `INTERNAL_ERROR`.
- API mengembalikan `{code, message, retryable, details?}`; `details` hanya informasi non-sensitif.
- Backend memiliki logging teknis untuk debugging, dengan URL query token/cookie disamarkan dan tidak pernah menyimpan secret mentah.

### FR-09 — UI/UX dan aksesibilitas (P1 untuk polish, P0 untuk fungsi)

**Gaya:** dark theme default, fokus sederhana dan desktop-first; jangan meniru logo/brand situs lain. Responsif di mobile browser.

**Acceptance:**
- Komponen UI: Header/health badge, URL input besar, preview card, format controls, download card, recent jobs, history list, settings panel, toast.
- Tombol memiliki disabled/loading yang benar; tidak memungkinkan submit ganda tanpa sengaja.
- Keyboard navigation, label input, focus rings, aria-live untuk update progres, dan kontras yang nyaman.
- Layout nyaman pada 360px, 768px, 1280px.
- Animasi ringan; hormati `prefers-reduced-motion`.

### FR-10 — Peningkatan setelah MVP (P2, jangan menunda MVP)

- Bulk URL input 5–20 baris dengan validasi per item.
- Playlist dengan persetujuan jumlah video dan batas kapasitas.
- Subtitle download dan metadata export JSON/CSV.
- Advanced filters dan pengaturan naming template.
- Opsional browser cookie import milik pengguna, **hanya** untuk konten yang berhak diakses; default nonaktif, dan harus melalui desain privasi tambahan.
- Adapter engine kedua hanya setelah ketentuan lisensi dan stabilitas jelas.

## 8. Rancangan UI dan Informasi

### 8.1 Routing halaman

- `/` : Dashboard.
- `/history` : daftar unduhan dengan search/filter.
- `/settings` : dependency health, display storage path, preferensi default, troubleshooting.
- Halaman 404 standar.

### 8.2 Dashboard (desktop)

```text
┌────────────────────────────────────────────────────────────────┐
│ MediaVault              Dashboard  History  Settings   ● Local │
├────────────────────────────────────────────────────────────────┤
│ Download from your favorite platforms                           │
│ [ https://www.youtube.com/watch?v=...               ] [Analyze]│
│                                                                │
│ ┌──────────── preview ──────────┐ ┌──── Download Options ────┐  │
│ │ thumbnail                   │ │ Format   Video | Audio    │  │
│ │ title, channel, duration    │ │ Quality  Best / 1080/... │  │
│ │ source platform            │ │ Container MP4 / MP3      │  │
│ └─────────────────────────────┘ │ [ Download now ]         │  │
│                                 └───────────────────────────┘  │
├────────────────────────────────────────────────────────────────┤
│ Active Downloads                                               │
│ [title]    downloading  47% [██████░░░░░░]  Cancel             │
│ [title]    processing   ...                   Cancel            │
├────────────────────────────────────────────────────────────────┤
│ Recent History              [Search] [Platform] [Status]       │
│ [file] [platform] [size] [status] [Open/download] [Delete]      │
└────────────────────────────────────────────────────────────────┘
```

### 8.3 Design tokens

- Font: `Geist` atau system sans-serif fallback.
- Background: `#0B1020`, surface `#141B2D`, border `#293246`, text utama `#F3F6FC`.
- Accent: `#6D8CFF`; success `#41C48B`; warning `#F3B65F`; error `#EF7474`.
- Radius 10–14px; spacing konsisten 4/8/12/16/24/32.
- Hindari glassmorphism berat, gradien berlebihan, atau efek dekoratif yang mengganggu readability.

## 9. Arsitektur Teknis

```text
Browser (127.0.0.1:3000)
           │ fetch JSON + polling status
           ▼
Next.js App Router (TypeScript, Tailwind)
           │ calls http://127.0.0.1:8000
           ▼
FastAPI (127.0.0.1:8000, strict local origin)
           ├── URL policy & metadata normalization
           ├── Job service + SQLite persistence
           ├── In-process bounded job dispatcher (single API process)
           ├── yt-dlp extractor for inspect (bounded timeout)
           ├── yt-dlp subprocess per download + progress parser
           ├── FFmpeg/ffprobe (through yt-dlp postprocessors)
           └── Local filesystem: storage/tmp, storage/downloads
```

**Pilihan penting:** Jangan mencoba mengeksekusi yt-dlp dalam browser. Hindari Next.js API routes sebagai downloader utama. Python backend menjadi satu sumber kebenaran status job dan berkas output.

### 9.1 Alasan desain backend

- Metadata dapat dianalisis memakai Python API `yt_dlp.YoutubeDL().extract_info(download=False)` dengan timeout yang dipaksakan pada operasi terisolasi; gunakan sanitasi payload.
- Download jangka panjang **sebaiknya subprocess** `sys.executable -m yt_dlp` dengan argument list (tanpa `shell=True`), karena Cancel harus dapat mengakhiri proses secara deterministik.
- Parse progres dari output terstruktur `--progress-template` + `--newline`/`--print` atau mekanisme library yang terbukti stabil, bukan regex rapuh atas output human-readable. Bentuk progress dapat berbeda antar tahap; jangan mengasumsikan ETA/percent selalu ada.
- Konfigurasi FFmpeg binary via PATH/env.
- Worker queue memakai semaphore + thread/pump sederhana; satu proses API saja untuk MVP. SQLAlchemy/SQLModel atau SQLite stdlib diperbolehkan, asal migrasi dan transaksi jelas.
- SQLite WAL opsional demi read/write job updates; hindari menulis status pada setiap byte, throttle misalnya tiap 500ms atau perubahan >=1%.

### 9.2 Paket inti

| Area | Pilihan |
|---|---|
| Frontend | Next.js App Router, React, TypeScript, Tailwind CSS |
| UI | Komponen ringan, lucide-react opsional |
| HTTP Client | native fetch + typed contract |
| Backend | FastAPI, Uvicorn, Pydantic v2 |
| Download | yt-dlp (`yt-dlp[default]` jika sesuai paket terbaru), yt-dlp-ejs, JS runtime kompatibel |
| Media | FFmpeg + ffprobe executable |
| DB | SQLite (satu file lokal), migrasi SQL yang sederhana |
| Testing | pytest + httpx; frontend Vitest/RTL atau minimal smoke/playwright |
| Package manager | pnpm frontend, pip/venv backend |

**Jangan hardcode angka versi yang tidak diverifikasi.** Pakai versi stabil kompatibel dan lock dependency frontend; dokumentasikan versi yang berhasil diuji. yt-dlp saat PRD ditulis mendukung Python 3.11+.

## 10. Struktur Repository yang Diminta

```text
mediavault/
├── AGENTS.md
├── PRD.md
├── README.md
├── .gitignore
├── .env.example
├── frontend/
│   ├── package.json
│   ├── pnpm-lock.yaml
│   ├── tsconfig.json
│   ├── next.config.ts
│   ├── src/
│   │   ├── app/
│   │   │   ├── page.tsx
│   │   │   ├── history/page.tsx
│   │   │   ├── settings/page.tsx
│   │   │   ├── layout.tsx
│   │   │   └── globals.css
│   │   ├── components/
│   │   │   ├── url-input.tsx
│   │   │   ├── media-preview.tsx
│   │   │   ├── format-picker.tsx
│   │   │   ├── download-progress.tsx
│   │   │   ├── history-table.tsx
│   │   │   └── app-shell.tsx
│   │   ├── lib/api.ts
│   │   ├── lib/types.ts
│   │   └── hooks/use-downloads.ts
│   └── ...
├── backend/
│   ├── requirements.txt (atau pyproject.toml + lock)
│   ├── app/
│   │   ├── main.py
│   │   ├── config.py
│   │   ├── schemas.py
│   │   ├── api/
│   │   │   ├── health.py
│   │   │   ├── media.py
│   │   │   ├── downloads.py
│   │   │   └── settings.py
│   │   ├── services/
│   │   │   ├── url_policy.py
│   │   │   ├── extractor.py
│   │   │   ├── downloader.py
│   │   │   ├── jobs.py
│   │   │   ├── files.py
│   │   │   └── dependency_checks.py
│   │   ├── db/
│   │   │   ├── connection.py
│   │   │   └── migrations/
│   │   └── utils/
│   └── tests/
│       ├── test_url_policy.py
│       ├── test_media_api.py
│       ├── test_jobs_api.py
│       ├── test_files_security.py
│       └── test_recovery.py
├── scripts/
│   ├── dev.ps1
│   ├── dev.sh
│   └── check-deps.py
└── storage/                 # .gitignored
    ├── downloads/
    ├── tmp/
    └── app.db
```

Struktur dapat sedikit disesuaikan ketika implementasi bila ada alasan teknis jelas; separation of concerns wajib dipertahankan.

## 11. Kontrak API REST

**Base URL:** `http://127.0.0.1:8000/api`  
**Content type:** JSON untuk operasi metadata/job; endpoint file adalah binary response.  
**CORS:** hanya origin `http://127.0.0.1:3000` dan `http://localhost:3000` (bisa dikonfigurasi eksplisit).  
**Response error:** konsisten, tidak membocorkan exception.

### 11.1 Health

`GET /api/health`

Response 200 (contoh; status bisa `degraded` jika FFmpeg hilang):

```json
{
  "status": "ok",
  "app_version": "0.1.0",
  "dependencies": {
    "yt_dlp": {"available": true, "version": "detected"},
    "ffmpeg": {"available": true, "version": "detected"},
    "ffprobe": {"available": true, "version": "detected"},
    "javascript_runtime": {"available": true, "name": "node", "version": "detected"},
    "yt_dlp_ejs": {"available": true, "version": "detected"}
  },
  "storage": {"writable": true, "download_dir": "/path/to/downloads"}
}
```

### 11.2 Analyze media

`POST /api/media/analyze`

Request:

```json
{"url": "https://www.youtube.com/watch?v=EXAMPLE"}
```

Response 200:

```json
{
  "url": "https://www.youtube.com/watch?v=EXAMPLE",
  "platform": "youtube",
  "title": "Example Video",
  "uploader": "Example Channel",
  "duration_seconds": 120,
  "thumbnail_url": "https://example.invalid/thumbnail.jpg",
  "media_id": "EXAMPLE",
  "available_qualities": ["best", "1080", "720", "480"],
  "has_audio": true,
  "supports_mp4": true,
  "estimated_size_bytes": null,
  "warnings": []
}
```

Response 422 / 502 / 504 untuk URL invalid atau extractor error (gunakan kode error standar, bukan 200 palsu).

**Penting:** `thumbnail_url` di atas hanya contoh struktur, bukan janji URL valid. Jangan meneruskan seluruh objek `info_dict` yt-dlp kepada client.

### 11.3 Create download

`POST /api/downloads`

```json
{
  "url": "https://www.youtube.com/watch?v=EXAMPLE",
  "kind": "video",
  "quality": "720",
  "container": "mp4",
  "allow_quality_fallback": false
}
```

Untuk audio:

```json
{
  "url": "https://www.youtube.com/watch?v=EXAMPLE",
  "kind": "audio",
  "quality": "best",
  "container": "mp3",
  "audio_bitrate_kbps": 192
}
```

Response 202:

```json
{
  "id": "uuid",
  "status": "queued",
  "created_at": "2026-11-25T00:00:00Z"
}
```

### 11.4 List and inspect jobs

- `GET /api/downloads?limit=50&offset=0&status=completed&search=...&platform=...`
- `GET /api/downloads/{job_id}`

Response item example:

```json
{
  "id": "uuid",
  "url": "https://www.youtube.com/watch?v=EXAMPLE",
  "title": "Example Video",
  "platform": "youtube",
  "kind": "video",
  "quality": "720",
  "container": "mp4",
  "status": "downloading",
  "progress_percent": 42.5,
  "downloaded_bytes": 43000000,
  "total_bytes": 101000000,
  "speed_bytes_per_second": 1200000,
  "eta_seconds": 48,
  "file_size_bytes": null,
  "output_filename": null,
  "error": null,
  "created_at": "2026-11-25T00:00:00Z",
  "updated_at": "2026-11-25T00:00:08Z",
  "completed_at": null,
  "retry_of": null
}
```

Progress fields dapat null. API juga dapat mengembalikan `total` dan pagination metadata.

### 11.5 Job actions

- `POST /api/downloads/{job_id}/cancel` → 202 bila pembatalan diterima, 409 bila status tak bisa dibatalkan.
- `POST /api/downloads/{job_id}/retry` → 202 dengan **ID job baru**.
- `GET /api/downloads/{job_id}/file` → `FileResponse` + content-disposition attachment, hanya jika `completed` dan file masih ada.
- `DELETE /api/downloads/{job_id}?delete_file=false` → 204; jika `delete_file=true`, lakukan validasi lokasi file dan konfirmasi client terlebih dulu. Jangan menghapus job active tanpa cancel.
- `GET /api/settings` → safe preferences, display download path, max concurrency.
- `PATCH /api/settings` → hanya key preferensi yang di-allowlist; tidak menerima arbitrary command/path.

### 11.6 Standard error schema

```json
{
  "error": {
    "code": "FORMAT_UNAVAILABLE",
    "message": "Resolusi pilihan tidak tersedia dari sumber.",
    "retryable": true,
    "details": null
  }
}
```

### 11.7 Polling

Frontend mem-poll `GET /api/downloads` setiap sekitar 1–2 detik **hanya ketika ada job aktif**, kemudian melambat atau berhenti saat idle/tab tersembunyi. Gunakan abort controller atau cache library dengan cleanup yang benar. Tidak perlu WebSocket/SSE pada MVP.

## 12. Model Data SQLite

### 12.1 Tabel `downloads`

| Kolom | Tipe | Keterangan |
|---|---|---|
| id | TEXT PK | UUID |
| source_url | TEXT | Original URL tervalidasi (hindari menyimpan query secret; redaksi bila perlu) |
| platform | TEXT | youtube/tiktok/etc |
| media_id | TEXT nullable | ID dari extractor |
| title | TEXT nullable | Judul yang disanitasi untuk display |
| uploader | TEXT nullable | Pembuat konten |
| thumbnail_url | TEXT nullable | URL thumbnail terpilih, bukan auth URL |
| kind | TEXT | video/audio |
| quality | TEXT | best/1080/720/480/360 |
| container | TEXT | mp4/mp3/m4a sesuai pilihan |
| audio_bitrate_kbps | INTEGER nullable | Opsi encode audio |
| allow_quality_fallback | INTEGER | 0/1 |
| status | TEXT | enum job |
| progress_percent | REAL nullable | 0..100 atau null |
| downloaded_bytes | INTEGER nullable | Progress |
| total_bytes | INTEGER nullable | Progress |
| speed_bytes_per_second | REAL nullable | Progress |
| eta_seconds | INTEGER nullable | Progress |
| output_path | TEXT nullable | Path internal server, **jangan** kirim ke semua API publik |
| output_filename | TEXT nullable | Nama file display |
| file_size_bytes | INTEGER nullable | Ukuran file final |
| error_code | TEXT nullable | Kode error |
| error_message | TEXT nullable | Pesan aman |
| retry_of | TEXT nullable | ID job sebelumnya |
| created_at | TEXT | ISO 8601 UTC |
| updated_at | TEXT | ISO 8601 UTC |
| completed_at | TEXT nullable | ISO 8601 UTC |

**Index:** `(created_at DESC)`, `(status)`, `(platform)`. Gunakan migration version table. Simpan output_path hanya untuk backend, validasi ulang pada akses.

### 12.2 Tabel `settings`

Key/value terbatas dengan validasi schema (default video quality, default audio format, theme). Jangan menaruh credential/cookie di tabel ini.

### 12.3 Lifecycle

`queued → downloading → processing → completed`  
`queued/downloading/processing → failed/cancelled/interrupted`  
`retry → job baru queued`.

Tidak ada transisi dari `completed` kembali ke `downloading` pada job yang sama.

## 13. Detail Engine Downloader

### 13.1 Dependency preflight

Saat backend startup:
1. Cek Python minimum 3.11.
2. Cek import yt-dlp dan versi.
3. Cek executable `ffmpeg` + `ffprobe` pada PATH atau lokasi env.
4. Cek runtime JavaScript kompatibel (`deno` atau `node`), serta `yt-dlp-ejs` untuk dukungan YouTube yang lengkap.
5. Cek hak tulis `DOWNLOAD_DIR` dan `TEMP_DIR`.
6. Cek kapasitas kosong bila OS menyediakan statistik.
7. Jika dependency tidak lengkap, API tetap hidup dengan status `degraded`, tetapi tombol download atau opsi bergantung dependency dinonaktifkan dengan petunjuk.

### 13.2 File lifecycle

1. Create job + validate policy.
2. Reserve staging directory by job ID.
3. Start yt-dlp subprocess using explicit args. Set `--no-playlist`, safe `--paths`, output template in staging, merge/output format berdasarkan opsi.
4. Track stdout/stderr tanpa mencetak secret. Batasi log buffer.
5. Capture progress template dan phase.
6. Setelah exit code 0, periksa file final dengan `stat()`; gunakan ffprobe bila tersedia untuk basic validation; jangan tandai selesai jika hasil kosong/invalid.
7. Sanitasi nama, pindahkan hasil akhir ke direktori final tanpa overwrite.
8. Simpan metadata final dan `completed` dalam SQLite.
9. Bersihkan staging sesuai kebijakan; cancellation/failed temp cleanup tidak boleh menghapus file yang bukan milik job.

### 13.3 Download constraints

- `MAX_ACTIVE_DOWNLOADS=2` default.
- `MAX_QUEUE_LENGTH=20` default.
- `MAX_FILE_SIZE_MB=2048` default; if platform cannot report size early, enforce during progress / filesystem checks as feasible and fail safely. This is best-effort, bukan jaminan menghindari full disk.
- `METADATA_TIMEOUT_SECONDS=45`.
- `DOWNLOAD_TIMEOUT_SECONDS=3600` (configurable; progress activity dapat memperpanjang ketika perlu).
- Satu job = satu item media. Jika URL menghasilkan banyak item/galleries/playlist, beri pesan jelas dan jangan silently mendownload banyak file.
- Jangan mengunduh otomatis pada langkah Analyze.

### 13.4 Kemampuan yang tidak dijanjikan

- Audio/video TikTok, Instagram, Facebook, X, atau YouTube dapat berhenti bekerja kapan saja karena perubahan platform.
- Rate limit, CAPTCHA, login, geo restriction, atau anti-bot mungkin terjadi. Tampilkan failure dengan jujur dan arahkan upgrade yt-dlp secara manual.
- MP4 atau resolusi tertentu mungkin tidak tersedia dan tidak boleh dipalsukan.

## 14. Keamanan dan Privasi

Meskipun aplikasi lokal, URL ekstraksi dan file output tetap memiliki attack surface.

**Wajib P0:**
1. Kedua service bind default ke `127.0.0.1`, bukan `0.0.0.0`.
2. Validasi HTTP Origin/Host pada endpoint mutasi; strict CORS dan penolakan host asing. CORS saja bukan proteksi CSRF/SSRF.
3. Domain allowlist media sosial, parser URL yang tegas; blok URL ke loopback, private ranges, link-local, metadata endpoint, dan nama host internal. Pertimbangkan redirect dan DNS resolution, jangan semata regex URL. yt-dlp melakukan fetch sendiri sehingga defense-in-depth diperlukan.
4. Tidak menyediakan endpoint fetch URL arbitrer, proxy thumbnail generik, atau input command line bebas.
5. Jalankan subprocess dengan `shell=False` dan argument array. Jangan memasukkan URL ke string perintah shell.
6. Simpan semua temporary/final files dalam controlled path; prevent `../` path traversal, symlink escapes, dan arbitrary delete.
7. Validasi UUID pada endpoint job; output hanya dari database job yang berhasil.
8. Sanitasi nama file, Content-Disposition, input URL, dan pesan error; jangan log session token/cookie/raw exception ke UI.
9. Tidak membaca browser cookies otomatis, tidak menyimpan cookie, tidak mengumpulkan data rahasia.
10. Tidak ada telemetry, remote analytics, update otomatis, atau upload konten ke layanan pihak ketiga.
11. Dependencies dipasang dari sumber terpercaya; `.env`, database, dan `storage/` masuk `.gitignore`.
12. Jangan gunakan `dangerouslySetInnerHTML` untuk judul atau deskripsi media yang berasal dari sumber luar.
13. Beri peringatan bahwa memakai downloader tetap tunduk pada hak cipta dan ketentuan platform, dan hanya untuk konten yang pengguna berhak akses/unduh.

**Catatan threat model:** aplikasi ini bukan sandbox untuk URL apa pun di internet. Area fetch lewat extractor pihak ketiga tidak dapat dijamin aman 100%. Scope MVP dibatasi ke domain media sosial yang diketahui dan mesin pengguna sendiri.

## 15. Kinerja dan Non-Fungsional

- Local-only, single user; tidak membutuhkan high availability.
- Frontend initial load responsif pada laptop low-to-mid hardware; hindari animasi berat.
- Metadata request tidak memblok request API lain.
- Download tidak memblok UI; beban concurrency dibatasi.
- History list pagination, maksimal 50 records per request default.
- Disiplin timeout untuk ekstraksi dan subprocess.
- UI menampilkan state gagal saat server offline, dilengkapi retry reconnect.
- Restart backend tidak menyebabkan file rusak dianggap completed.
- Tidak perlu GPU, Docker, Redis, PostgreSQL, dan server eksternal.
- Waktu dan tanggal disimpan sebagai UTC dan diformat menggunakan waktu lokal browser.
- Frontend menjaga aksesibilitas minimum keyboard dan teks kontras.

## 16. Pengujian dan QA

### 16.1 Unit tests backend

- Domain allowlist accepts valid official domains, rejects `youtube.com.evil.tld` and `127.0.0.1`, private IP, localhost, bad scheme, embedded credentials, excessive URL length.
- Metadata normalizer handles missing duration/thumbnail/formats, playlist/galleries, malformed extractor response.
- Filename sanitizer handles Windows-invalid symbols, Unicode, long titles, collision, traversal.
- State machine rejects invalid status transitions; retry creates new job.
- Error mapping maps expected extractor/system errors without leaking traceback.

### 16.2 Integration tests backend (mock subprocess/extractor)

- Analyze returns normalized metadata.
- Job queued -> downloading -> processing -> completed and file is downloadable.
- Active limit respected, 21st pending job rejects or responds appropriately if queue capped.
- Cancel queued, cancel running terminates fake process, cleans up staging.
- Failure, timeout, and restart recovery.
- File endpoint blocks non-completed, unknown job, path outside root, and missing file.
- Delete history with/without file; mutation origin and CORS checks.
- Health check degraded when dependency missing.

### 16.3 Frontend tests

- Input validation, loading, success preview, error states.
- Format choices only from real metadata.
- Active job polling, progress transitions, cancellation, and retry.
- History persists after refresh through backend.
- Screen widths 360, 768, 1280; keyboard interaction.
- Typecheck/lint/build.

### 16.4 Manual smoke test (network-dependent)

Use only a **public test URL supplied or authorized by the developer**:
1. Start backend and frontend.
2. Check health.
3. Analyze one public, accessible video.
4. Download in available MP4 resolution.
5. Validate file is playable, correct name/extension, recorded in history.
6. Extract MP3 and validate it has an audio stream.
7. Cancel a suitable longer download.
8. Restart servers; verify history and interrupted handling.

If network access is blocked in coding environment, do not pretend real-site smoke test passed: record it as `NOT RUN` and provide instructions for local manual test.

## 17. Implementasi Bertahap untuk Codex

### Fase 0 — Foundation

- Scaffold monorepo frontend/backend.
- `.env.example`, `.gitignore`, README, dev scripts, lint/typecheck/test commands.
- Health endpoint and dependency detection, SQLite migration.

**Exit:** dua server start lokal, health tampil di dashboard.

### Fase 1 — Metadata + UI

- URL policy, extract/normalize metadata, error contract.
- Dashboard URL form, preview card, quality/audio UI.
- Mocked tests for supported paths.

**Exit:** valid URL menampilkan informasi yang berasal dari backend (bukan dummy).

### Fase 2 — Download end-to-end

- Job persistence, bounded worker queue, subprocess yt-dlp, FFmpeg, progress polling.
- File stage/finalize/download endpoint; cancel/retry.
- Error handling, state machine tests.

**Exit:** unduhan nyata yang kompatibel tersimpan dan dapat dibuka.

### Fase 3 — History + polish

- History search/filter/delete, settings/health details, responsive UI, accessibility.
- Documentation Windows/WSL, quality gate lengkap.

**Exit:** kebutuhan MVP seluruhnya lulus.

**Catatan:** kerjakan fase berurutan dalam satu implementasi sampai MVP selesai. Jangan berhenti di scaffold/frontend statis jika implementasi backend masih bisa dikerjakan.

## 18. Definition of Done (DoD)

Project **tidak boleh diklaim selesai** sebelum semua hal berikut diperiksa:

- [ ] Frontend dan backend menjalankan aplikasi lokal tanpa cloud.
- [ ] Bind default kedua service pada `127.0.0.1`.
- [ ] Health check dependency dan status storage berfungsi.
- [ ] URL aman divalidasi dan URL tidak dikenal ditolak.
- [ ] Metadata diproses dari yt-dlp dan tampil di UI.
- [ ] Download MP4 yang tersedia berjalan end-to-end.
- [ ] Konversi/extraction MP3 yang tersedia berjalan dengan FFmpeg.
- [ ] File akhir tersedia secara lokal, file endpoint aman.
- [ ] Queue dan download progress ditampilkan.
- [ ] Cancel dan retry berfungsi.
- [ ] SQLite history bertahan setelah restart.
- [ ] History search/filter/delete berfungsi.
- [ ] Error mapping jelas dan sensitif data tidak bocor.
- [ ] Unit/integration test lulus (atau blocker dicatat jujur).
- [ ] Frontend lint, typecheck, build lulus (atau blocker dicatat jujur).
- [ ] `.env.example`, `.gitignore`, README, Windows/WSL setup lengkap.
- [ ] Minimal 1 real-site smoke test berhasil jika jaringan/test URL memungkinkan; jika tidak, jujur tandai belum diuji.
- [ ] Tidak ada API key atau rahasia dikomit.

## 19. Konfigurasi yang Diharapkan

Contoh `.env.example` (Codex boleh menambah vars yang benar-benar digunakan):

```dotenv
# backend
BACKEND_HOST=127.0.0.1
BACKEND_PORT=8000
FRONTEND_ORIGINS=http://127.0.0.1:3000,http://localhost:3000
DATABASE_PATH=./storage/app.db
DOWNLOAD_DIR=./storage/downloads
TEMP_DIR=./storage/tmp
MAX_ACTIVE_DOWNLOADS=2
MAX_QUEUE_LENGTH=20
MAX_FILE_SIZE_MB=2048
METADATA_TIMEOUT_SECONDS=45
DOWNLOAD_TIMEOUT_SECONDS=3600
FFMPEG_PATH=

# frontend: frontend/.env.local
NEXT_PUBLIC_API_URL=http://127.0.0.1:8000/api
```

**Catatan:** implementasi env loader harus konsisten dengan lokasi file. Gunakan absolute paths yang sudah di-resolve dari project root; jangan membuat storage di working directory tak terduga.

## 20. Cara Menjalankan yang Wajib Didokumentasikan

README yang dibuat Codex **wajib** menjelaskan setup Windows PowerShell dan WSL2/Linux secara terpisah, termasuk:

1. Instal Node.js dan pnpm; Python 3.11+; FFmpeg/ffprobe.
2. Instal yt-dlp dengan extras/dependency yang sesuai dan yt-dlp-ejs; pastikan JS runtime kompatibel dapat dipanggil.
3. Buat Python virtualenv di backend, install dependency.
4. `pnpm install` frontend.
5. Salin `.env.example` ke lokasi yang benar.
6. Jalankan `scripts/dev.ps1` untuk Windows atau `scripts/dev.sh` untuk Linux/WSL (script mengelola dua server, bind localhost, cek prerequisite jika masuk akal).
7. Akses `http://127.0.0.1:3000` dan verifikasi `http://127.0.0.1:8000/api/health`.
8. Atasi FFmpeg not found, JS runtime not found, metadata unavailable, 403, dan port conflict.
9. Keterangan bahwa storage Windows & WSL2 berbeda jika server dijalankan pada environment yang berbeda; berikan contoh mengganti DOWNLOAD_DIR.

Jangan menulis README berisi perintah yang tidak pernah diverifikasi terhadap file project aktual.

## 21. Risiko, Trade-off, dan Dependensi

| Risiko | Dampak | Mitigasi |
|---|---|---|
| Platform memperbarui anti-bot/extractor | Gagal analisis/download | Perbarui yt-dlp secara manual, error eksplisit, best-effort support |
| Tidak ada FFmpeg | Gagal merge/convert | Preflight + disable/penjelasan opsi |
| JS runtime/ejs tidak sesuai | Sebagian YouTube gagal | Health diagnostics + panduan konfigurasi resmi |
| Download besar memenuhi disk | Data hilang/OS melambat | Limit, disk checks, cancellation, clean staging |
| Backend mati di tengah pekerjaan | Partial file | Recovery `interrupted`, retry manual, staging isolated |
| Subprocess tersangkut | Job tertahan | Timeout & cancellation process group |
| Endpoint lokal diserang situs lain | Mutasi data/SSRF | localhost bind, Origin/Host checks, domain policy, path hardening |
| Metadata info tak lengkap | Salah label quality | Dynamic options, null handling, avoid guarantee |
| Lisensi dependency | Kendala distribusi | Cek lisensi dari repositori; jangan menyalin kode platform lain tanpa pemeriksaan |

## 22. Referensi Resmi

- yt-dlp README, CLI, embedding: https://github.com/yt-dlp/yt-dlp
- yt-dlp EJS and runtime: https://github.com/yt-dlp/yt-dlp/wiki/EJS
- yt-dlp installation: https://github.com/yt-dlp/yt-dlp/wiki/Installation
- Next.js App Router: https://nextjs.org/docs/app
- Next.js CLI hostname flag: https://nextjs.org/docs/app/api-reference/cli/next
- FastAPI docs: https://fastapi.tiangolo.com/
- FFmpeg docs: https://ffmpeg.org/documentation.html

---

## 23. Instruksi Eksekusi Akhir untuk Coding Agent

Implementasikan MediaVault berdasarkan PRD ini **sampai MVP berjalan**, bukan hanya mengulang spesifikasi. Prioritaskan keandalan download, security boundary lokal, test, dan setup daripada dekorasi UI. Buat kode produksi lokal yang modular. Setelah coding, jalankan semua test/build yang tersedia, laporkan hasil aktual, daftar gap yang tersisa, serta instruksi menjalankan proyek yang pasti sesuai implementasi. Jika platform sumber tidak dapat diakses saat pengujian, tulis keterbatasan tersebut secara eksplisit; jangan mengarang keberhasilan.
