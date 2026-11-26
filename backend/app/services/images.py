"""Isolated public photo & carousel extraction and Pillow validation/conversion.

Runs inside the engine subprocess after sys.addaudithook(outbound_policy) is installed.
Techniques adapted from open-source public extractors (cobalt & gallery-dl) without login/cookie bypass.
"""
import html
import io
import json
import math
import re
import zipfile
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, urlsplit
from urllib.request import ProxyHandler, Request, build_opener

from ..errors import AppError, extractor_error
from .extractor import clean_text, nonnegative
from .url_policy import image_url

MAX_IMAGES = 30
MAX_IMAGE_BYTES = 50 * 1024 * 1024
MAX_HTML_BYTES = 4 * 1024 * 1024
MAX_IMAGE_PIXELS = 40_000_000
IMAGE_CONTAINERS = ("jpg", "png", "webp")
GENERIC_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
)
SINGLE_POST_HINTS = (
    "/p/", "/reel/", "/tv/", "/pin/", "/status/", "/comments/", "/photo/", "/post/",
)


def looks_like_photo_url(url: str, platform: str) -> bool:
    parsed = urlsplit(url)
    host = (parsed.hostname or "").lower()
    if host == "i.redd.it":
        return True
    path = parsed.path.lower()
    if platform == "pinterest" and "/pin/" in path:
        return True
    if platform == "tiktok" and "/photo/" in path:
        return True
    return any(hint in path for hint in ("/p/", "/status/", "/comments/", "/post/"))


def _fetch_bytes(url: str, *, headers: dict[str, str] | None = None, max_bytes: int = MAX_HTML_BYTES,
                 timeout: int = 15, on_progress=None) -> tuple[bytes, str]:
    opener = build_opener(ProxyHandler({}))
    req_headers = {"User-Agent": GENERIC_UA, "Accept-Language": "en-US,en;q=0.9"}
    if headers:
        req_headers.update(headers)
    req = Request(url, headers=req_headers)
    try:
        with opener.open(req, timeout=timeout) as resp:
            content_type = (resp.headers.get("Content-Type") or "").partition(";")[0].strip().lower()
            total_hdr = resp.headers.get("Content-Length")
            total = int(total_hdr) if total_hdr and total_hdr.isdigit() else None
            if total is not None and total > max_bytes:
                raise AppError("FILE_TOO_LARGE")
            chunks = []
            received = 0
            while True:
                chunk = resp.read(65536)
                if not chunk:
                    break
                received += len(chunk)
                if received > max_bytes:
                    raise AppError("FILE_TOO_LARGE")
                chunks.append(chunk)
                if on_progress:
                    on_progress(received, total)
            return b"".join(chunks), content_type
    except AppError:
        raise
    except HTTPError as error:
        if error.code in (401, 403):
            raise AppError("LOGIN_REQUIRED") from None
        if error.code == 404:
            raise AppError("UNAVAILABLE_MEDIA") from None
        raise extractor_error(f"HTTP error {error.code}") from None
    except (URLError, OSError, TimeoutError):
        raise AppError("NETWORK_ERROR") from None


def _og_meta(text: str, prop: str) -> str | None:
    pattern = rf'<meta[^>]+(?:property|name)=["\']{re.escape(prop)}["\'][^>]+content=["\']([^"\']+)["\']'
    match = re.search(pattern, text, re.IGNORECASE)
    if not match:
        pattern = rf'<meta[^>]+content=["\']([^"\']+)["\'][^>]+(?:property|name)=["\']{re.escape(prop)}["\']'
        match = re.search(pattern, text, re.IGNORECASE)
    return clean_text(html.unescape(match.group(1))) if match else None


def _normalize_items(raw_items: list[dict]) -> list[dict]:
    seen = set()
    result = []
    for item in raw_items:
        safe = image_url(item.get("url"))
        if not safe or safe in seen:
            continue
        seen.add(safe)
        result.append({
            "index": len(result) + 1,
            "url": safe,
            "width": nonnegative(item.get("width")),
            "height": nonnegative(item.get("height")),
        })
        if len(result) >= MAX_IMAGES:
            break
    return result


