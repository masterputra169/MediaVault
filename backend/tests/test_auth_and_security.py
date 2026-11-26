import os
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.config import Config
from app.main import create_app
from app.services import aliases, processes
from app.services.auth import hash_password
from tests.test_jobs_api import FakeEngine


def auth_config(tmp_path: Path, *, public=False):
    origins = (
        ("http://127.0.0.1:3000", "https://mediavault.example.com")
        if public
        else ("http://127.0.0.1:3000", "http://localhost:3000")
    )
    return Config(
        database=tmp_path / "app.db",
        download_dir=tmp_path / "downloads",
        temp_dir=tmp_path / "stage",
        origins=origins,
        port=8000,
        auth_username="owner",
        auth_password_hash=hash_password("SuperSecretPass123!"),
        auth_session_secret="a" * 64,
        auth_session_ttl_hours=24,
    )


def test_public_origin_requires_auth_config(monkeypatch, tmp_path):
    monkeypatch.setenv("FRONTEND_ORIGINS", "https://mediavault.example.com")
    monkeypatch.setenv("DATABASE_PATH", str(tmp_path / "app.db"))
    monkeypatch.setenv("DOWNLOAD_DIR", str(tmp_path / "downloads"))
    monkeypatch.setenv("TEMP_DIR", str(tmp_path / "stage"))
    monkeypatch.delenv("AUTH_USERNAME", raising=False)
    monkeypatch.delenv("AUTH_PASSWORD_HASH", raising=False)
    monkeypatch.delenv("AUTH_SESSION_SECRET", raising=False)
    with pytest.raises(ValueError, match="AUTH_USERNAME"):
        Config.load()


def test_single_user_login_session_logout_and_csrf_boundary(tmp_path):
    cfg = auth_config(tmp_path, public=True)
    app = create_app(cfg, FakeEngine)
    origin = {"Origin": "https://mediavault.example.com"}
    with TestClient(app, base_url="http://127.0.0.1:8000") as client:
        # Unauthenticated session check works, protected routes return 401
        sess = client.get("/api/auth/session")
        assert sess.status_code == 200
        assert sess.json() == {"auth_required": True, "authenticated": False, "username": None}
        for path in ("/api/health", "/api/settings", "/api/downloads"):
            res = client.get(path)
            assert res.status_code == 401
            assert res.headers["x-content-type-options"] == "nosniff"
            assert res.headers["cache-control"] == "no-store"

        # Origin-less mutating login is rejected when auth is enabled
        assert client.post("/api/auth/login", json={"username": "owner", "password": "SuperSecretPass123!"}).status_code == 403

        # Wrong credentials fail with 401
        bad = client.post("/api/auth/login", json={"username": "owner", "password": "wrong-password"}, headers=origin)
        assert bad.status_code == 401
        assert bad.json()["error"]["code"] == "INVALID_CREDENTIALS"

        # Correct credentials set HttpOnly SameSite=Strict Secure __Host- cookie
        ok = client.post("/api/auth/login", json={"username": "owner", "password": "SuperSecretPass123!"}, headers=origin)
        assert ok.status_code == 200
        assert ok.json() == {"auth_required": True, "authenticated": True, "username": "owner"}
        set_cookie = ok.headers["set-cookie"]
        assert "__Host-mv_session=" in set_cookie
        assert "HttpOnly" in set_cookie
        assert "SameSite=strict" in set_cookie or "SameSite=Strict" in set_cookie
        assert "Secure" in set_cookie

        cookie_val = set_cookie.split("__Host-mv_session=", 1)[1].split(";", 1)[0]
        auth_cookie = {"__Host-mv_session": cookie_val}

        # Authenticated endpoints work and mask server host paths in public mode
        assert client.get("/api/auth/session", cookies=auth_cookie).json()["authenticated"] is True
        health = client.get("/api/health", cookies=auth_cookie)
        assert health.status_code == 200
        assert health.json()["storage"]["download_dir"] == "storage/downloads"
        settings = client.get("/api/settings", cookies=auth_cookie)
        assert settings.status_code == 200
        assert settings.json()["download_dir"] == "storage/downloads"

        # Cross-site Sec-Fetch-Site is rejected even with valid cookie
        assert client.get("/api/health", cookies=auth_cookie, headers={"Sec-Fetch-Site": "cross-site"}).status_code == 403

        # Origin-less DELETE is rejected when auth is enabled
        assert client.delete(
            "/api/downloads/00000000-0000-0000-0000-000000000001",
            cookies=auth_cookie,
        ).status_code == 403

        # Logout invalidates server-side session in SQLite
        out = client.post("/api/auth/logout", json={}, cookies=auth_cookie, headers=origin)
        assert out.status_code == 204
        assert client.get("/api/health", cookies=auth_cookie).status_code == 401


def test_login_rate_limiting_locks_after_failures(tmp_path):
    cfg = auth_config(tmp_path, public=False)
    app = create_app(cfg, FakeEngine)
    origin = {"Origin": "http://127.0.0.1:3000"}
    with TestClient(app, base_url="http://127.0.0.1:8000") as client:
        for _ in range(5):
            r = client.post("/api/auth/login", json={"username": "owner", "password": "bad"}, headers=origin)
            assert r.status_code == 401
        locked = client.post(
            "/api/auth/login",
            json={"username": "owner", "password": "SuperSecretPass123!"},
            headers=origin,
        )
        assert locked.status_code == 429
        assert locked.json()["error"]["code"] == "RATE_LIMITED"


