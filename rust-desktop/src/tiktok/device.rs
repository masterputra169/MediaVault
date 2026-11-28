//! Android app identity and query construction, ported from yt-dlp.
//!
//! The public web front-end is protected by a WAF that escalates to a
//! JavaScript proof-of-work challenge. The Android app API is not, so it is
//! tried first: it only wants a plausible device identity together with the
//! app query parameters below.

use std::sync::atomic::{AtomicUsize, Ordering};

use once_cell::sync::Lazy;

use crate::tiktok::http::{random_hex, random_u8};

#[derive(Clone, Copy)]
pub struct AppInfo {
    pub app_name: &'static str,
    pub app_version: &'static str,
    pub manifest_app_version: &'static str,
    pub aid: &'static str,
}

/// A few real release trains; the extractor rotates through them when a
/// response is not valid JSON (the classic "app version retired" signal).
static APP_INFO_POOL: Lazy<Vec<AppInfo>> = Lazy::new(|| {
    vec![
        AppInfo {
            app_name: "musical_ly",
            app_version: "35.1.3",
            manifest_app_version: "2023501030",
            aid: "0",
        },
        AppInfo {
            app_name: "musical_ly",
            app_version: "34.9.5",
            manifest_app_version: "2023409050",
            aid: "0",
        },
        AppInfo {
            app_name: "trill",
            app_version: "35.1.3",
            manifest_app_version: "2023501030",
            aid: "1180",
        },
        AppInfo {
            app_name: "aweme",
            app_version: "35.1.3",
            manifest_app_version: "2023501030",
            aid: "1128",
        },
    ]
});

static CURSOR: AtomicUsize = AtomicUsize::new(0);

/// Rotate through the pool; never runs dry.
pub fn next_app_info() -> AppInfo {
    let index = CURSOR.fetch_add(1, Ordering::Relaxed);
    APP_INFO_POOL[index % APP_INFO_POOL.len()]
}

impl AppInfo {
    /// Cronet-flavoured app user agent, byte for byte what the app sends.
    pub fn user_agent(&self) -> String {
        let package = if self.app_name == "musical_ly" {
            format!("com.zhiliaoapp.musically/{}", self.manifest_app_version)
        } else {
            format!("com.ss.android.ugc.{}/{}", self.app_name, self.manifest_app_version)
        };
        format!(
            "{package} (Linux; U; Android 13; en_US; Pixel 7; Build/TD1A.220804.031; Cronet/58.0.2991.0)"
        )
    }

    fn version_code(&self) -> String {
        self.app_version
            .split('.')
            .map(|part| format!("{:02}", part.parse::<u32>().unwrap_or(0)))
            .collect()
    }
}

pub struct Device {
    pub device_id: String,
    pub openudid: String,
    pub cdid: String,
}

impl Device {
    pub fn fresh() -> Self {
        // Genuine install-space identifiers, in the range real devices use.
        let low = 7_250_000_000_000_000_000u64;
        let span = 732_509_989_999_999_4577u64 - low;
        let jitter = uuid::Uuid::new_v4().as_u128() as u64;
        Self {
            device_id: (low + jitter % span).to_string(),
            openudid: random_hex(16),
            cdid: uuid::Uuid::new_v4().to_string(),
        }
    }
}

fn quote_plus(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

impl Device {
    /// Full app query string; values are percent-encoded like urlencode().
    pub fn build_query(&self, app: &AppInfo) -> String {
        let now = chrono::Utc::now();
        let ts = now.timestamp();
        let last_install = ts - 86_400 - (random_u8() as i64 * 3600 % 1_036_800);
        let pairs: Vec<(&str, String)> = vec![
            ("device_platform", "android".into()),
            ("os", "android".into()),
            ("ssmix", "a".into()),
            ("_rticket", now.timestamp_millis().to_string()),
            ("cdid", self.cdid.clone()),
            ("channel", "googleplay".into()),
            ("aid", app.aid.into()),
            ("app_name", app.app_name.into()),
            ("version_code", app.version_code()),
            ("version_name", app.app_version.into()),
            ("manifest_version_code", app.manifest_app_version.into()),
            ("update_version_code", app.manifest_app_version.into()),
            ("ab_version", app.app_version.into()),
            ("resolution", "1080*2400".into()),
            ("dpi", "420".into()),
            ("device_type", "Pixel 7".into()),
            ("device_brand", "Google".into()),
            ("language", "en".into()),
            ("os_api", "29".into()),
            ("os_version", "13".into()),
            ("ac", "wifi".into()),
            ("is_pad", "0".into()),
            ("current_region", "US".into()),
            ("app_type", "normal".into()),
            ("sys_region", "US".into()),
            ("last_install_time", last_install.to_string()),
            ("timezone_name", "America/New_York".into()),
            ("residence", "US".into()),
            ("app_language", "en".into()),
            ("timezone_offset", "-14400".into()),
            ("host_abi", "armeabi-v7a".into()),
            ("locale", "en".into()),
            ("ac2", "wifi5g".into()),
            ("uoo", "1".into()),
            ("carrier_region", "US".into()),
            ("op_region", "US".into()),
            ("build_number", app.app_version.into()),
            ("region", "US".into()),
            ("ts", ts.to_string()),
            ("device_id", self.device_id.clone()),
            ("openudid", self.openudid.clone()),
        ];
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={}", quote_plus(v)))
            .collect::<Vec<_>>()
            .join("&")
    }
}
