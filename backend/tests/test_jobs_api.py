import threading
import time
from uuid import uuid4

import pytest
from fastapi.testclient import TestClient

from app.config import Config
from app.db.connection import Database
from app.db.jobs import Repository
from app.errors import AppError
from app.main import create_app
from app.services import processes
from app.services.downloader import Engine
from app.services.extractor import normalize

ORIGIN = "http://127.0.0.1:3000"
BASE = "http://127.0.0.1:8000"
HEADERS = {"Origin": ORIGIN, "Content-Type": "application/json"}
REQUEST = {"url": "https://youtu.be/demo", "kind": "video", "quality": "720", "container": "mp4"}
PAYLOAD = b"\x00\x00\x00\x18ftypmp42data"


class FakeEngine(Engine):
    def __init__(self, config, health, runtime):
        super().__init__(config, health, runtime)
        health["dependencies"]["yt_dlp"]["available"] = True
        self.release = threading.Event()
        self.release.set()
        self.started = threading.Event()
        self.processing = threading.Event()
        self.finish = threading.Event()
        self.finish.set()
        self.fail = None
        self.calls = 0

    def analyze(self, url):
        raw = {"title": "Hewan lokal", "id": "demo", "formats": [{
            "url": "https://media.example.invalid/video", "vcodec": "avc1", "acodec": "mp4a",
            "ext": "mp4", "height": 720,
        }]}
        return normalize(raw, url, "youtube", True)

    def download(self, row, stage, cancel, observe):
        self.calls += 1
        (stage / "media.mp4.part").write_bytes(b"partial")
        observe({"event": "progress", "downloaded_bytes": 50, "total_bytes": 100, "speed": 10, "eta": 5})
        self.started.set()
        while not self.release.wait(0.02):
            if cancel.is_set():
                raise processes.Cancelled()
        if self.fail:
            raise self.fail
        observe({"event": "processing"})
        self.processing.set()
        while not self.finish.wait(0.02):
            if cancel.is_set():
                raise processes.Cancelled()
        (stage / "media.mp4").write_bytes(PAYLOAD)
        return self.analyze(row["source_url"])


@pytest.fixture
def config(tmp_path):
    return Config(tmp_path / "app.db", tmp_path / "outputs", tmp_path / "stage",
                  active=1, queue=20, ffmpeg_path=tmp_path / "missing-ffmpeg")


@pytest.fixture
def client(config):
    with TestClient(create_app(config, FakeEngine), base_url=BASE) as client:
        yield client