def test_worker_env_allowlist_strips_auth_secrets(monkeypatch, tmp_path):
    monkeypatch.setenv("AUTH_PASSWORD_HASH", "secret-hash")
    monkeypatch.setenv("AUTH_SESSION_SECRET", "super-secret")
    monkeypatch.setenv("NODE_OPTIONS", "--require=/tmp/evil.js")
    captured = {}

    class FakeProc:
        pid = 12345
        returncode = 0
        stdin = type("S", (), {"closed": False, "write": lambda self, b: None, "close": lambda self: setattr(self, "closed", True)})()
        stdout = type("O", (), {"readline": lambda self, n=-1: b"", "close": lambda self: None})()

        def poll(self):
            return 0

        def wait(self):
            return 0

        def kill(self):
            pass

    def fake_popen(cmd, **kwargs):
        captured["env"] = kwargs["env"]
        return FakeProc()

    monkeypatch.setattr(processes.subprocess, "Popen", fake_popen)
    monkeypatch.setattr(processes, "ProcessTree", lambda proc: type("T", (), {"stop": lambda self: None})())
    with pytest.raises(Exception):
        processes.run_engine({"mode": "analyze"}, 1)
    assert "AUTH_PASSWORD_HASH" not in captured["env"]
    assert "AUTH_SESSION_SECRET" not in captured["env"]
    assert "NODE_OPTIONS" not in captured["env"]
    assert captured["env"]["YTDLP_NO_PLUGINS"] == "1"


def test_alias_resolver_rejects_unresolved_200(monkeypatch):
    class FakeResp:
        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

    class FakeOpener:
        def open(self, req, timeout=10):
            return FakeResp()

    monkeypatch.setattr(aliases, "build_opener", lambda *a, **kw: FakeOpener())
    with pytest.raises(Exception) as exc:
        aliases.resolve_alias("https://t.co/abc123", "x")
    assert getattr(exc.value, "code", None) == "UNSUPPORTED_PLATFORM"


@pytest.mark.parametrize(
    "bad_origin",
    [
        ",,,",
        "https://192.168.1.1",
        "https://169.254.169.254",
        "https://a.b:notaport",
        "https://a.b:99999",
        "https://example.com:",
        "https://.",
        "https://*.example.com",
        "https://Example.COM",
        "https://example.com/",
        "http://example.com",
    ],
)
def test_config_rejects_malformed_or_ip_frontend_origins(monkeypatch, tmp_path, bad_origin):
    monkeypatch.setenv("FRONTEND_ORIGINS", bad_origin)
    monkeypatch.setenv("DATABASE_PATH", str(tmp_path / "app.db"))
    monkeypatch.setenv("DOWNLOAD_DIR", str(tmp_path / "downloads"))
    monkeypatch.setenv("TEMP_DIR", str(tmp_path / "stage"))
    monkeypatch.setenv("AUTH_USERNAME", "owner")
    monkeypatch.setenv("AUTH_PASSWORD_HASH", hash_password("SuperSecretPass123!"))
    monkeypatch.setenv("AUTH_SESSION_SECRET", "a" * 64)
    with pytest.raises(ValueError):
        Config.load()


def test_x_real_ip_ignored_without_proxy_trust_and_multi_sec_fetch_rejected(tmp_path):
    cfg = auth_config(tmp_path, public=False)
    app = create_app(cfg, FakeEngine)
    origin = {"Origin": "http://127.0.0.1:3000"}
    with TestClient(app, base_url="http://127.0.0.1:8000") as client:
        # Rotating spoofed X-Real-IP headers must NOT bypass per-client lockout when trust_proxy_headers=False
        for i in range(5):
            r = client.post(
                "/api/auth/login",
                json={"username": "owner", "password": "bad"},
                headers={**origin, "X-Real-IP": f"203.0.113.{i + 1}"},
            )
            assert r.status_code == 401
        locked = client.post(
            "/api/auth/login",
            json={"username": "owner", "password": "SuperSecretPass123!"},
            headers={**origin, "X-Real-IP": "203.0.113.99"},
        )
        assert locked.status_code == 429

        # Duplicate Sec-Fetch-Site headers must be rejected
        dup = client.get(
            "/api/auth/session",
            headers=[("Sec-Fetch-Site", "same-origin"), ("Sec-Fetch-Site", "cross-site")],
        )
        assert dup.status_code == 403


def test_invalid_scrypt_parameters_rejected():
    from app.services.auth import is_valid_password_hash

    salt = "A" * 22 + "=="
    dk = "A" * 43 + "="
    # Non-power-of-2 N
    assert not is_valid_password_hash(f"scrypt$3000$8$1${salt}${dk}")
    # Exceeds SCRYPT_MAXMEM
    assert not is_valid_password_hash(f"scrypt$65536$16$1${salt}${dk}")
    assert is_valid_password_hash(hash_password("ValidPass123!"))
