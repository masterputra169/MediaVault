import errno

ERRORS = {
    "INVALID_URL": (422, "Masukkan URL media HTTP atau HTTPS yang valid.", False),
    "UNSUPPORTED_PLATFORM": (422, "Platform atau jenis tautan ini belum didukung.", False),
    "UNAVAILABLE_MEDIA": (422, "Media tidak tersedia atau bukan satu video publik.", False),
    "LOGIN_REQUIRED": (422, "Sumber meminta login atau verifikasi akses (atau memblokir IP server).", False),
    "GEO_RESTRICTED": (422, "Media dibatasi untuk wilayah tertentu.", False),
    "FORMAT_UNAVAILABLE": (422, "Format atau kualitas pilihan tidak tersedia dari sumber.", True),
    "EXTRACTOR_FAILED": (502, "Media gagal diproses. Periksa status sumber dan versi yt-dlp.", True),
    "NETWORK_ERROR": (502, "Koneksi ke sumber gagal. Periksa jaringan dan coba lagi.", True),
    "TIMEOUT": (504, "Waktu tunggu habis. Coba lagi.", True),
    "FFMPEG_MISSING": (503, "Pasang FFmpeg dan ffprobe atau atur FFMPEG_PATH.", True),
    "DEPENDENCY_MISSING": (503, "Pasang dependency backend sesuai petunjuk di Pengaturan.", True),
    "STORAGE_UNAVAILABLE": (503, "Folder penyimpanan tidak dapat ditulis.", True),
    "DISK_FULL": (507, "Ruang penyimpanan tidak mencukupi.", True),
    "FILE_TOO_LARGE": (413, "Media melebihi batas ukuran file.", False),
    "JOB_NOT_FOUND": (404, "Pekerjaan tidak ditemukan.", False),
    "FILE_NOT_FOUND": (404, "File tidak ditemukan.", False),
    "QUEUE_FULL": (429, "Antrean penuh. Tunggu hingga pekerjaan lain selesai.", True),
    "JOB_STATE": (409, "Tindakan ini tidak tersedia untuk status pekerjaan tersebut.", False),
    "INTERRUPTED": (409, "Pekerjaan terhenti saat server dimulai ulang.", True),
    "INVALID_REQUEST": (422, "Data permintaan tidak valid.", False),
    "UNAUTHORIZED": (401, "Silakan masuk terlebih dahulu.", False),
    "INVALID_CREDENTIALS": (401, "Nama pengguna atau kata sandi salah.", False),
    "RATE_LIMITED": (429, "Terlalu banyak percobaan gagal. Coba lagi nanti.", True),
    "FORBIDDEN": (403, "Permintaan harus berasal dari aplikasi lokal.", False),
    "NOT_FOUND": (404, "Endpoint tidak ditemukan.", False),
    "INTERNAL_ERROR": (500, "Terjadi kesalahan internal.", True),
}


class AppError(Exception):
    def __init__(self, code):
        self.code = code
        self.status, self.message, self.retryable = ERRORS[code]
        super().__init__(code)

    def payload(self):
        return dict(code=self.code, message=self.message, retryable=self.retryable, details=None)


def extractor_error(message):
    message = message.casefold()
    patterns = (
        ("LOGIN_REQUIRED", ("sign in", "login required", "log in", "private video", "cookies")),
        ("GEO_RESTRICTED", ("geo restricted", "geo-restricted", "your country", "your region")),
        ("FORMAT_UNAVAILABLE", ("requested format", "no video formats", "no suitable formats")),
        ("DISK_FULL", ("no space left", "disk full", "not enough space")),
        ("FFMPEG_MISSING", ("ffmpeg not found", "ffprobe not found", "ffmpeg is not installed")),
        ("UNAVAILABLE_MEDIA", ("unavailable", "removed", "unsupported url", "404", "drm")),
        ("TIMEOUT", ("timed out", "timeout")),
        ("NETWORK_ERROR", ("connection", "dns", "http error", "certificate", "resolve host")),
    )
    return AppError(next((code for code, terms in patterns if any(t in message for t in terms)),
                         "EXTRACTOR_FAILED"))


def storage_error(error):
    return AppError("DISK_FULL" if error.errno in {errno.ENOSPC, errno.EDQUOT}
                    else "STORAGE_UNAVAILABLE")
