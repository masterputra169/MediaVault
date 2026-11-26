import io
import re
import secrets
import socket
import threading
import time
from collections import OrderedDict
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from ..errors import AppError
from .url_policy import image_url, is_public, validate

PREVIEW_TOKEN_RE = re.compile(r"^[A-Za-z0-9_-]{16,64}$")
PREVIEW_TTL_SECONDS = 600
PREVIEW_MAX_TOKENS = 256
PREVIEW_MAX_CACHED_BLOBS = 64
PREVIEW_MAX_FETCH_BYTES = 15 * 1024 * 1024
PREVIEW_MAX_PIXELS = 40_000_000
PREVIEW_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
)


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        return None


def _encode_preview_jpeg(raw: bytes) -> bytes:
    from PIL import Image, ImageOps, UnidentifiedImageError

    Image.MAX_IMAGE_PIXELS = PREVIEW_MAX_PIXELS
    try:
        with Image.open(io.BytesIO(raw)) as check_img:
            check_img.verify()
        with Image.open(io.BytesIO(raw)) as img:
            img = ImageOps.exif_transpose(img)
            img.thumbnail((720, 720))
            if img.mode in ("RGBA", "LA", "PA") or (img.mode == "P" and "transparency" in img.info):
                bg = Image.new("RGB", img.size, (15, 22, 40))
                rgba = img.convert("RGBA")
                bg.paste(rgba, mask=rgba.split()[-1])
                rgb = bg
            else:
                rgb = img.convert("RGB")
            out = io.BytesIO()
            rgb.save(out, format="JPEG", quality=82, optimize=True)
            return out.getvalue()
    except (UnidentifiedImageError, Image.DecompressionBombError, OSError, ValueError):
        raise AppError("NOT_FOUND") from None


def _fetch_public_image_bytes(target_url: str) -> bytes:
    safe = image_url(target_url)
    if not safe:
        raise AppError("NOT_FOUND")
    parsed = urlsplit(safe)
    host = parsed.hostname
    if not host:
        raise AppError("NOT_FOUND")
    try:
        infos = socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)
    except OSError:
        raise AppError("NETWORK_ERROR") from None
    addresses = [info[4][0] for info in infos if info[4]]
    if not addresses or not all(is_public(addr) for addr in addresses):
        raise AppError("FORBIDDEN")
    opener = build_opener(ProxyHandler({}), _NoRedirect())
    req = Request(safe, headers={"User-Agent": PREVIEW_UA, "Accept": "image/*,*/*;q=0.8"})
    try:
        with opener.open(req, timeout=12) as resp:
            total_hdr = resp.headers.get("Content-Length")
            if total_hdr and total_hdr.isdigit() and int(total_hdr) > PREVIEW_MAX_FETCH_BYTES:
                raise AppError("FILE_TOO_LARGE")
            chunks = []
            received = 0
            while True:
                chunk = resp.read(65536)
                if not chunk:
                    break
                received += len(chunk)
                if received > PREVIEW_MAX_FETCH_BYTES:
                    raise AppError("FILE_TOO_LARGE")
                chunks.append(chunk)
            return b"".join(chunks)
    except AppError:
        raise
    except (HTTPError, URLError, OSError, TimeoutError):
        raise AppError("NOT_FOUND") from None


