//! Native TikTok extractor.
//!
//! The site is behind a WAF that fingerprints the TLS handshake and serves a
//! JavaScript proof-of-work challenge to anything that looks automated, so the
//! extractor mirrors the yt-dlp strategy in pure Rust:
//!
//! 1. signed app API request (Android device identity, app query parameters),
//! 2. browser-impersonating fetch of the public page,
//! 3. WAF challenge solve + retry when the page comes back as a challenge,
//! 4. the old rehydration-only path as a last resort.

pub mod challenge;
pub mod device;
pub mod http;
pub mod parse;

use once_cell::sync::Lazy;

use crate::errors::AppError;
use crate::extractors::{ImageItem, TikTokMediaResult};
use crate::tiktok::http::{CookieStore, Jar};

static DEVICE: Lazy<device::Device> = Lazy::new(device::Device::fresh);

/// Process-wide cookie jar: keeping ttwid/msToken warm across analyses makes
/// the WAF hand out far fewer challenges.
static JAR: Lazy<CookieStore> = Lazy::new(CookieStore::new);

/// Endpoint holding the full aweme record (formats, images, captions).
const DETAIL_ENDPOINT: &str = "multi/aweme/detail";

pub fn extract_ids(url: &str) -> Option<(String, Option<String>)> {
    let parsed = url::Url::parse(url).ok()?;
    let mut handle = None;
    let mut aweme_id = None;
    for segment in parsed.path_segments()? {
        if let Some(name) = segment.strip_prefix('@') {
            if !name.is_empty() {
                handle = Some(name.to_string());
            }
            continue;
        }
        if !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()) {
            aweme_id = Some(segment.to_string());
        }
    }
    aweme_id.map(|id| (id, handle))
}

fn canonical_page(aweme_id: &str, handle: Option<&str>) -> String {
    match handle {
        Some(name) => format!("https://www.tiktok.com/@{name}/video/{aweme_id}"),
        None => format!("https://www.tiktok.com/@_/video/{aweme_id}"),
    }
}

async fn fetch_app(aweme_id: &str, jar: &CookieStore) -> Result<parse::AwemeMedia, AppError> {
    let mut last_error = AppError::new("EXTRACTOR_FAILED");
    // Start from a random host so repeat runs spread across the pool.
    let host_offset = (http::random_u8() as usize) % http::API_HOSTS.len();

    for _ in 0..2 {
        let app = device::next_app_info();
        let device = &*DEVICE;
        jar.set(Jar::Api, "odin_tt", &http::random_hex(160));
        let query = device.build_query(&app);
        let body = format!("aweme_ids=%5B{aweme_id}%5D&request_source=0");

        for step in 0..http::API_HOSTS.len() {
            let host = http::API_HOSTS[(host_offset + step) % http::API_HOSTS.len()];
            let (status, text) = match http::post_app_api(
                host,
                DETAIL_ENDPOINT,
                &query,
                &body,
                &app.user_agent(),
                &[("X-Argus".into(), String::new())],
                jar,
            )
            .await
            {
                Ok(pair) => pair,
                Err(err) => {
                    last_error = err;
                    continue;
                }
            };

            if status == 429 {
                last_error = AppError::new("LOGIN_REQUIRED");
                continue;
            }
            if status >= 400 {
                last_error = crate::errors::classify_extractor_error(&format!("HTTP error {status}"));
                continue;
            }

            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                // Empty / non-JSON body means this app_info pair is retired.
                last_error = AppError::new("EXTRACTOR_FAILED");
                break;
            };

            if let Some(detail) = json
                .pointer("/aweme_details/0")
                .or_else(|| json.pointer("/aweme_detail"))
            {
                if let Some(media) = parse::parse_app(detail) {
                    return Ok(media);
                }
            }
            if let Some(status_code) = json.get("status_code").and_then(serde_json::Value::as_i64) {
                if status_code != 0 {
                    last_error = map_status_code(status_code);
                    continue;
                }
            }
            last_error = AppError::new("UNAVAILABLE_MEDIA");
        }
    }
    Err(last_error)
}

