import math
import unicodedata
from urllib.parse import urlsplit

from ..errors import AppError
from .url_policy import thumbnail

QUALITIES = ("1080", "720", "480", "360")
MEDIA_PROTOCOLS = {"https", "http", "m3u8_native", "m3u8", "http_dash_segments"}
LOCAL_FORMATS = "mov,matroska,webm,mpegts,aac,mp3,ogg,wav,flac,flv,mpeg,avi"
HLS_PROTOCOLS = {"m3u8", "m3u8_native"}


def clean_text(value, length=240):
    if not isinstance(value, str):
        return None
    result = "".join(char if not unicodedata.category(char).startswith("C") else " " for char in value)
    return " ".join(result.split())[:length] or None


def nonnegative(value):
    if type(value) is int and 0 <= value < 2**63:
        return value
    if type(value) is float and math.isfinite(value) and 0 <= value < 2**63:
        return value
    return None


def ensure_single(info):
    if (not isinstance(info, dict) or "entries" in info or info.get("_type") in {"playlist", "multi_video"}
            or info.get("is_live") or info.get("live_status") in {"is_live", "is_upcoming"}):
        raise AppError("UNAVAILABLE_MEDIA")
    if info.get("availability") in {"private", "premium_only", "subscriber_only", "needs_auth"}:
        raise AppError("LOGIN_REQUIRED")


def usable(fmt):
    try:
        url = urlsplit(fmt.get("url", ""))
        return (url.scheme in {"https", "http"} and bool(url.hostname) and url.username is None
                and url.port in (None, 80, 443) and fmt.get("protocol", url.scheme) in MEDIA_PROTOCOLS
                and not fmt.get("has_drm"))
    except (TypeError, ValueError, AttributeError):
        return False


def tracks(info):
    formats = info.get("formats")
    if not isinstance(formats, list):
        formats = [info]
    return [fmt for fmt in formats if isinstance(fmt, dict) and usable(fmt)]


def has_codec(fmt, kind):
    return fmt.get(kind) not in (None, "none", "")


def is_compatible_video(fmt):
    if fmt.get("ext") != "mp4":
        return False
    vcodec = fmt.get("vcodec")
    if vcodec in (None, ""):
        return True
    if vcodec == "none":
        return False
    return str(vcodec).startswith(("avc", "h264", "hev", "hvc", "av01", "vp09", "vp9"))


def is_compatible_audio(fmt):
    return fmt.get("ext") in {"mp4", "m4a"} and str(fmt.get("acodec", "")).startswith(("mp4a", "aac"))


def video_choices(formats, can_merge):
    """Input is yt-dlp's sorted format list, worst to best."""
    has_audio = any(fmt.get("acodec") != "none" for fmt in formats)
    audio = next((fmt for fmt in reversed(formats) if fmt.get("vcodec") == "none"
                  and is_compatible_audio(fmt)), None)
    for fmt in reversed(formats):
        if not is_compatible_video(fmt):
            continue
        if not can_merge and fmt.get("protocol") in HLS_PROTOCOLS:
            continue
        acodec = fmt.get("acodec")
        if is_compatible_audio(fmt) or acodec in (None, "") or (acodec == "none" and not has_audio):
            yield (fmt,)
        elif acodec == "none" and can_merge and audio:
            yield (fmt, audio)


def height(fmt):
    dimensions = [n for key in ("height", "width") if (n := nonnegative(fmt.get(key))) is not None and n > 0]
    return min(dimensions, default=0)


def select(info, request, can_merge):
    ensure_single(info)
    formats = tracks(info)
    if request["kind"] == "audio":
        audio = [fmt for fmt in formats if fmt.get("acodec") != "none"]
        selected = next((fmt for fmt in reversed(audio) if fmt.get("vcodec") == "none"), None)
        if selected is not None or audio:
            return (selected or audio[-1],)
    else:
        videos = list(video_choices(formats, can_merge))
        quality = request["quality"]
        if videos and quality == "best":
            return videos[0]
        if videos and quality != "best":
            target = int(quality)
            exact = next((pair for pair in videos if height(pair[0]) == target), None)
            if exact:
                return exact
            if request["allow_quality_fallback"]:
                lower = max((pair for pair in videos if 0 < height(pair[0]) < target),
                            key=lambda pair: height(pair[0]), default=None)
                if lower:
                    return lower
    raise AppError("FORMAT_UNAVAILABLE")


def size(formats):
    lengths = [nonnegative(fmt.get("filesize")) or nonnegative(fmt.get("filesize_approx")) for fmt in formats]
    return int(sum(lengths)) if lengths and all(lengths) and sum(lengths) < 2**63 else None


def normalize(info, url, platform, can_merge):
    ensure_single(info)
    formats = tracks(info)
    videos = list(video_choices(formats, can_merge))
    heights = {height(pair[0]) for pair in videos}
    audio = any(fmt.get("acodec") != "none" for fmt in formats)
    return dict(
        url=url, platform=platform, title=clean_text(info.get("title")),
        uploader=clean_text(info.get("uploader")) or clean_text(info.get("channel")),
        duration_seconds=nonnegative(info.get("duration")), thumbnail_url=thumbnail(info.get("thumbnail")),
        media_id=clean_text(info.get("id"), 80), media_type="video" if (videos or any(
            has_codec(fmt, "vcodec") for fmt in formats)) else ("audio" if audio else None),
        format_count=len(formats), available_qualities=(["best"] if videos else []) + [
            q for q in QUALITIES if int(q) in heights],
        has_audio=audio, supports_mp4=bool(videos), available_audio_formats=["mp3", "m4a"] if audio and can_merge else [],
        available_image_formats=[], images=[],
        estimated_size_bytes=size(videos[0]) if videos else None,
        warnings=[] if videos else ["Video MP4 yang kompatibel tidak tersedia."],
    )