class MediaService:
    def __init__(self, engine):
        self.engine = engine
        self.slots = threading.BoundedSemaphore(2)
        self.lock = threading.Lock()
        self.cache = OrderedDict()
        self.previews: OrderedDict[str, tuple[float, str]] = OrderedDict()
        self.preview_tokens_by_url: dict[str, str] = {}
        self.preview_bytes: OrderedDict[str, tuple[float, bytes]] = OrderedDict()

    def register_preview(self, raw_url: str | None) -> str | None:
        safe = image_url(raw_url)
        if not safe:
            return None
        now = time.monotonic()
        with self.lock:
            existing = self.preview_tokens_by_url.get(safe)
            if existing:
                entry = self.previews.get(existing)
                if entry and entry[0] > now:
                    self.previews.move_to_end(existing)
                    return f"/api/media/preview/{existing}"
                self.previews.pop(existing, None)
                self.preview_tokens_by_url.pop(safe, None)
            token = secrets.token_urlsafe(18)
            self.previews[token] = (now + PREVIEW_TTL_SECONDS, safe)
            self.preview_tokens_by_url[safe] = token
            while len(self.previews) > PREVIEW_MAX_TOKENS:
                old_token, (_, old_url) = self.previews.popitem(last=False)
                if self.preview_tokens_by_url.get(old_url) == old_token:
                    self.preview_tokens_by_url.pop(old_url, None)
                self.preview_bytes.pop(old_token, None)
            return f"/api/media/preview/{token}"

    def annotate_metadata(self, metadata: dict) -> dict:
        result = dict(metadata)
        result["preview_url"] = self.register_preview(metadata.get("thumbnail_url"))
        raw_images = metadata.get("images")
        if isinstance(raw_images, list):
            result["images"] = [
                {**item, "preview_url": self.register_preview(item.get("url"))}
                for item in raw_images
            ]
        return result

    def fetch_preview(self, token: str) -> bytes:
        if not PREVIEW_TOKEN_RE.fullmatch(token):
            raise AppError("NOT_FOUND")
        now = time.monotonic()
        with self.lock:
            cached = self.preview_bytes.get(token)
            if cached and cached[0] > now:
                self.preview_bytes.move_to_end(token)
                return cached[1]
            entry = self.previews.get(token)
            if not entry or entry[0] <= now:
                self.previews.pop(token, None)
                self.preview_bytes.pop(token, None)
                raise AppError("NOT_FOUND")
            target_url = entry[1]
        raw = _fetch_public_image_bytes(target_url)
        encoded = _encode_preview_jpeg(raw)
        with self.lock:
            self.preview_bytes[token] = (time.monotonic() + PREVIEW_TTL_SECONDS, encoded)
            self.preview_bytes.move_to_end(token)
            while len(self.preview_bytes) > PREVIEW_MAX_CACHED_BLOBS:
                self.preview_bytes.popitem(last=False)
        return encoded

    def analyze(self, raw_url):
        url, _ = validate(raw_url)
        cached = self.recent(url)
        if cached is not None:
            return self.annotate_metadata(cached)
        if not self.slots.acquire(blocking=False):
            raise AppError("QUEUE_FULL")
        try:
            metadata = self.engine.analyze(url)
        finally:
            self.slots.release()
        with self.lock:
            self.cache[url] = (time.monotonic() + 300, metadata)
            self.cache.move_to_end(url)
            if len(self.cache) > 64:
                self.cache.popitem(last=False)
        return self.annotate_metadata(metadata)

    def recent(self, url):
        with self.lock:
            expires, metadata = self.cache.get(url, (0, None))
            return metadata if expires > time.monotonic() else None

    def options_for(self, request):
        metadata = self.recent(request["url"])
        if metadata is None:
            return None
        if request["kind"] == "audio":
            if request["container"] not in metadata["available_audio_formats"]:
                raise AppError("FORMAT_UNAVAILABLE")
        elif request["kind"] == "image":
            if request["container"] not in metadata.get("available_image_formats", []):
                raise AppError("FORMAT_UNAVAILABLE")
            index = request.get("image_index")
            if index is not None and index > len(metadata.get("images", [])):
                raise AppError("FORMAT_UNAVAILABLE")
        elif request["quality"] not in metadata["available_qualities"]:
            target = request["quality"]
            # The worker can select compatible heights outside the advertised presets.
            if not request["allow_quality_fallback"] or target == "best" or not metadata["supports_mp4"]:
                raise AppError("FORMAT_UNAVAILABLE")
        return metadata
