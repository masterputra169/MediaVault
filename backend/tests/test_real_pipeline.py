import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.config import Config
from app.main import create_app
from app.services import downloader
from app.services.processes import run_engine
from test_jobs_api import BASE, HEADERS, wait_status


def test_real_ytdlp_ffmpeg_pipeline_keeps_outputs_and_history(tmp_path, monkeypatch):
    configured = os.environ.get("FFMPEG_PATH")
    directory = Path(configured) if configured else None
    if directory and directory.is_file():
        directory = directory.parent
    ffmpeg = shutil.which("ffmpeg", path=str(directory) if directory else None)
    ffprobe = shutil.which("ffprobe", path=str(directory) if directory else None)
    if not ffmpeg or not ffprobe:
        pytest.skip("FFmpeg and ffprobe not available")

    video = tmp_path / "video.mp4"
    audio = tmp_path / "audio.m4a"
    subprocess.run([ffmpeg, "-loglevel", "error", "-f", "lavfi", "-i", "color=blue:s=640x360:d=1",
                    "-c:v", "libx264", "-pix_fmt", "yuv420p", str(video)], check=True)
    subprocess.run([ffmpeg, "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=500:duration=1",
                    "-c:a", "aac", str(audio)], check=True)

    command = [sys.executable, str(Path(__file__).with_name("fixture_engine.py")), str(tmp_path)]
    monkeypatch.setattr(downloader, "run_engine",
                        lambda payload, timeout, **kwargs: run_engine(payload, timeout, command=command, **kwargs))
    config = Config(tmp_path / "pipeline.db", tmp_path / "downloads", tmp_path / "temp", ffmpeg_path=Path(ffmpeg).parent)
    url = "https://youtu.be/fixture"
    expected = [
        ({"kind": "video", "quality": "360", "container": "mp4"}, {"video": "h264", "audio": "aac"}),
        ({"kind": "audio", "quality": "best", "container": "mp3"}, {"audio": "mp3"}),
        ({"kind": "audio", "quality": "best", "container": "m4a"}, {"audio": "aac"}),
    ]
    job_ids = []
    with TestClient(create_app(config), base_url=BASE) as client:
        response = client.post("/api/media/analyze", json={"url": url}, headers=HEADERS)
        assert response.status_code == 200, response.text
        assert response.json()["available_qualities"] == ["best", "360"]
        assert response.json()["available_audio_formats"] == ["mp3", "m4a"]
        for options, codecs in expected:
            response = client.post("/api/downloads", json={"url": url, **options}, headers=HEADERS)
            assert response.status_code == 202, response.text
            job_id = response.json()["id"]
            job_ids.append(job_id)
            row = wait_status(client, job_id, "completed", timeout=45)
            file = config.download_dir / row["output_filename"]
            probe = subprocess.run([ffprobe, "-v", "error", "-of", "json", "-show_entries",
                                    "stream=codec_type,codec_name", str(file)], check=True, capture_output=True)
            assert {s["codec_type"]: s["codec_name"] for s in json.loads(probe.stdout)["streams"]} == codecs
            assert client.get(f"/api/downloads/{job_id}/file").content == file.read_bytes()
    with TestClient(create_app(config), base_url=BASE) as client:
        assert all(client.get(f"/api/downloads/{job}").json()["file_available"] for job in job_ids)
