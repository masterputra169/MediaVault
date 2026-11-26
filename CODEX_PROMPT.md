# Prompt untuk Codex

Baca seluruh `PRD.md` dan `AGENTS.md` di repository ini, lalu IMPLEMENTASIKAN proyek **MediaVault** sekarang sampai MVP benar-benar berfungsi end-to-end. Jangan hanya membuat rencana, wireframe, landing page, atau mock API.

Target: aplikasi downloader media sosial untuk **penggunaan pribadi** di `127.0.0.1`. Frontend Next.js App Router + TypeScript + Tailwind CSS, backend Python 3.11+ FastAPI, yt-dlp + FFmpeg (+ yt-dlp-ejs dan runtime JS yang sesuai), penyimpanan SQLite dan file lokal. Utamakan UI dark modern yang rapi tetapi pastikan backend dan download sungguhan bekerja.

Kerjakan berurutan:
1. Audit direktori kerja, baca requirements, scaffold project dengan dependencies kompatibel.
2. Setup backend config, keamanan URL/host/origin, health checks, SQLite migration.
3. Implementasi analyze media via yt-dlp, response metadata ter-normalisasi, error mapping.
4. Implementasi job queue persisten, subprocess downloader, FFmpeg merge/conversion, progress, cancel, retry, file output aman.
5. Implementasi frontend dashboard, preview, pemilihan output, active jobs, history, settings, responsive dark UI.
6. Buat script start Windows PowerShell dan WSL2/Linux, README lengkap, `.env.example`, `.gitignore`.
7. Tulis dan jalankan tes backend, frontend lint/typecheck/build. Lakukan smoke test nyata hanya jika akses jaringan dan URL uji publik yang diizinkan tersedia; jangan mengklaim berhasil bila belum diuji.
8. Bandingkan hasil dengan seluruh DoD di PRD, perbaiki kegagalan yang dapat diperbaiki.

Kerjakan langsung dan lanjutkan antar-fase tanpa meminta persetujuan setiap langkah. Jangan ekspose server ke internet. Jangan mengimplementasikan bypass DRM, private media, atau login bypass. Jika ada blocker teknis, tulis keterbatasannya secara jelas dan lanjutkan pekerjaan lain.

Pada akhir, berikan ringkasan file yang dibuat, semua perintah yang sudah dijalankan dan hasilnya, instruksi menjalankan aplikasi secara lokal, serta status DoD.
