import importlib.util
import time

from ..errors import AppError
from . import files
from .dependency_checks import executable
from .processes import Cancelled, run_engine


class Engine:
    def __init__(self, config, health, runtime):
        self.config = config
        self.health = health
        self.runtime = runtime

    @property
    def can_convert(self):
        deps = self.health["dependencies"]
        return deps["ffmpeg"]["available"] and deps["ffprobe"]["available"]

    def check_available(self, kind=None):
        if not self.health["dependencies"]["yt_dlp"]["available"]:
            raise AppError("DEPENDENCY_MISSING")
        if kind and not self.health["storage"]["writable"]:
            raise AppError("STORAGE_UNAVAILABLE")
        if kind == "audio" and not self.can_convert:
            raise AppError("FFMPEG_MISSING")
        if kind == "image" and importlib.util.find_spec("PIL") is None:
            raise AppError("DEPENDENCY_MISSING")

    def payload(self, mode, url):
        return dict(mode=mode, url=url, runtime=self.runtime, ffmpeg=self.can_convert,
                    ffmpeg_location=str(self.config.ffmpeg_path) if self.config.ffmpeg_path else None)

    def analyze(self, url):
        self.check_available()
        return run_engine(self.payload("analyze", url), self.config.metadata_timeout)

    def download(self, row, stage, cancel, observe):
        self.check_available(row["kind"])
        payload = self.payload("download", row["source_url"]) | dict(
            request={key: row.get(key) for key in ("kind", "quality", "container", "audio_bitrate_kbps",
                                                   "allow_quality_fallback", "image_index")},
            stage=str(stage), max_bytes=self.config.max_bytes,
            ffprobe=executable(self.config, "ffprobe"),
            ffmpeg_exe=executable(self.config, "ffmpeg"),
        )
        return run_engine(payload, self.config.download_timeout, cancel=cancel, observe=observe,
                          check=lambda: files.check_capacity(self.config, stage))


class Progress:
    def __init__(self, report, maximum):
        self.report = report
        self.maximum = maximum
        self.tracks = {}
        self.expected = 1
        self.last_update = 0
        self.last_percent = 0
        self.processing = False

    def finish(self):
        if not self.processing:
            self.report(status="processing", speed_bytes_per_second=None, eta_seconds=None)
            self.processing = True

    def __call__(self, event):
        event_type = event.get("event")
        if event_type == "selection":
            self.expected = event["tracks"]
        if event_type == "processing":
            self.finish()
        if event_type != "progress" or self.processing:
            return
        key = event.get("track") or ""
        downloaded = min(int(event.get("downloaded_bytes") or 0), 2**63 - 1)
        total = event.get("total_bytes")
        previous = self.tracks.get(key, (0, None))
        self.tracks[key] = (max(previous[0], downloaded), int(total) if total else previous[1])
        current = sum(count for count, _ in self.tracks.values())
        if current > self.maximum:
            raise AppError("FILE_TOO_LARGE")
        totals = [length for _, length in self.tracks.values()]
        known = len(totals) == self.expected and all(totals)
        total_bytes = sum(totals) if known else None
        percent = min(100, round(100 * current / total_bytes, 1)) if total_bytes else None
        moment = time.monotonic()
        if moment - self.last_update < 0.5:
            return
        if percent is not None:
            percent = self.last_percent = max(self.last_percent, percent)
        self.report(progress_percent=percent, downloaded_bytes=current, total_bytes=total_bytes,
                    speed_bytes_per_second=event.get("speed"),
                    eta_seconds=int(event["eta"]) if event.get("eta") is not None and known else None)
        self.last_update = moment