def _extract_pinterest(url: str) -> dict | None:
    match = re.search(r"/pin/(?:[^/]*--)?(\d+)", urlsplit(url).path)
    if not match:
        return None
    pin_id = match.group(1)
    raw, _ = _fetch_bytes(f"https://www.pinterest.com/pin/{pin_id}/")
    text = raw.decode("utf-8", "replace")
    if re.search(r'"__typename"\s*:\s*"PinNotFound"', text):
        raise AppError("UNAVAILABLE_MEDIA")
    originals = re.findall(r'https://i\.pinimg\.com/originals/[A-Za-z0-9/_.-]+\.(?:jpg|jpeg|png|webp)', text)
    if not originals:
        originals = re.findall(r'https://i\.pinimg\.com/(?:736x|1200x)/[A-Za-z0-9/_.-]+\.(?:jpg|jpeg|png|webp)', text)
    items = _normalize_items([{"url": link} for link in originals[:1]])
    if not items:
        return None
    return {
        "title": _og_meta(text, "og:title") or f"Pinterest Pin {pin_id}",
        "uploader": None,
        "media_id": pin_id,
        "images": items,
    }


def _twitter_syndication_token(tweet_id: str) -> str:
    from yt_dlp.jsinterp import js_number_to_string

    translation = str.maketrans(dict.fromkeys("0."))
    return js_number_to_string((int(tweet_id) / 1e15) * math.pi, 36).translate(translation)


def _extract_twitter(url: str) -> dict | None:
    match = re.search(r"/status/(\d+)", urlsplit(url).path)
    if not match:
        return None
    tweet_id = match.group(1)
    token = _twitter_syndication_token(tweet_id)
    query = urlencode({"id": tweet_id, "token": token})
    raw, _ = _fetch_bytes(
        f"https://cdn.syndication.twimg.com/tweet-result?{query}",
        headers={"Accept": "application/json", "User-Agent": "Googlebot"},
    )
    try:
        data = json.loads(raw)
    except ValueError:
        return None
    if not isinstance(data, dict):
        return None
    candidates = []
    for media in data.get("mediaDetails") or []:
        if isinstance(media, dict) and media.get("type") == "photo" and media.get("media_url_https"):
            info = media.get("original_info") or {}
            media_url = media["media_url_https"]
            if "?" not in media_url:
                media_url = media_url + "?name=orig"
            candidates.append({"url": media_url, "width": info.get("width"), "height": info.get("height")})
    if not candidates:
        for photo in data.get("photos") or []:
            if isinstance(photo, dict) and photo.get("url"):
                candidates.append({"url": photo["url"], "width": photo.get("width"), "height": photo.get("height")})
    items = _normalize_items(candidates)
    if not items:
        return None
    user = data.get("user") or {}
    return {
        "title": clean_text(data.get("text")) or f"Post {tweet_id}",
        "uploader": clean_text(user.get("name") or user.get("screen_name")),
        "media_id": tweet_id,
        "images": items,
    }


