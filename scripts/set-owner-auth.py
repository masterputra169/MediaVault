#!/usr/bin/env python3
"""Configure or rotate single-user owner authentication credentials in .env."""
import argparse
import getpass
import os
import secrets
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "backend"))

from app.services.auth import hash_password  # noqa: E402


def upsert_env(env_path: Path, updates: dict[str, str]) -> None:
    lines = env_path.read_text(encoding="utf-8").splitlines() if env_path.exists() else []
    seen: set[str] = set()
    out: list[str] = []
    for line in lines:
        key = line.split("=", 1)[0].strip() if "=" in line else ""
        if key in updates:
            out.append(f"{key}={updates[key]}")
            seen.add(key)
        else:
            out.append(line)
    for key, value in updates.items():
        if key not in seen:
            out.append(f"{key}={value}")
    content = ("\n".join(out) + "\n").encode("utf-8")
    if os.name != "nt":
        tmp_path = env_path.with_name(f".{env_path.name}.tmp.{os.getpid()}")
        fd = os.open(tmp_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        try:
            with os.fdopen(fd, "wb") as handle:
                handle.write(content)
            os.chmod(tmp_path, 0o600)
            os.replace(tmp_path, env_path)
        except Exception:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass
            raise
    else:
        env_path.write_bytes(content)


def main() -> int:
    parser = argparse.ArgumentParser(description="Set or rotate MediaVault single-user owner auth in .env")
    parser.add_argument("--env", default=str(ROOT / ".env"), help="Path to .env file")
    parser.add_argument("--username", default="owner", help="Owner login username (default: owner)")
    parser.add_argument("--generate", action="store_true", help="Generate a strong random password")
    parser.add_argument("--ttl-hours", type=int, default=168, help="Session TTL in hours (default: 168)")
    args = parser.parse_args()

    username = args.username.strip()
    if not username or len(username) > 64:
        print("Username harus 1 hingga 64 karakter.", file=sys.stderr)
        return 1

    if args.generate:
        password = secrets.token_urlsafe(18)
    else:
        password = getpass.getpass("Owner password (min 8 chars): ")
        confirm = getpass.getpass("Confirm password: ")
        if password != confirm:
            print("Password tidak cocok.", file=sys.stderr)
            return 1

    password_hash = hash_password(password)
    session_secret = secrets.token_hex(32)
    env_path = Path(args.env).resolve()
    upsert_env(env_path, {
        "AUTH_USERNAME": username,
        "AUTH_PASSWORD_HASH": password_hash,
        "AUTH_SESSION_SECRET": session_secret,
        "AUTH_SESSION_TTL_HOURS": str(args.ttl_hours),
    })
    print(f"Updated {env_path}")
    print(f"Username: {username}")
    if args.generate:
        print(f"Generated password: {password}")
    print("Restart mediavault-backend to apply.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