def wait_status(client, job_id, status, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        response = client.get(f"/api/downloads/{job_id}")
        row = response.json()
        if row.get("status") == status:
            return row
        if row.get("status") in {"completed", "cancelled", "failed", "interrupted"}:
            break
        time.sleep(0.05)
    pytest.fail(f"Expected {status}, got {row}")


def enqueue(client):
    response = client.post("/api/downloads", json=REQUEST, headers=HEADERS)
    assert response.status_code == 202, response.text
    assert response.json()["status"] == "queued"
    return response.json()["id"]


def test_analyze_process_history_and_delete(client, config):
    response = client.post("/api/media/analyze", json={"url": REQUEST["url"] + "?token=secret"}, headers=HEADERS)
    assert response.status_code == 200, response.text
    data = response.json()
    assert data["title"] == "Hewan lokal"
    assert data["available_qualities"] == ["best", "720"]
    assert "secret" not in response.text
    assert "formats" not in data
    rejected = client.post("/api/downloads", json=REQUEST | {"quality": "1080"}, headers=HEADERS)
    assert rejected.json()["error"]["code"] == "FORMAT_UNAVAILABLE"
    engine = client.app.state.jobs.engine
    engine.release.clear()
    engine.finish.clear()
    job_id = enqueue(client)
    try:
        assert engine.started.wait(5)
        row = wait_status(client, job_id, "downloading")
        assert row["progress_percent"] == 50
        assert client.get(f"/api/downloads/{job_id}/file").status_code == 404
        assert client.delete(f"/api/downloads/{job_id}", headers=HEADERS).status_code == 409
        engine.release.set()
        row = wait_status(client, job_id, "processing")
        assert row["progress_percent"] == 50 and row["speed_bytes_per_second"] is None
    finally:
        engine.release.set()
        engine.finish.set()
    row = wait_status(client, job_id, "completed")
    assert row["file_size_bytes"] == len(PAYLOAD) and row["file_available"]
    assert "output_path" not in row and "storage" not in row
    media = client.get(f"/api/downloads/{job_id}/file")
    assert media.content == PAYLOAD
    assert media.headers["content-disposition"].startswith("attachment")
    path = config.download_dir / row["output_filename"]
    assert client.get(f"/api/downloads/{job_id}/location").json() == {"path": str(path)}
    index = client.get("/api/downloads?status=completed&search=hewan&platform=youtube&limit=1").json()
    assert index["total"] == 1 and index["items"][0]["id"] == job_id
    assert client.get("/api/downloads?search=absent").json()["total"] == 0
    assert client.get("/api/downloads?limit=51").status_code == 422
    assert client.delete(f"/api/downloads/{job_id}", headers=HEADERS).status_code == 204
    assert path.is_file()
    second = enqueue(client)
    row = wait_status(client, second, "completed")
    second_path = config.download_dir / row["output_filename"]
    assert client.delete(f"/api/downloads/{second}?delete_file=true", headers=HEADERS).status_code == 204
    assert not second_path.exists() and path.exists()


def test_active_limit_queue_capacity_cancel_and_retry(client, config):
    engine = client.app.state.jobs.engine
    engine.release.clear()
    first = enqueue(client)
    assert engine.started.wait(5)
    queue = [enqueue(client) for _ in range(20)]
    rejected = client.post("/api/downloads", json=REQUEST, headers=HEADERS)
    assert rejected.status_code == 429 and rejected.json()["error"]["code"] == "QUEUE_FULL"
    assert engine.calls == 1
    for job in queue:
        assert client.post(f"/api/downloads/{job}/cancel", headers=HEADERS).status_code == 202
        assert client.get(f"/api/downloads/{job}").json()["status"] == "cancelled"
    assert client.post(f"/api/downloads/{first}/cancel", headers=HEADERS).status_code == 202
    wait_status(client, first, "cancelled")
    assert not (config.temp_dir / first).exists()
    assert client.post(f"/api/downloads/{first}/cancel", headers=HEADERS).status_code == 409
    engine.release.set()
    retry = client.post(f"/api/downloads/{first}/retry", headers=HEADERS)
    assert retry.status_code == 202 and retry.json()["id"] != first
    assert wait_status(client, retry.json()["id"], "completed")["retry_of"] == first
    assert client.get(f"/api/downloads/{first}").json()["status"] == "cancelled"


def test_failure_timeout_and_missing_output(client, config):
    engine = client.app.state.jobs.engine
    engine.fail = AppError("TIMEOUT")
    first = enqueue(client)
    row = wait_status(client, first, "failed")
    assert row["error"]["code"] == "TIMEOUT"
    assert not (config.temp_dir / first).exists()
    engine.fail = None
    second = enqueue(client)
    row = wait_status(client, second, "completed")
    (config.download_dir / row["output_filename"]).unlink()
    assert client.get(f"/api/downloads/{second}/file").json()["error"]["code"] == "FILE_NOT_FOUND"
    assert not client.get(f"/api/downloads/{second}").json()["file_available"]
    assert client.get(f"/api/downloads/{uuid4()}").json()["error"]["code"] == "JOB_NOT_FOUND"
    assert client.get("/api/downloads/garbage").status_code == 404


def test_cors_health_and_settings(client):
    assert client.get("/api/health", headers={"Host": "foreign.test"}).status_code == 403
    denied = client.post("/api/media/analyze", json={"url": REQUEST["url"]}, headers={"Origin": "https://foreign.test"})
    assert denied.status_code == 403
    assert "access-control-allow-origin" not in denied.headers
    assert client.post("/api/media/analyze", data={"url": REQUEST["url"]}).status_code == 403
    preflight = {"Access-Control-Request-Method": "POST", "Access-Control-Request-Headers": "content-type"}
    assert client.options("/api/media/analyze", headers=preflight | {"Origin": "null"}).status_code == 403
    result = client.options("/api/media/analyze", headers=preflight | {"Origin": ORIGIN})
    assert result.status_code == 200 and result.headers["access-control-allow-origin"] == ORIGIN
    health = client.get("/api/health").json()
    assert health["status"] == "degraded" and not health["dependencies"]["ffmpeg"]["available"]
    assert health["queue_available"] and health["storage"]["writable"]
    response = client.patch("/api/settings", json={"theme": "light"}, headers=HEADERS)
    assert response.json()["preferences"] == {
        "theme": "light", "default_video_quality": "best", "default_audio_format": "mp3"}
    assert client.patch("/api/settings", json={"download_dir": "tmp"}, headers=HEADERS).status_code == 422
    assert client.post("/api/downloads", json=REQUEST | {"kind": "audio", "quality": "best", "container": "mp3"},
                       headers=HEADERS).json()["error"]["code"] == "FFMPEG_MISSING"


def test_restart_interrupts_active_jobs_and_keeps_history(config):
    config.temp_dir.mkdir()
    foreign = config.temp_dir / str(uuid4())
    foreign.mkdir()
    db = Database(config.database)
    db.migrate()
    repo = Repository(db, 20)
    from app.schemas import Create
    rows = [repo.enqueue(Create.model_validate(REQUEST).model_dump(), "youtube")["id"] for _ in range(4)]
    repo.update(rows[0], status="downloading")
    repo.update(rows[1], status="downloading")
    repo.update(rows[1], status="processing")
    repo.update(rows[2], status="cancelled")
    staging = config.temp_dir / rows[0]
    staging.mkdir()
    (staging / "media.part").write_bytes(b"test")
    with TestClient(create_app(config, FakeEngine), base_url=BASE) as client:
        for job in (rows[0], rows[1], rows[3]):
            assert client.get(f"/api/downloads/{job}").json()["status"] == "interrupted"
        assert client.get(f"/api/downloads/{rows[2]}").json()["status"] == "cancelled"
        assert not staging.exists() and foreign.exists()
        with pytest.raises(AppError, match="JOB_STATE"):
            repo.update(rows[2], status="downloading")