fn map_status_code(code: i64) -> AppError {
    match code {
        10216 | 10222 => AppError::new("LOGIN_REQUIRED"),
        10204 => AppError::new("GEO_RESTRICTED"),
        _ => AppError::new("UNAVAILABLE_MEDIA"),
    }
}

fn status_of(universal: &serde_json::Value) -> i64 {
    universal
        .pointer("/webapp.video-detail/statusCode")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

fn item_of(universal: &serde_json::Value) -> Option<&serde_json::Value> {
    universal
        .pointer("/webapp.video-detail/itemInfo/itemStruct")
        .or_else(|| universal.pointer("/webapp.reflow.video.detail/itemInfo/itemStruct"))
}

fn universal_data(html: &str) -> Option<serde_json::Value> {
    static RE: Lazy<regex::Regex> = Lazy::new(|| {
        regex::Regex::new(
            r#"(?s)<script[^>]+id=["']__UNIVERSAL_DATA_FOR_REHYDRATION__["'][^>]*>(\{.+?\})</script>"#,
        )
        .unwrap()
    });
    let captured = RE.captures(html)?;
    let parsed: serde_json::Value = serde_json::from_str(&captured[1]).ok()?;
    // Everything lives under __DEFAULT_SCOPE__; return just that object.
    parsed.pointer("/__DEFAULT_SCOPE__").cloned()
}

async fn fetch_web(
    page_url: &str,
    jar: &CookieStore,
) -> Result<parse::AwemeMedia, AppError> {
    let mut last_error = AppError::new("WAF_BLOCKED");
    for profile in [http::WebProfile::Safari, http::WebProfile::Chrome] {
        match fetch_web_with(profile, page_url, jar).await {
            Ok(media) => return Ok(media),
            Err(err) => {
                if matches!(err.code, "UNAVAILABLE_MEDIA" | "LOGIN_REQUIRED" | "GEO_RESTRICTED") {
                    return Err(err);
                }
                last_error = err;
            }
        }
    }
    Err(last_error)
}

async fn fetch_web_with(
    profile: http::WebProfile,
    page_url: &str,
    jar: &CookieStore,
) -> Result<parse::AwemeMedia, AppError> {
    let headers = http::blockbuster_headers();
    let mut page = http::get_web_page(profile, page_url, &headers, jar).await?;

    // Warm the cookie jar (ttwid, msToken) from the landing page first.
    if jar.value(Jar::Web, "ttwid").is_none() && page.status == 200 {
        if let Ok(landing) = http::get_web_page(profile, http::WEB_ORIGIN, &headers, jar).await {
            let _ = landing.final_url;
        }
    }

    let mut universal = universal_data(&page.body);
    eprintln!("[tt] profile={:?} status={} len={} challenge={}", profile, page.status, page.body.len(), challenge::looks_like_challenge(&page.body));
    if universal.is_none() && challenge::looks_like_challenge(&page.body) {
        let solved = challenge::solve(&page.body)?;
        eprintln!("[tt] solved cookie name={} len={}", solved.cookie_name, solved.cookie_value.len());
        jar.set(Jar::Web, &solved.cookie_name, &solved.cookie_value);
        for (name, value) in &solved.extra {
            jar.set(Jar::Web, name, value);
        }
        page = http::get_web_page(profile, page_url, &headers, jar).await?;
        eprintln!("[tt] retry status={} len={} hydration={} head={}", page.status, page.body.len(), universal_data(&page.body).is_some(), page.body.chars().take(120).collect::<String>());
        // The challenge cookie is single-use; drop it again.
        jar.clear(Jar::Web, &solved.cookie_name);
        for (name, _) in &solved.extra {
            jar.clear(Jar::Web, name);
        }
        universal = universal_data(&page.body);
    }

    let Some(universal) = universal else {
        return Err(if challenge::looks_like_challenge(&page.body) {
            AppError::new("WAF_BLOCKED")
        } else if page.body.contains("Please wait") || page.body.contains("SlardarWAF") {
            AppError::new("WAF_BLOCKED")
        } else {
            AppError::new("UNAVAILABLE_MEDIA")
        });
    };

    let code = status_of(&universal);
    if code != 0 {
        return Err(map_status_code(code));
    }
    let item = item_of(&universal).ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
    let media = parse::parse_web(item).ok_or_else(|| AppError::new("UNAVAILABLE_MEDIA"))?;
    if media.video_variants.is_empty()
        && item
            .get("isContentClassified")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    {
        return Err(AppError::new("LOGIN_REQUIRED"));
    }
    Ok(media)
}

fn merge(primary: Option<parse::AwemeMedia>, fallback: Option<parse::AwemeMedia>) -> Option<parse::AwemeMedia> {
    match (primary, fallback) {
        (Some(mut base), Some(other)) => {
            if base.thumbnail_url.is_none() {
                base.thumbnail_url = other.thumbnail_url;
            }
            if base.title.is_none() {
                base.title = other.title;
            }
            if base.uploader.is_none() {
                base.uploader = other.uploader;
            }
            if base.duration_seconds.is_none() {
                base.duration_seconds = other.duration_seconds;
            }
            for variant in other.video_variants {
                if !base.video_variants.iter().any(|v| v.url == variant.url) {
                    base.video_variants.push(variant);
                }
            }
            let mut images: Vec<ImageItem> = base.images.clone();
            for image in other.images {
                if !images.iter().any(|i| i.url == image.url) {
                    images.push(image);
                }
            }
            images.sort_by_key(|i| i.index);
            for (position, image) in images.iter_mut().enumerate() {
                image.index = position + 1;
            }
            base.images = images;
            base.video_variants.sort_by(|a, b| b.height.cmp(&a.height));
            Some(base)
        }
        (Some(base), None) => Some(base),
        (None, Some(fallback)) => Some(fallback),
        (None, None) => None,
    }
}

/// Resolve a TikTok video or photo post into downloadable media.
pub async fn fetch(url: &str) -> Result<TikTokMediaResult, AppError> {
    let (aweme_id, handle) = extract_ids(url).ok_or_else(|| AppError::new("INVALID_URL"))?;
    let jar = &*JAR;

    // The page is the primary source: it carries the full format ladder for
    // videos and the original images for slideshows.
    let page_url = canonical_page(&aweme_id, handle.as_deref());
    let web_result = fetch_web(&page_url, jar).await;
    if let Ok(media) = web_result.as_ref() {
        if !media.video_variants.is_empty() || !media.images.is_empty() {
            return Ok(into_result(clone_media(media), &jar));
        }
    }

    // The app API needs no WAF clearance, so it is the fallback when the web
    // front-end is unreachable, challenged or geo-blocked.
    let app_result = fetch_app(&aweme_id, jar).await;
    let merged = merge(
        app_result.as_ref().ok().map(clone_media),
        web_result.as_ref().ok().map(clone_media),
    );
    let Some(media) = merged else {
        return match (app_result.err(), web_result.err()) {
            (Some(app_err), Some(web_err)) => {
                // A blockage is the most actionable error to surface.
                if matches!(app_err.code, "UNAVAILABLE_MEDIA" | "EXTRACTOR_FAILED") {
                    Err(web_err)
                } else {
                    Err(app_err)
                }
            }
            (Some(err), None) | (None, Some(err)) => Err(err),
            (None, None) => Err(AppError::new("EXTRACTOR_FAILED")),
        };
    };
    Ok(into_result(media, jar))
}

fn into_result(media: parse::AwemeMedia, jar: &CookieStore) -> TikTokMediaResult {
    TikTokMediaResult {
        media_id: media.media_id,
        title: media.title,
        uploader: media.uploader,
        duration_seconds: media.duration_seconds,
        thumbnail_url: media.thumbnail_url,
        video_variants: media.video_variants,
        images: media.images,
        // CDN hotlinks accept either jar; the web one is the useful one.
        cookies: jar.header(Jar::Web).or_else(|| jar.header(Jar::Api)),
    }
}

fn clone_media(m: &parse::AwemeMedia) -> parse::AwemeMedia {
    parse::AwemeMedia {
        media_id: m.media_id.clone(),
        title: m.title.clone(),
        uploader: m.uploader.clone(),
        duration_seconds: m.duration_seconds,
        thumbnail_url: m.thumbnail_url.clone(),
        video_variants: m.video_variants.clone(),
        images: m.images.clone(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiktok::challenge;
    use base64::Engine as _;
    use sha2::Digest as _;

    const CHALLENGE_PAGE: &str = r#"<html><body>Please wait...
        <p id="wci" class="_wafchallengeid"></p>
        <p id="cs" class="PAYLOAD"></p>
        <p id="rci" class="waforiginalreid"></p>
        <p id="rs" class=""></p></body></html>"#;

    fn challenge_payload(answer: u32) -> (String, Vec<u8>) {
        let base = b"0123456789abcdef0123456789abcdef".to_vec();
        let mut hasher = sha2::Sha256::new();
        hasher.update(&base);
        hasher.update(answer.to_string().as_bytes());
        let expected = hasher.finalize().to_vec();
        let json = serde_json::json!({
            "v": {
                "a": base64::engine::general_purpose::STANDARD.encode(&base),
                "b": 1_795_800_526,
                "c": base64::engine::general_purpose::STANDARD.encode(&expected),
            },
            "s": "signature",
        });
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.to_string());
        (encoded, expected)
    }

    fn page_with(payload: &str) -> String {
        CHALLENGE_PAGE.replace("PAYLOAD", payload)
    }

    #[test]
    fn detects_and_solves_challenge() {
        let (payload, _) = challenge_payload(4_242);
        let page = page_with(&payload);
        assert!(challenge::looks_like_challenge(&page));

        let solved = challenge::solve(&page).expect("challenge solves");
        assert_eq!(solved.cookie_name, "_wafchallengeid");

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&solved.cookie_value)
            .expect("cookie is base64 json");
        let value: serde_json::Value = serde_json::from_slice(&decoded).expect("json");
        let answer = value
            .get("d")
            .and_then(|v| v.as_str())
            .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
            .expect("answer nonce");
        assert_eq!(String::from_utf8_lossy(&answer), "4242");
        // Order-dependent fields survive the round trip.
        assert!(value.pointer("/v/a").is_some());
        // Empty rci value is skipped like yt-dlp does.
        assert!(solved.extra.is_empty());
    }

    #[test]
    fn plain_pages_are_not_challenges() {
        assert!(!challenge::looks_like_challenge("<html><body>hi</body></html>"));
    }

    #[test]
    fn extracts_ids_from_every_url_shape() {
        assert_eq!(
            extract_ids("https://www.tiktok.com/@tatemcrae/video/7107337212743830830?lang=en")
                .unwrap(),
            (
                "7107337212743830830".into(),
                Some("tatemcrae".into())
            )
        );
        assert_eq!(
            extract_ids("https://www.tiktok.com/@user/photo/7107337212743830830").unwrap(),
            ("7107337212743830830".into(), Some("user".into()))
        );
        assert_eq!(
            extract_ids("https://www.tiktokv.com/share/video/7668090902816017671/").unwrap(),
            ("7668090902816017671".into(), None)
        );
        assert!(extract_ids("https://www.tiktok.com/@user").is_none());
    }

    #[test]
    fn parses_app_payload_into_formats_and_photos() {
        let payload = serde_json::json!({
            "aweme_id": "7107337212743830830",
            "desc": "hello world",
            "author": { "nickname": "Tate", "unique_id": "tatemcrae" },
            "video": {
                "width": 720, "height": 1280, "duration": 15000,
                "cover": { "url_list": ["https://p16-sign.tiktokcdn.com/cover.jpeg"] },
                "play_addr": {
                    "url_list": ["https://v16-webapp-prime.tiktok.com/video/play.mp4"],
                    "url_key": "v1_abc_720p_4500"
                },
                "bit_rate": [{
                    "bit_rate": 900_000,
                    "play_addr": {
                        "url_list": ["https://v19-webapp.tiktokcdn.com/hd.mp4"],
                        "width": 1080, "height": 1920
                    }
                }]
            },
            "image_post_info": { "images": [{
                "image_width": 1080, "image_height": 1440,
                "display_image": { "url_list": ["https://p19-sign.tiktokcdn.com/a.jpg"] }
            }] }
        });
        let media = parse::parse_app(&payload).expect("parsed");
        assert_eq!(media.media_id, "7107337212743830830");
        assert_eq!(media.images.len(), 1);
        assert_eq!(media.video_variants.len(), 2);
        // Highest resolution first.
        assert_eq!(media.video_variants[0].height, 1080);
        assert_eq!(media.uploader.as_deref(), Some("Tate"));
        assert_eq!(media.duration_seconds, Some(15.0));
        assert_eq!(
            media.thumbnail_url.as_deref(),
            Some("https://p16-sign.tiktokcdn.com/cover.jpeg")
        );
    }

    #[test]
    fn parses_web_payload_and_drops_page_urls() {
        let payload = serde_json::json!({
            "id": "7107337212743830830",
            "desc": "web",
            "author": { "nickname": "Web" },
            "video": {
                "width": 720, "height": 1280, "duration": 9,
                "playAddr": "https://www.tiktok.com/@x/video/7107337212743830830",
                "bitrateInfo": [{
                    "Bitrate": 1000,
                    "PlayAddr": {
                        "Width": 720, "Height": 1280,
                        "UrlList": ["https://v16-webapp.tiktokcdn-us.com/play.mp4"]
                    }
                }]
            },
            "imagePost": { "images": [{
                "imageWidth": 800, "imageHeight": 1200,
                "imageURL": { "urlList": ["https://p19-common-sign.tiktokcdn.com/b.jpeg"] }
            }] }
        });
        let media = parse::parse_web(&payload).expect("parsed");
        assert_eq!(media.video_variants.len(), 1);
        assert!(media.video_variants[0].url.ends_with("/play.mp4"));
        assert_eq!(media.images.len(), 1);
        assert_eq!(media.images[0].width, Some(800));
    }

    #[test]
    fn parses_bytevc2_and_h265_variants() {
        let payload = serde_json::json!({
            "aweme_id": "1",
            "video": {
                "play_addr": { "url_list": ["https://v16-webapp.tiktokcdn.com/a.mp4"] },
                "play_addr_h264": { "url_list": ["https://v16-webapp.tiktokcdn.com/b.mp4"], "height": 720, "width": 406 }
            }
        });
        let media = parse::parse_app(&payload).expect("parsed");
        assert_eq!(media.video_variants.len(), 2);
        assert_eq!(media.video_variants[0].height, 406);
    }

    #[test]
    fn rejects_hostile_or_empty_media_urls() {
        let payload = serde_json::json!({
            "aweme_id": "1",
            "video": { "play_addr": { "url_list": [
                "https://evil.example.com/a.mp4",
                "http://v16-webapp.tiktokcdn.com/insecure.mp4"
            ] } }
        });
        assert!(parse::parse_app(&payload).is_none());
    }

    #[test]
    fn blockbuster_headers_stay_in_range() {
        for _ in 0..40 {
            let headers = http::blockbuster_headers();
            assert!((2..=8).contains(&headers.len()));
            for (name, value) in headers {
                assert!(name.len() >= 8 && name.len() <= 24);
                assert!(value.len() >= 16 && value.len() <= 32);
                assert!(name.chars().all(|c| "bcdfghjklmnpqrstvwxz".contains(c)));
                assert!(!name.chars().any(|c| c.is_uppercase()));
            }
        }
    }

    #[test]
    fn device_query_carries_required_app_parameters() {
        let device = device::Device::fresh();
        let app = device::AppInfo {
            app_name: "musical_ly",
            app_version: "35.1.3",
            manifest_app_version: "2023501030",
            aid: "0",
        };
        let query = device.build_query(&app);
        for needed in [
            "device_platform=android",
            "app_name=musical_ly",
            "version_code=350103",
            "manifest_version_code=2023501030",
            "device_type=Pixel+7",
            "ts=",
            "_rticket=",
            "device_id=",
            "openudid=",
        ] {
            assert!(query.contains(needed), "missing {needed} in {query}");
        }
        assert!(app.user_agent().starts_with("com.zhiliaoapp.musically/2023501030"));
    }

    #[test]
    fn cookie_store_scopes_jars_and_handles_expiry() {
        let jar = http::CookieStore::new();
        jar.set(http::Jar::Web, "a", "1");
        jar.absorb(
            http::Jar::Web,
            &[(
                "set-cookie".into(),
                "b=2; Path=/; Max-Age=0".into(),
            )],
        );
        assert_eq!(jar.header(http::Jar::Web).as_deref(), Some("a=1"));
        assert!(jar.header(http::Jar::Api).is_none());
        jar.set(http::Jar::Api, "sid_tt", "xyz");
        assert_eq!(jar.value(http::Jar::Api, "sid_tt").as_deref(), Some("xyz"));
    }

    // ---- live smoke test: cargo test tiktok -- --ignored --nocapture ----

    fn live_url() -> String {
        std::env::var("TT_LIVE_URL")
            .unwrap_or_else(|_| "https://www.tiktok.com/@hankgreen1/video/7047596209028074758".into())
    }

    #[tokio::test]
    #[ignore = "hits the live TikTok site"]
    async fn live_extracts_public_video() {
        let url = live_url();
        let media = fetch(&url).await.expect("live extraction succeeds");
        assert_eq!(extract_ids(&url).unwrap().0, media.media_id);
        assert!(!media.video_variants.is_empty(), "no video formats returned");
        assert!(media
            .video_variants
            .iter()
            .all(|v| v.url.starts_with("https://")));
        assert!(media.thumbnail_url.is_some());
        println!(
            "variants={} best={} title={:?} uploader={:?} cookies={}",
            media.video_variants.len(),
            media.video_variants[0].url,
            media.title,
            media.uploader,
            media.cookies.is_some()
        );
    }

    #[tokio::test]
    #[ignore = "hits the live TikTok site; set TT_LIVE_PHOTO to a slideshow URL"]
    async fn live_extracts_public_photo_post() {
        let Ok(url) = std::env::var("TT_LIVE_PHOTO") else {
            println!("TT_LIVE_PHOTO not set, skipping");
            return;
        };
        let media = fetch(&url).await.expect("live photo extraction succeeds");
        assert_eq!(extract_ids(&url).unwrap().0, media.media_id);
        assert!(!media.images.is_empty(), "no slideshow images returned");
        assert!(media.images.iter().all(|i| i.url.starts_with("https://")));
        assert!(media.images.iter().all(|i| i.width.is_some() || i.height.is_some()));
        println!(
            "images={} first={} title={:?}",
            media.images.len(),
            media.images[0].url,
            media.title
        );
    }

    #[tokio::test]
    #[ignore = "hits the live TikTok site"]
    async fn live_waf_challenge_is_solved_by_impersonation() {
        // The plain generic client is challenged; the impersonating one is not.
        let jar = http::CookieStore::new();
        let page = http::get_web_page(
            http::WebProfile::Safari,
            "https://www.tiktok.com/@hankgreen1/video/7047596209028074758",
            &http::blockbuster_headers(),
            &jar,
        )
        .await
        .expect("page fetch");
        assert_eq!(page.status, 200);
        assert!(
            page.body.contains("__UNIVERSAL_DATA_FOR_REHYDRATION__")
                || challenge::looks_like_challenge(&page.body),
            "expected hydration payload or a solvable challenge"
        );
        println!("page_len={} ttwid={:?}", page.body.len(), jar.value(http::Jar::Web, "ttwid").is_some());
    }
}