def _extract_reddit(url: str) -> dict | None:
    parsed = urlsplit(url)
    host = (parsed.hostname or "").lower()
    if host == "i.redd.it":
        safe = image_url(url)
        if not safe:
            return None
        stem = Path(parsed.path).stem or "reddit"
        return {"title": f"Reddit {stem}", "uploader": None, "media_id": stem[:80],
                "images": _normalize_items([{"url": safe}])}
    match = re.search(r"/comments/([A-Za-z0-9]+)", parsed.path)
    if not match:
        return None
    post_id = match.group(1)
    raw, _ = _fetch_bytes(
        f"https://www.reddit.com/comments/{post_id}.json?raw_json=1",
        headers={"Accept": "application/json", "User-Agent": "MediaVault/0.1 (single-user local reader)"},
    )
    try:
        payload = json.loads(raw)
        post = payload[0]["data"]["children"][0]["data"]
    except (ValueError, KeyError, IndexError, TypeError):
        return None
    candidates = []
    if post.get("is_gallery") and isinstance(post.get("gallery_data"), dict):
        meta = post.get("media_metadata") or {}
        for entry in post["gallery_data"].get("items") or []:
            mid = entry.get("media_id")
            item = meta.get(mid) if isinstance(meta, dict) else None
            if isinstance(item, dict) and item.get("e") == "Image":
                src = item.get("s") or {}
                if src.get("u"):
                    candidates.append({"url": html.unescape(src["u"]), "width": src.get("x"), "height": src.get("y")})
    else:
        direct = post.get("url_overridden_by_dest") or post.get("url")
        if isinstance(direct, str) and image_url(direct):
            candidates.append({"url": direct})
    items = _normalize_items(candidates)
    if not items:
        return None
    return {
        "title": clean_text(post.get("title")) or f"Reddit {post_id}",
        "uploader": clean_text(post.get("author")),
        "media_id": post_id,
        "images": items,
    }


def _extract_tiktok(url: str) -> dict | None:
    match = re.search(r"/(?:photo|video)/(\d+)", urlsplit(url).path)
    if not match:
        return None
    post_id = match.group(1)
    raw, _ = _fetch_bytes(f"https://www.tiktok.com/@i/video/{post_id}")
    text = raw.decode("utf-8", "replace")
    marker = '<script id="__UNIVERSAL_DATA_FOR_REHYDRATION__" type="application/json">'
    if marker not in text:
        return None
    try:
        blob = text.split(marker, 1)[1].split("</script>", 1)[0]
        data = json.loads(blob)
        detail = data["__DEFAULT_SCOPE__"]["webapp.video-detail"]["itemInfo"]["itemStruct"]
    except (ValueError, KeyError, TypeError):
        return None
    image_post = detail.get("imagePost") or {}
    raw_images = image_post.get("images") or []
    candidates = []
    for entry in raw_images:
        if not isinstance(entry, dict):
            continue
        urls = (entry.get("imageURL") or {}).get("urlList") or []
        chosen = next((u for u in urls if isinstance(u, str) and (".jpeg" in u or ".jpg" in u) and image_url(u)), None)
        if not chosen:
            chosen = next((u for u in urls if isinstance(u, str) and image_url(u)), None)
        if chosen:
            candidates.append({"url": chosen, "width": entry.get("imageWidth"), "height": entry.get("imageHeight")})
    items = _normalize_items(candidates)
    if not items:
        return None
    author = detail.get("author") or {}
    return {
        "title": clean_text(detail.get("desc")) or clean_text(image_post.get("title")) or f"TikTok {post_id}",
        "uploader": clean_text(author.get("nickname") or author.get("uniqueId")),
        "media_id": post_id,
        "images": items,
    }


_IG_SJS_RE = re.compile(r'<script\b[^>]+\bdata-sjs>(\{.+?\})</script>')


def _instagram_best_image(item: dict) -> dict | None:
    if not isinstance(item, dict):
        return None
    if item.get("is_video") or item.get("video_versions") or item.get("media_type") == 2:
        return None
    cands = [
        c for c in ((item.get("image_versions2") or {}).get("candidates") or [])
        if isinstance(c, dict) and isinstance(c.get("url"), str)
    ]
    best = None
    if cands:
        sized = [c for c in cands if nonnegative(c.get("width")) and nonnegative(c.get("height"))]
        if sized:
            best = max(sized, key=lambda c: c["width"] * c["height"])
        else:
            best = cands[0]
    src = (best or {}).get("url") or item.get("display_uri") or item.get("display_url")
    if not src:
        return None
    dims = item.get("dimensions") or {}
    return {
        "url": src,
        "width": (best or {}).get("width") or item.get("original_width") or dims.get("width"),
        "height": (best or {}).get("height") or item.get("original_height") or dims.get("height"),
    }


