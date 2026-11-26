"""Run the production yt-dlp engine with a local fixture transport."""
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from yt_dlp import YoutubeDL
from yt_dlp.downloader.http import HttpFD
from yt_dlp.extractor.common import InfoExtractor

from app.services.engine import main


class FixtureIE(InfoExtractor):
    IE_NAME = "youtube:fixture"
    _VALID_URL = r"https://youtu\.be/(?P<id>fixture)"

    def _real_extract(self, url):
        return {
            "id": self._match_id(url), "title": "MediaVault Fixture", "duration": 1,
            "formats": [
                {"format_id": "video", "url": "https://media.example.invalid/video",
                 "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 360, "width": 640,
                 "filesize": (FIXTURES / "video.mp4").stat().st_size},
                {"format_id": "audio", "url": "https://media.example.invalid/audio",
                 "ext": "m4a", "vcodec": "none", "acodec": "mp4a",
                 "filesize": (FIXTURES / "audio.m4a").stat().st_size},
            ],
        }


def only_fixture(downloader):
    downloader.add_info_extractor(FixtureIE())


def copy_fixture(downloader, filename, info):
    source = FIXTURES / {"video": "video.mp4", "audio": "audio.m4a"}[info["format_id"]]
    size = source.stat().st_size
    shutil.copyfile(source, filename)
    downloader._hook_progress({"status": "downloading", "downloaded_bytes": size // 2, "total_bytes": size}, info)
    downloader._hook_progress({"status": "finished", "downloaded_bytes": size, "total_bytes": size}, info)
    return True


if __name__ == "__main__":
    FIXTURES = Path(sys.argv[1])
    YoutubeDL.add_default_info_extractors = only_fixture
    HttpFD.real_download = copy_fixture
    main()
