import ipaddress
import os
import re
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

from dotenv import dotenv_values

from .services.auth import is_valid_password_hash

ROOT = Path(__file__).resolve().parents[2]
LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1"}
_HOST_LABEL_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$")


def _is_loopback_origin(origin: str) -> bool:
    return urlsplit(origin).hostname in LOOPBACK_HOSTS


def _is_valid_public_hostname(host: str) -> bool:
    if not host or len(host) > 253 or host != host.lower():
        return False
    try:
        ipaddress.ip_address(host)
        return False
    except ValueError:
        pass
    labels = host.split(".")
    if len(labels) < 2:
        return False
    if not all(_HOST_LABEL_RE.match(label) for label in labels):
        return False
    # Reject numeric-only TLDs
    if labels[-1].isdigit():
        return False
    return True


def _validate_origin(origin: str) -> None:
    parsed = urlsplit(origin)
    if parsed.scheme not in {"http", "https"}:
        raise ValueError("FRONTEND_ORIGINS harus berupa origin loopback atau origin HTTPS yang valid.")
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("FRONTEND_ORIGINS harus berupa origin loopback atau origin HTTPS yang valid.")
    if parsed.path or parsed.query or parsed.fragment or parsed.netloc.endswith(":"):
        raise ValueError("FRONTEND_ORIGINS harus berupa origin loopback atau origin HTTPS yang valid.")
    try:
        port = parsed.port
    except ValueError as exc:
        raise ValueError("FRONTEND_ORIGINS memiliki port tidak valid.") from exc
    if port is not None and not (1 <= port <= 65535):
        raise ValueError("FRONTEND_ORIGINS memiliki port tidak valid.")
    if origin != f"{parsed.scheme}://{parsed.netloc}":
        raise ValueError("FRONTEND_ORIGINS harus berupa origin kanonik tanpa trailing slash.")
    host = parsed.hostname or ""
    if host in LOOPBACK_HOSTS:
        expected_host = f"[{host}]" if ":" in host else host
        expected = f"{parsed.scheme}://{expected_host}" + (f":{port}" if port is not None else "")
        if origin != expected:
            raise ValueError("FRONTEND_ORIGINS loopback harus huruf kecil dan kanonik.")
        return
    if parsed.scheme != "https" or not _is_valid_public_hostname(host):
        raise ValueError("FRONTEND_ORIGINS harus berupa origin loopback atau origin HTTPS yang valid.")
    expected = f"https://{host}" + (f":{port}" if port is not None else "")
    if origin != expected:
        raise ValueError("FRONTEND_ORIGINS publik harus huruf kecil dan kanonik.")


@dataclass(frozen=True)
class Config:
    database: Path
    download_dir: Path
    temp_dir: Path
    origins: tuple[str, ...] = ("http://127.0.0.1:3000", "http://localhost:3000")
    host: str = "127.0.0.1"
    port: int = 8000
    active: int = 2
    queue: int = 20
    max_bytes: int = 2048 * 1024**2
    metadata_timeout: int = 45
    download_timeout: int = 3600
    ffmpeg_path: Path | None = None
    auth_username: str | None = None
    auth_password_hash: str | None = None
    auth_session_secret: str | None = None
    auth_session_ttl_hours: int = 168
    trust_proxy_headers: bool = False

    @property
    def public_mode(self) -> bool:
        return any(not _is_loopback_origin(origin) for origin in self.origins)

    @property
    def auth_required(self) -> bool:
        return bool(self.auth_password_hash)

    @property
    def auth_cookie_secure(self) -> bool:
        return any(urlsplit(origin).scheme == "https" for origin in self.origins)

    @property
    def auth_cookie_name(self) -> str:
        return "__Host-mv_session" if self.auth_cookie_secure else "mv_session"

    @classmethod
    def load(cls):
        values = dotenv_values(ROOT / ".env") | dict(os.environ)

        def path(key, default):
            return (ROOT / (values.get(key) or default)).resolve()

        def positive(key, default, maximum=2**31):
            value = int(values.get(key) or default)
            if not 0 < value <= maximum:
                raise ValueError(f"Nilai {key} di luar batas.")
            return value

        raw_origins = values.get("FRONTEND_ORIGINS")
        if raw_origins is None:
            raw_origins = ",".join(cls.origins)
        origins = tuple(origin.strip() for origin in raw_origins.split(",") if origin.strip())
        if not origins:
            raise ValueError("FRONTEND_ORIGINS tidak boleh kosong.")
        for origin in origins:
            _validate_origin(origin)
        host = values.get("BACKEND_HOST") or cls.host
        if host not in {"127.0.0.1", "::1", "localhost"}:
            raise ValueError("BACKEND_HOST harus berupa alamat loopback.")
        auth_username = (values.get("AUTH_USERNAME") or "").strip() or None
        auth_password_hash = (values.get("AUTH_PASSWORD_HASH") or "").strip() or None
        auth_session_secret = (values.get("AUTH_SESSION_SECRET") or "").strip() or None
        public_mode = any(not _is_loopback_origin(origin) for origin in origins)
        any_auth = any((auth_username, auth_password_hash, auth_session_secret))
        if public_mode or any_auth:
            if not (auth_username and auth_password_hash and auth_session_secret):
                raise ValueError("AUTH_USERNAME, AUTH_PASSWORD_HASH, dan AUTH_SESSION_SECRET wajib diisi saat autentikasi atau origin publik aktif.")
            if len(auth_username) > 64:
                raise ValueError("AUTH_USERNAME terlalu panjang.")
            if not is_valid_password_hash(auth_password_hash):
                raise ValueError("AUTH_PASSWORD_HASH harus berformat scrypt yang valid.")
            if len(auth_session_secret) < 32:
                raise ValueError("AUTH_SESSION_SECRET minimal 32 karakter.")
        config = cls(
            database=path("DATABASE_PATH", "storage/app.db"),
            download_dir=path("DOWNLOAD_DIR", "storage/downloads"),
            temp_dir=path("TEMP_DIR", "storage/tmp"),
            origins=origins, host=host, port=positive("BACKEND_PORT", 8000, 65535),
            active=positive("MAX_ACTIVE_DOWNLOADS", 2, 32),
            queue=positive("MAX_QUEUE_LENGTH", 20, 1000),
            max_bytes=positive("MAX_FILE_SIZE_MB", 2048) * 1024**2,
            metadata_timeout=positive("METADATA_TIMEOUT_SECONDS", 45),
            download_timeout=positive("DOWNLOAD_TIMEOUT_SECONDS", 3600),
            ffmpeg_path=path("FFMPEG_PATH", "") if values.get("FFMPEG_PATH") else None,
            auth_username=auth_username,
            auth_password_hash=auth_password_hash,
            auth_session_secret=auth_session_secret,
            auth_session_ttl_hours=positive("AUTH_SESSION_TTL_HOURS", 168, 720),
            trust_proxy_headers=(values.get("TRUST_PROXY_HEADERS") or "").strip().lower() in {"1", "true", "yes"},
        )
        if (
            config.download_dir.is_relative_to(config.temp_dir)
            or config.temp_dir.is_relative_to(config.download_dir)
            or config.database.is_relative_to(config.temp_dir)
            or config.database.is_relative_to(config.download_dir)
        ):
            raise ValueError("Pisahkan folder staging, folder hasil, dan file database.")
        return config