def _parse_instagram_polaris(text: str, shortcode: str) -> dict | None:
    from yt_dlp.utils import traverse_obj

    matches = _IG_SJS_RE.findall(text)
    if not matches:
        return None
    media = traverse_obj(matches, (
        ..., {json.loads},
        "require", ..., ..., ..., "__bbox", "require",
        lambda _, v: isinstance(v, list) and len(v) > 0 and v[0] == "RelayPrefetchedStreamCache", ...,
        lambda _, v: isinstance(v, dict) and traverse_obj(v, ("__bbox", "result", "data", "xig_polaris_media")),
        "__bbox", "result", "data", "xig_polaris_media", {dict}, any,
    ))
    product = traverse_obj(media, ("if_not_gated_logged_out", {dict}))
    if not isinstance(product, dict):
        return None

    user = product.get("user") or {}
    username = clean_text(user.get("username"))
    uploader = clean_text(user.get("full_name") or username)
    caption = product.get("caption")
    caption_text = clean_text(caption.get("text")) if isinstance(caption, dict) else clean_text(caption)

    carousel = product.get("carousel_media") or []
    candidates = []
    if carousel:
        for slide in carousel:
            picked = _instagram_best_image(slide)
            if picked:
                candidates.append(picked)
    else:
        picked = _instagram_best_image(product)
        if picked:
            candidates.append(picked)

    items = _normalize_items(candidates)
    if not items:
        return None
    return {
        "title": caption_text or (f"Photo by {username}" if username else f"Instagram {shortcode}"),
        "uploader": uploader,
        "media_id": shortcode,
        "images": items,
    }


def _parse_instagram_embed(text: str, shortcode: str) -> dict | None:
    candidates = []
    title = None
    uploader = None

    init_match = re.search(r'"init",\[\],\[(.*?)\]\],', text)
    if init_match:
        try:
            embed_data = json.loads(init_match.group(1))
            ctx = json.loads(embed_data["contextJSON"]) if isinstance(embed_data.get("contextJSON"), str) else {}
            media = (ctx.get("gql_data") or {}).get("shortcode_media") or ctx.get("media") or {}
            if isinstance(media, dict):
                owner = media.get("owner") or {}
                uploader = clean_text(owner.get("username") or owner.get("full_name"))
                edges = ((media.get("edge_media_to_caption") or {}).get("edges")) or []
                if edges and isinstance(edges[0], dict):
                    title = clean_text((edges[0].get("node") or {}).get("text"))
                sidecar = ((media.get("edge_sidecar_to_children") or {}).get("edges")) or []
                if sidecar:
                    for edge in sidecar:
                        node = (edge or {}).get("node") or {}
                        if node.get("is_video"):
                            continue
                        resources = node.get("display_resources") or []
                        best = resources[-1] if resources and isinstance(resources[-1], dict) else {}
                        src = best.get("src") or node.get("display_url")
                        dims = node.get("dimensions") or {}
                        if src:
                            candidates.append({"url": src, "width": best.get("config_width") or dims.get("width"),
                                               "height": best.get("config_height") or dims.get("height")})
                elif not media.get("is_video"):
                    resources = media.get("display_resources") or []
                    best = resources[-1] if resources and isinstance(resources[-1], dict) else {}
                    src = best.get("src") or media.get("display_url")
                    dims = media.get("dimensions") or {}
                    if src:
                        candidates.append({"url": src, "width": best.get("config_width") or dims.get("width"),
                                           "height": best.get("config_height") or dims.get("height")})
        except (ValueError, KeyError, TypeError):
            pass

    if not candidates:
        for match_img in re.finditer(r'class="EmbeddedMediaImage"[^>]+src="([^"]+)"', text):
            candidates.append({"url": html.unescape(match_img.group(1))})

    items = _normalize_items(candidates)
    if not items:
        return None
    return {
        "title": title or (f"Photo by {uploader}" if uploader else f"Instagram {shortcode}"),
        "uploader": uploader,
        "media_id": shortcode,
        "images": items,
    }


