import base64
import hashlib
import hmac
import secrets
import threading
import time
from collections import deque
from datetime import datetime, timedelta, timezone

from ..errors import AppError

SCRYPT_N = 16384
SCRYPT_R = 8
SCRYPT_P = 1
SCRYPT_DKLEN = 32
SCRYPT_MAXMEM = 64 * 1024 * 1024
MAX_ACTIVE_SESSIONS = 10
CLIENT_MAX_FAILURES = 5
GLOBAL_MAX_FAILURES = 15
FAILURE_WINDOW_SECONDS = 300
LOCKOUT_SECONDS = 900
DUMMY_HASH = (
    "scrypt$16384$8$1$"
    "AAAAAAAAAAAAAAAAAAAAAA==$"
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
)


def _now():
    return datetime.now(timezone.utc)


def _iso(dt):
    return dt.isoformat(timespec="seconds")


def hash_password(password: str) -> str:
    if not isinstance(password, str) or len(password) < 8 or len(password) > 256:
        raise ValueError("Kata sandi harus 8 hingga 256 karakter.")
    salt = secrets.token_bytes(16)
    derived = hashlib.scrypt(
        password.encode("utf-8"), salt=salt, n=SCRYPT_N, r=SCRYPT_R, p=SCRYPT_P, dklen=SCRYPT_DKLEN,
    )
    salt_b64 = base64.b64encode(salt).decode("ascii")
    dk_b64 = base64.b64encode(derived).decode("ascii")
    return f"scrypt${SCRYPT_N}${SCRYPT_R}${SCRYPT_P}${salt_b64}${dk_b64}"


def is_valid_password_hash(encoded: str) -> bool:
    try:
        scheme, n_s, r_s, p_s, salt_b64, dk_b64 = encoded.split("$")
        if scheme != "scrypt":
            return False
        n, r, p = int(n_s), int(r_s), int(p_s)
        if (
            n < 1024
            or n > 65536
            or (n & (n - 1)) != 0
            or not (1 <= r <= 16 and 1 <= p <= 8)
            or 128 * n * r > SCRYPT_MAXMEM
        ):
            return False
        salt = base64.b64decode(salt_b64.encode("ascii"), validate=True)
        dk = base64.b64decode(dk_b64.encode("ascii"), validate=True)
        return len(salt) >= 16 and len(dk) >= 32
    except (ValueError, TypeError):
        return False


def verify_password(password: str, encoded: str) -> bool:
    try:
        scheme, n_s, r_s, p_s, salt_b64, dk_b64 = encoded.split("$")
        if scheme != "scrypt":
            return False
        n, r, p = int(n_s), int(r_s), int(p_s)
        if (
            n < 1024
            or n > 65536
            or (n & (n - 1)) != 0
            or not (1 <= r <= 16 and 1 <= p <= 8)
            or 128 * n * r > SCRYPT_MAXMEM
        ):
            return False
        salt = base64.b64decode(salt_b64.encode("ascii"), validate=True)
        expected = base64.b64decode(dk_b64.encode("ascii"), validate=True)
        derived = hashlib.scrypt(
            password.encode("utf-8"),
            salt=salt,
            n=n,
            r=r,
            p=p,
            maxmem=SCRYPT_MAXMEM,
            dklen=len(expected),
        )
        return hmac.compare_digest(derived, expected)
    except (ValueError, TypeError):
        return False


