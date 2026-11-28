//! Normalisation of TikTok aweme payloads.
//!
//! Both the app API (snake_case) and the web rehydration blob (camelCase) are
//! parsed into the same shape, so the orchestration layer does not care which
//! one answered.

use serde_json::Value;

use crate::extractors::{normalize_items, DirectVideoVariant, ImageItem};
use crate::url_policy::{clean_text, validate_image_url, validate_video_cdn_url};

pub struct AwemeMedia {
    pub media_id: String,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub video_variants: Vec<DirectVideoVariant>,
    pub images: Vec<ImageItem>,
}

fn as_str_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn first_url_from_url_list(list: &Value) -> Option<String> {
    list.as_array()?
        .iter()
        .filter_map(Value::as_str)
        // A www.tiktok.com URL is the page itself, not a media stream.
        .filter(|u| !u.contains("://www.tiktok.com/"))
        .find_map(|u| validate_video_cdn_url(u, "tiktok"))
}

fn height_from_url_key(url_key: &str) -> Option<u32> {
    for part in url_key.split('_') {
        if let Some(digits) = part.strip_suffix('p') {
            if let Ok(value) = digits.parse::<u32>() {
                if (144..=4320).contains(&value) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn as_u32(value: &Value, keys: [&str; 2]) -> u32 {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_u64))
        .unwrap_or(0) as u32
}

fn dimension_of(addr: &Value) -> Option<u32> {
    let w = as_u32(addr, ["width", "Width"]);
    let h = as_u32(addr, ["height", "Height"]);
    if w == 0 && h == 0 {
        let key = addr
            .get("url_key")
            .or_else(|| addr.get("UrlKey"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return height_from_url_key(key);
    }
    if w > 0 && h > 0 {
        return Some(w.min(h));
    }
    Some(h)
}

fn push_variants(out: &mut Vec<DirectVideoVariant>, addr: &Value, bitrate: u64, fallback: u32) {
    // The app API uses url_list, the web payload uses UrlList.
    let list = addr.get("url_list").or_else(|| addr.get("UrlList"));
    let Some(url) = list.and_then(first_url_from_url_list) else {
        return;
    };
    if out.iter().any(|v| v.url == url) {
        return;
    }
    let height = dimension_of(addr)
        .or(if fallback > 0 { Some(fallback) } else { None })
        .unwrap_or(0);
    out.push(DirectVideoVariant {
        url,
        height,
        bitrate,
    });
}

fn first_url_at(node: &Value, pointers: &[&str]) -> Option<String> {
    for pointer in pointers {
        if let Some(url) = node
            .pointer(pointer)
            .and_then(Value::as_array)
            .and_then(|list| list.iter().filter_map(Value::as_str).next())
        {
            return Some(url.to_string());
        }
    }
    None
}

fn collect_images(
    raw: &mut Vec<(String, Option<u32>, Option<u32>)>,
    node: &Value,
    pointers: &[&str],
) {
    let width = node
        .get("image_width")
        .or_else(|| node.get("imageWidth"))
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .map(|v| v as u32);
    let height = node
        .get("image_height")
        .or_else(|| node.get("imageHeight"))
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .map(|v| v as u32);
    if let Some(url) = first_url_at(node, pointers) {
        raw.push((url, width, height));
    }
}

fn finish(
    media_id: String,
    title: Option<String>,
    uploader: Option<String>,
    duration_seconds: Option<f64>,
    thumbnail_url: Option<String>,
    mut video_variants: Vec<DirectVideoVariant>,
    images: Vec<ImageItem>,
) -> Option<AwemeMedia> {
    if video_variants.is_empty() && images.is_empty() {
        return None;
    }
    video_variants.sort_by(|a, b| b.height.cmp(&a.height).then_with(|| b.bitrate.cmp(&a.bitrate)));
    video_variants.truncate(24);
    let thumbnail_url = thumbnail_url.or_else(|| images.first().map(|i| i.url.clone()));
    Some(AwemeMedia {
        media_id,
        title,
        uploader,
        duration_seconds,
        thumbnail_url,
        video_variants,
        images,
    })
}

fn media_id_of(detail: &Value) -> Option<String> {
    if let Some(text) = detail.get("aweme_id").and_then(Value::as_str) {
        if !text.trim().is_empty() {
            return Some(text.trim().to_string());
        }
    }
    detail
        .get("aweme_id")
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
}

fn author_of(detail: &Value, nickname: &str, handle: &str) -> Option<String> {
    let raw = as_str_at(detail, nickname).or_else(|| as_str_at(detail, handle));
    clean_text(raw.as_deref(), 120)
}

/// Parse an aweme detail from the Android app API.
pub fn parse_app(detail: &Value) -> Option<AwemeMedia> {
    let media_id = media_id_of(detail)?;

    let mut video_variants = Vec::new();
    let mut fallback_height = 0;
    if let Some(video) = detail.get("video") {
        let w = video.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
        let h = video.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
        if w > 0 && h > 0 {
            fallback_height = w.min(h);
        }
        for pointer in [
            "/play_addr",
            "/download_addr",
            "/play_addr_h264",
            "/play_addr_bytevc1",
        ] {
            if let Some(addr) = video.pointer(pointer) {
                if addr.is_object() {
                    push_variants(&mut video_variants, addr, 0, fallback_height);
                }
            }
        }
        if let Some(bit_rates) = video.get("bit_rate").and_then(Value::as_array) {
            for entry in bit_rates {
                let bitrate = entry.get("bit_rate").and_then(Value::as_u64).unwrap_or(0);
                if let Some(addr) = entry.get("play_addr") {
                    push_variants(&mut video_variants, addr, bitrate, fallback_height);
                }
            }
        }
    }

    let mut raw_images = Vec::new();
    if let Some(items) = detail
        .pointer("/image_post_info/images")
        .and_then(Value::as_array)
    {
        for item in items {
            collect_images(
                &mut raw_images,
                item,
                &[
                    "/display_image/url_list",
                    "/owner_watermark_image/url_list",
                    "/thumbnail/url_list",
                ],
            );
        }
    }
    let images = normalize_items(raw_images);

    let thumbnail = [
        "/video/cover/url_list/0",
        "/video/origin_cover/url_list/0",
        "/video/dynamic_cover/url_list/0",
    ]
    .iter()
    .find_map(|pointer| as_str_at(detail, pointer))
    .and_then(|u| validate_image_url(Some(&u)));

    let duration = detail
        .pointer("/video/duration")
        .and_then(Value::as_u64)
        .map(|ms| ms as f64 / 1000.0)
        .filter(|d| *d > 0.0);

    finish(
        media_id,
        clean_text(as_str_at(detail, "/desc").as_deref(), 240),
        author_of(detail, "/author/nickname", "/author/unique_id"),
        duration,
        thumbnail,
        video_variants,
        images,
    )
}

/// Parse an itemStruct from the web rehydration payload.
pub fn parse_web(item: &Value) -> Option<AwemeMedia> {
    let media_id = item
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)?;

    let mut video_variants = Vec::new();
    let mut fallback_height = 0;
    if let Some(video) = item.get("video") {
        let w = video.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
        let h = video.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
        if w > 0 && h > 0 {
            fallback_height = w.min(h);
        }
        if let Some(bitrates) = video.get("bitrateInfo").and_then(Value::as_array) {
            for entry in bitrates {
                let bitrate = entry.get("Bitrate").and_then(Value::as_u64).unwrap_or(0);
                if let Some(addr) = entry.get("PlayAddr") {
                    push_variants(&mut video_variants, addr, bitrate, fallback_height);
                }
            }
        }
        for pointer in ["/playAddr", "/downloadAddr"] {
            if let Some(url) = as_str_at(video, pointer) {
                if url.contains("://www.tiktok.com/") {
                    continue;
                }
                if let Some(safe) = validate_video_cdn_url(&url, "tiktok") {
                    if !video_variants.iter().any(|v| v.url == safe) {
                        video_variants.push(DirectVideoVariant {
                            url: safe,
                            height: fallback_height,
                            bitrate: 0,
                        });
                    }
                }
            }
        }
    }

    let mut raw_images = Vec::new();
    if let Some(items) = item
        .pointer("/imagePost/images")
        .or_else(|| item.pointer("/imagePostInfo/images"))
        .and_then(Value::as_array)
    {
        for entry in items {
            collect_images(
                &mut raw_images,
                entry,
                &["/imageURL/urlList", "/displayImage/urlList"],
            );
        }
    }
    let images = normalize_items(raw_images);

    let thumbnail = ["/video/cover", "/video/originCover", "/video/dynamicCover"]
        .iter()
        .find_map(|pointer| as_str_at(item, pointer))
        .and_then(|u| validate_image_url(Some(&u)));

    let duration = item
        .pointer("/video/duration")
        .and_then(Value::as_f64)
        .filter(|d| *d > 0.0);

    finish(
        media_id,
        clean_text(as_str_at(item, "/desc").as_deref(), 240),
        author_of(item, "/author/nickname", "/author/uniqueId"),
        duration,
        thumbnail,
        video_variants,
        images,
    )
}