def _extract_instagram(url: str) -> dict | None:
    match = re.search(r"/(?:p|reel|tv)/([A-Za-z0-9_-]+)", urlsplit(url).path)
    if not match:
        return None
    shortcode = match.group(1)
    nav_headers = {
        "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        "Sec-Fetch-Mode": "navigate",
    }
    try:
        raw, _ = _fetch_bytes(f"https://www.instagram.com/p/{shortcode}/", headers=nav_headers)
        text = raw.decode("utf-8", "replace")
        parsed = _parse_instagram_polaris(text, shortcode) or _parse_instagram_embed(text, shortcode)
        if parsed:
            return parsed
    except AppError:
        pass

    raw, _ = _fetch_bytes(
        f"https://www.instagram.com/p/{shortcode}/embed/captioned/",
        headers=nav_headers,
    )
    text = raw.decode("utf-8", "replace")
    return _parse_instagram_embed(text, shortcode)


def _gdl_meta_fields(meta: dict) -> tuple[str | None, str | None, str | None]:
    title = clean_text(meta.get("desc") or meta.get("title") or meta.get("description") or meta.get("content"))
    author_val = meta.get("author")
    user_val = meta.get("user")
    uploader = None
    if isinstance(author_val, dict):
        uploader = clean_text(author_val.get("nickname") or author_val.get("name") or author_val.get("uniqueId") or author_val.get("username"))
    elif isinstance(author_val, str):
        uploader = clean_text(author_val)
    if not uploader and isinstance(user_val, dict):
        uploader = clean_text(user_val.get("nickname") or user_val.get("name") or user_val.get("uniqueId") or user_val.get("username"))
    elif not uploader and isinstance(user_val, str):
        uploader = clean_text(user_val)
    if not uploader:
        uploader = clean_text(meta.get("username") or meta.get("uploader"))
    raw_id = meta.get("id") or meta.get("post_id") or meta.get("tweet_id")
    media_id = clean_text(str(raw_id), 80) if raw_id is not None else None
    return title, uploader, media_id


def _extract_gallery_dl(url: str) -> dict | None:
    path = urlsplit(url).path.lower()
    if not any(hint in path for hint in SINGLE_POST_HINTS):
        return None
    try:
        from gallery_dl import config as gdl_config, extractor as gdl_extractor
        from gallery_dl.extractor.message import Message
    except ImportError:
        return None
    gdl_config.clear()
    gdl_config.set(("extractor",), "proxy", None)
    gdl_config.set(("extractor",), "cookies", None)
    gdl_config.set(("extractor",), "timeout", 10.0)
    gdl_config.set(("extractor",), "retries", 1)
    extr = gdl_extractor.find(url)
    if extr is None:
        return None
    extr.initialize()
    candidates = []
    title = None
    uploader = None
    media_id = None
    for msg in extr:
        kind = msg[0]
        if kind == Message.Directory:
            meta = msg[1] if len(msg) > 1 and isinstance(msg[1], dict) else (msg[2] if len(msg) > 2 and isinstance(msg[2], dict) else None)
            if meta:
                t, u, m = _gdl_meta_fields(meta)
                title = title or t
                uploader = uploader or u
                media_id = media_id or m
        elif kind == Message.Url:
            target = msg[1]
            meta = msg[2] if len(msg) > 2 and isinstance(msg[2], dict) else {}
            t, u, m = _gdl_meta_fields(meta)
            title = title or t
            uploader = uploader or u
            media_id = media_id or m
            ext = str(meta.get("extension") or "").lower()
            if ext in {"mp4", "webm", "mov", "m4a", "mp3", "gifv"}:
                continue
            if image_url(target):
                candidates.append({"url": target, "width": meta.get("width"), "height": meta.get("height")})
                if len(candidates) >= MAX_IMAGES:
                    break
    items = _normalize_items(candidates)
    if not items:
        return None
    return {"title": title, "uploader": uploader, "media_id": media_id, "images": items}


