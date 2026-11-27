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
            "INVALID_URL" => ("Enter a valid HTTP or HTTPS media URL.", false),
            "UNSUPPORTED_PLATFORM" => ("This platform or link type is not supported yet.", false),
            "UNAVAILABLE_MEDIA" => ("Media is unavailable or not a public single post.", false),
            "LOGIN_REQUIRED" => (
                "The source requires sign-in, access verification, or is rate-limiting this network.",
                false,
            ),
            "WAF_BLOCKED" => (
                "This site is showing an anti-bot verification challenge (WAF) on this network. MediaVault does not bypass anti-bot gates.",
                false,
            ),
            "GEO_RESTRICTED" => ("This media is restricted in your region.", false),
            "FORMAT_UNAVAILABLE" => ("The selected format or quality is unavailable from the source.", true),
            "EXTRACTOR_FAILED" => ("Failed to extract media from the public source.", true),
            "MUX_FAILED" => ("Failed to mux MP4 video and audio streams.", true),
            "NETWORK_ERROR" => ("Connection to the source failed. Check your network and try again.", true),
            "TIMEOUT" => ("Request timed out. Please try again.", true),
            "STORAGE_UNAVAILABLE" => ("The destination folder is not writable.", true),
            "FILE_TOO_LARGE" => ("Media exceeds the 2 GB file size limit.", false),
            "JOB_NOT_FOUND" => ("Download entry was not found.", false),
            "FILE_NOT_FOUND" => ("File was not found on disk.", false),
            "QUEUE_FULL" => ("Download queue is full. Wait for active downloads to finish.", true),
            "JOB_STATE" => ("This action is not available for the current download status.", false),
            "INVALID_REQUEST" => ("Invalid download request parameters.", false),
            "FORBIDDEN" => ("Action blocked for security reasons.", false),
            "NOT_FOUND" => ("Requested item was not found.", false),
            _ => ("An internal error occurred.", true),
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