class AuthService:
    def __init__(self, config, db):
        self.config = config
        self.db = db
        self.lock = threading.Lock()
        self._login_lock = threading.Lock()
        self._client_failures: dict[str, deque[float]] = {}
        self._client_locked_until: dict[str, float] = {}
        self._global_failures: deque[float] = deque()
        self._global_locked_until = 0.0

    def _token_hash(self, token: str) -> str:
        secret = (self.config.auth_session_secret or "").encode("utf-8")
        return hmac.new(secret, token.encode("utf-8"), hashlib.sha256).hexdigest()

    def _check_rate_limit(self, client_key: str) -> None:
        now = time.monotonic()
        with self.lock:
            if now < self._global_locked_until or now < self._client_locked_until.get(client_key, 0.0):
                raise AppError("RATE_LIMITED")

    def _record_failure(self, client_key: str) -> None:
        now = time.monotonic()
        cutoff = now - FAILURE_WINDOW_SECONDS
        with self.lock:
            while self._global_failures and self._global_failures[0] < cutoff:
                self._global_failures.popleft()
            self._global_failures.append(now)
            if len(self._global_failures) >= GLOBAL_MAX_FAILURES:
                self._global_locked_until = now + LOCKOUT_SECONDS

            bucket = self._client_failures.setdefault(client_key, deque())
            while bucket and bucket[0] < cutoff:
                bucket.popleft()
            bucket.append(now)
            if len(bucket) >= CLIENT_MAX_FAILURES:
                self._client_locked_until[client_key] = now + LOCKOUT_SECONDS
            if len(self._client_failures) > 256:
                stale = [k for k, v in self._client_failures.items() if not v or v[-1] < cutoff]
                for key in stale:
                    self._client_failures.pop(key, None)
            if len(self._client_locked_until) > 256:
                expired_locks = [k for k, until in self._client_locked_until.items() if until <= now]
                for key in expired_locks:
                    self._client_locked_until.pop(key, None)

    def _clear_failures(self, client_key: str) -> None:
        with self.lock:
            self._client_failures.pop(client_key, None)
            self._client_locked_until.pop(client_key, None)

    def login(self, username: str, password: str, client_key: str) -> tuple[str, int, str]:
        if not self.config.auth_required:
            raise AppError("NOT_FOUND")
        with self._login_lock:
            self._check_rate_limit(client_key)
            expected_user = self.config.auth_username or ""
            expected_hash = self.config.auth_password_hash or DUMMY_HASH
            user_ok = hmac.compare_digest(username.strip(), expected_user)
            # Always run scrypt against expected_hash (same parameters) to prevent timing oracles.
            pass_ok = verify_password(password, expected_hash)
            if not (user_ok and pass_ok):
                self._record_failure(client_key)
                raise AppError("INVALID_CREDENTIALS")

            self._clear_failures(client_key)
        token = secrets.token_urlsafe(32)
        token_hash = self._token_hash(token)
        now = _now()
        ttl_seconds = self.config.auth_session_ttl_hours * 3600
        expires = now + timedelta(seconds=ttl_seconds)
        with self.db.connect(write=True) as conn:
            conn.execute("DELETE FROM auth_sessions WHERE expires_at <= ?", (_iso(now),))
            conn.execute(
                "INSERT INTO auth_sessions (token_hash, username, created_at, expires_at) VALUES (?, ?, ?, ?)",
                (token_hash, expected_user, _iso(now), _iso(expires)),
            )
            conn.execute(
                """
                DELETE FROM auth_sessions
                WHERE token_hash NOT IN (
                    SELECT token_hash FROM auth_sessions ORDER BY created_at DESC LIMIT ?
                )
                """,
                (MAX_ACTIVE_SESSIONS,),
            )
        return token, ttl_seconds, expected_user

    def authenticate(self, token: str | None) -> str | None:
        if not self.config.auth_required:
            return None
        if not token or len(token) > 256:
            return None
        token_hash = self._token_hash(token)
        now_iso = _iso(_now())
        with self.db.connect() as conn:
            row = conn.execute(
                "SELECT username, expires_at FROM auth_sessions WHERE token_hash = ?",
                (token_hash,),
            ).fetchone()
        if not row or row["expires_at"] <= now_iso:
            return None
        return row["username"]

    def logout(self, token: str | None) -> None:
        if not self.config.auth_required or not token or len(token) > 256:
            return
        token_hash = self._token_hash(token)
        with self.db.connect(write=True) as conn:
            conn.execute("DELETE FROM auth_sessions WHERE token_hash = ?", (token_hash,))