def extract_images(url: str, platform: str) -> dict | None:
    handlers = {
        "pinterest": _extract_pinterest,
        "x": _extract_twitter,
        "reddit": _extract_reddit,
        "tiktok": _extract_tiktok,
        "instagram": _extract_instagram,
    }
    handler = handlers.get(platform)
    if handler:
        try:
            result = handler(url)
            if result and result.get("images"):
                return result
        except AppError:
            pass
        except Exception:
            pass
    try:
        return _extract_gallery_dl(url)
    except Exception:
        return None


def _encode_image(raw: bytes, container: str) -> bytes:
    from PIL import Image, ImageOps, UnidentifiedImageError

    Image.MAX_IMAGE_PIXELS = MAX_IMAGE_PIXELS
    try:
        with Image.open(io.BytesIO(raw)) as verify_img:
            verify_img.verify()
        with Image.open(io.BytesIO(raw)) as img:
            img = ImageOps.exif_transpose(img)
            out = io.BytesIO()
            if container == "jpg":
                if img.mode in ("RGBA", "LA", "PA") or (img.mode == "P" and "transparency" in img.info):
                    bg = Image.new("RGB", img.size, (255, 255, 255))
                    rgba = img.convert("RGBA")
                    bg.paste(rgba, mask=rgba.split()[-1])
                    rgb = bg
                else:
                    rgb = img.convert("RGB")
                rgb.save(out, format="JPEG", quality=92, optimize=True)
            elif container == "png":
                mode = "RGBA" if img.mode in ("RGBA", "LA", "PA", "P") else "RGB"
                img.convert(mode).save(out, format="PNG", optimize=True)
            elif container == "webp":
                mode = "RGBA" if img.mode in ("RGBA", "LA", "PA", "P") else "RGB"
                img.convert(mode).save(out, format="WEBP", quality=90)
            else:
                raise AppError("FORMAT_UNAVAILABLE")
            return out.getvalue()
    except (UnidentifiedImageError, Image.DecompressionBombError, OSError, ValueError):
        raise AppError("FORMAT_UNAVAILABLE") from None


def download_images(info: dict, stage: Path, request: dict, max_bytes: int, emit) -> None:
    items = info.get("images") or []
    if not items:
        raise AppError("FORMAT_UNAVAILABLE")
    container = request["container"]
    per_image_cap = min(max_bytes, MAX_IMAGE_BYTES)

    if container == "zip":
        if len(items) < 2:
            raise AppError("FORMAT_UNAVAILABLE")
        emit("selection", tracks=len(items))
        zip_path = stage / "media.zip"
        total_written = 0
        with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_STORED) as archive:
            for item in items:
                idx = item["index"]
                raw, _ = _fetch_bytes(
                    item["url"],
                    max_bytes=per_image_cap,
                    timeout=30,
                    on_progress=lambda dl, tot, i=idx: emit(
                        "progress", downloaded_bytes=dl, total_bytes=tot, speed=None, eta=None, track=f"img-{i}"
                    ),
                )
                encoded = _encode_image(raw, "jpg")
                total_written += len(encoded)
                if total_written > max_bytes:
                    raise AppError("FILE_TOO_LARGE")
                archive.writestr(f"photo_{idx:02d}.jpg", encoded)
        emit("processing")
        return

    index = request.get("image_index") or 1
    if not (1 <= index <= len(items)):
        raise AppError("FORMAT_UNAVAILABLE")
    chosen = items[index - 1]
    emit("selection", tracks=1)
    raw, _ = _fetch_bytes(
        chosen["url"],
        max_bytes=per_image_cap,
        timeout=30,
        on_progress=lambda dl, tot: emit(
            "progress", downloaded_bytes=dl, total_bytes=tot, speed=None, eta=None, track=f"img-{index}"
        ),
    )
    emit("processing")
    encoded = _encode_image(raw, container)
    if len(encoded) > max_bytes:
        raise AppError("FILE_TOO_LARGE")
    (stage / f"media.{container}").write_bytes(encoded)
