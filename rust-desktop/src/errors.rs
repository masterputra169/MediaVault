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
                "Sumber meminta login, verifikasi akses, atau membatasi permintaan dari jaringan ini.",
                false,
            ),
            "GEO_RESTRICTED" => ("Media dibatasi untuk wilayah tertentu.", false),
            "FORMAT_UNAVAILABLE" => ("Format atau kualitas pilihan tidak tersedia dari sumber.", true),
            "EXTRACTOR_FAILED" => ("Gagal mengekstrak media dari sumber publik.", true),
            "MUX_FAILED" => ("Gagal menggabungkan aliran video & audio MP4.", true),
            "NETWORK_ERROR" => ("Koneksi ke sumber gagal. Periksa jaringan dan coba lagi.", true),
            "TIMEOUT" => ("Waktu tunggu habis. Coba lagi.", true),
            "STORAGE_UNAVAILABLE" => ("Folder penyimpanan tidak dapat ditulis.", true),
            "FILE_TOO_LARGE" => ("Media melebihi batas ukuran file (2 GB).", false),
            "JOB_NOT_FOUND" => ("Pekerjaan tidak ditemukan.", false),
            "FILE_NOT_FOUND" => ("File tidak ditemukan di disk.", false),
            "QUEUE_FULL" => ("Antrean penuh. Tunggu hingga pekerjaan lain selesai.", true),
            "JOB_STATE" => ("Tindakan ini tidak tersedia untuk status pekerjaan tersebut.", false),
            "INVALID_REQUEST" => ("Data permintaan tidak valid.", false),
            "FORBIDDEN" => ("Akses ditolak demi keamanan.", false),
            "NOT_FOUND" => ("Data tidak ditemukan.", false),
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
                "401",
                "403",
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
