import socket
import sys
import threading
import time

import pytest

from app.config import Config
from app.errors import AppError
from app.services import files
from app.services.downloader import Progress
from app.services.engine import outbound_policy
from app.services.processes import Cancelled, run_engine


def test_name_publish_and_file_boundary(tmp_path):
    config = Config(tmp_path / "db", tmp_path / "out", tmp_path / "stage")
    config.download_dir.mkdir()
    source = tmp_path / "source"
    source.write_bytes(b"new data")
    name = files.filename('NUL: /../ 😀' * 99, "../escape:1", "abcdefgh", "mp4")
    assert len(name.encode()) < 200
    assert not any(c in name for c in ':?/\\') and name.endswith("_abcdefgh.mp4")
    assert files.component("NUL.mp4", 40) == "_NUL.mp4"
    output = config.download_dir / name
    output.write_bytes(b"keep")
    alternate = files.publish(config, source, name)
    assert alternate.read_bytes() == b"new data" and output.read_bytes() == b"keep"

    row = {"status": "completed", "output_path": str(alternate),
           "output_filename": alternate.name, "container": "mp4"}
    assert files.output_path(config, row) == alternate
    with pytest.raises(AppError, match="FILE_NOT_FOUND"):
        files.output_path(config, row | {"output_path": str(tmp_path / alternate.name)})
    with pytest.raises(AppError, match="FILE_NOT_FOUND"):
        files.output_path(config, row | {"status": "downloading"})


def test_symlink_cannot_be_served(tmp_path):
    config = Config(tmp_path / "db", tmp_path / "out", tmp_path / "stage")
    config.download_dir.mkdir()
    outside = tmp_path / "personal.mp4"
    outside.write_bytes(b"private")
    link = config.download_dir / outside.name
    try:
        link.symlink_to(outside)
    except OSError:
        pytest.skip("Symlink unavailable on this host")
    with pytest.raises(AppError, match="FILE_NOT_FOUND"):
        files.output_path(config, {"status": "completed", "output_path": str(link),
                                   "output_filename": link.name, "container": "mp4"})


def test_socket_guard_rejects_private_targets_before_connection():
    with socket.socket() as sock:
        with pytest.raises(OSError):
            outbound_policy("socket.connect", (sock, ("127.0.0.1", 80)))
        with pytest.raises(OSError):
            outbound_policy("socket.connect", (sock, ("1.1.1.1", 8080)))
        outbound_policy("socket.connect", (sock, ("1.1.1.1", 443)))


def test_multi_track_progress_stays_indeterminate_and_never_regresses():
    snapshots = []
    progress = Progress(lambda **values: snapshots.append(values), 1000)
    progress({"event": "selection", "tracks": 2})
    progress({"event": "progress", "track": "video", "downloaded_bytes": 100, "total_bytes": 100})
    assert snapshots[-1]["progress_percent"] is None
    progress.last_update = 0
    progress({"event": "progress", "track": "audio", "downloaded_bytes": 25, "total_bytes": 50})
    assert snapshots[-1]["progress_percent"] == 83.3
    progress.last_update = 0
    progress({"event": "progress", "track": "audio", "downloaded_bytes": 26, "total_bytes": 65})
    assert snapshots[-1]["progress_percent"] == 83.3
    progress.finish()
    assert snapshots[-1]["status"] == "processing"


def test_subprocess_result_and_timeout():
    cmd = [sys.executable, "-c", 'import json, sys; print(json.dumps({"event":"result","data":json.load(sys.stdin)}))']
    assert run_engine({"test": 42}, 5, command=cmd) == {"test": 42}
    with pytest.raises(AppError, match="TIMEOUT"):
        run_engine({}, 0.15, command=[sys.executable, "-c", "import sys,time; sys.stdin.read(); time.sleep(5)"])


def test_cancel_kills_child_and_descendants(tmp_path):
    marker = tmp_path / "unexpected"
    stop = threading.Event()
    command = [sys.executable, "-c", (
        "import json, subprocess, sys, time; json.load(sys.stdin); "
        "subprocess.Popen([sys.executable, '-c', "
        "'import pathlib, sys, time; time.sleep(1.5); pathlib.Path(sys.argv[1]).touch()', sys.argv[1]], "
        "stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); "
        "print('{\"event\":\"started\"}', flush=True); time.sleep(10)"
    ), str(marker)]
    with pytest.raises(Cancelled):
        run_engine({}, 5, command=command, cancel=stop, observe=lambda _: stop.set())
    time.sleep(1.65)
    assert not marker.exists()
