use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: &'static str,
    pub message: &'static str,
    pub retryable: bool,
}

impl AppError {
    pub fn new(code: &'static str) -> Self {
        let (message, retryable) = match code {
            "INVALID_URL" => ("Masukkan URL media HTTP atau HTTPS yang valid.", false),
            "UNSUPPORTED_PLATFORM" => ("Platform atau jenis tautan ini belum didukung.", false),
            "UNAVAILABLE_MEDIA" => ("Media tidak tersedia atau bukan satu konten publik.", false),
            "LOGIN_REQUIRED" => (
                "Sumber meminta login atau verifikasi akses (atau memblokir IP).",
                false,
            ),
            "GEO_RESTRICTED" => ("Media dibatasi untuk wilayah tertentu.", false),
            "FORMAT_UNAVAILABLE" => ("Format atau kualitas pilihan tidak tersedia dari sumber.", true),
            "EXTRACTOR_FAILED" => ("Media gagal diproses. Periksa tautan atau versi yt-dlp.", true),
            "NETWORK_ERROR" => ("Koneksi ke sumber gagal. Periksa jaringan dan coba lagi.", true),
            "TIMEOUT" => ("Waktu tunggu habis. Coba lagi.", true),
            "FFMPEG_MISSING" => ("FFmpeg dan ffprobe diperlukan untuk format ini.", true),
            "DEPENDENCY_MISSING" => ("yt-dlp belum ditemukan. Periksa tab Pengaturan.", true),
            "STORAGE_UNAVAILABLE" => ("Folder penyimpanan tidak dapat ditulis.", true),
            "FILE_TOO_LARGE" => ("Media melebihi batas ukuran file.", false),
            "JOB_NOT_FOUND" => ("Pekerjaan tidak ditemukan.", false),
            "FILE_NOT_FOUND" => ("File tidak ditemukan di disk.", false),
            "QUEUE_FULL" => ("Antrean penuh. Tunggu hingga pekerjaan lain selesai.", true),
            "JOB_STATE" => ("Tindakan ini tidak tersedia untuk status pekerjaan tersebut.", false),
            "INVALID_REQUEST" => ("Data permintaan tidak valid.", false),
            "FORBIDDEN" => ("Akses ditolak demi keamanan.", false),
            "NOT_FOUND" => ("Data tidak ditemukan.", false),
            "INSTALL_IN_PROGRESS" => ("Pemasangan komponen lain sedang berjalan. Tunggu hingga selesai.", false),
            "INSTALL_FAILED" => ("Gagal memasang komponen dari rilis resmi. Periksa koneksi internet dan coba lagi.", true),
            _ => ("Terjadi kesalahan internal.", true),
        };
        Self {
            code,
            message,
            retryable,
        }
    }

    pub fn payload(&self) -> ErrorPayload {
        ErrorPayload {
            code: self.code.to_string(),
            message: self.message.to_string(),
            retryable: self.retryable,
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for AppError {}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.payload().serialize(serializer)
    }
}

pub fn classify_extractor_error(msg: &str) -> AppError {
    let lower = msg.to_lowercase();
    let patterns: &[(&str, &[&str])] = &[
        (
            "LOGIN_REQUIRED",
            &[
                "sign in",
                "login required",
                "log in",
                "private video",
                "cookies",
                "not a bot",
            ],
        ),
        (
            "GEO_RESTRICTED",
            &["geo restricted", "geo-restricted", "your country", "your region"],
        ),
        (
            "FORMAT_UNAVAILABLE",
            &["requested format", "no video formats", "no suitable formats"],
        ),
        (
            "FFMPEG_MISSING",
            &["ffmpeg not found", "ffprobe not found", "ffmpeg is not installed"],
        ),
        (
            "UNAVAILABLE_MEDIA",
            &["unavailable", "removed", "unsupported url", "404", "drm"],
        ),
        ("TIMEOUT", &["timed out", "timeout"]),
        (
            "NETWORK_ERROR",
            &["connection", "dns", "http error", "certificate", "resolve host"],
        ),
    ];
    for (code, terms) in patterns {
        if terms.iter().any(|t| lower.contains(t)) {
            return AppError::new(code);
        }
    }
    AppError::new("EXTRACTOR_FAILED")
}
