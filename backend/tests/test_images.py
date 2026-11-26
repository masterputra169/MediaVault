import io
import json
import zipfile
from pathlib import Path

import pytest
from fastapi.testclient import TestClient
from PIL import Image

from app.config import Config
from app.errors import AppError
from app.main import create_app
from app.services import files, images
from app.services.url_policy import image_url, validate


def _png_bytes(color=(40, 120, 220), size=(32, 24)):
    buf = io.BytesIO()
    Image.new("RGB", size, color).save(buf, format="PNG")
    return buf.getvalue()


def test_image_url_validation_and_reddit_direct():
    assert validate("https://i.redd.it/demo123.jpg") == ("https://i.redd.it/demo123.jpg", "reddit")
    assert image_url("https://scontent-sin6-1.cdninstagram.com/v/photo.jpg?stp=dst-jpg&oh=abc") is not None
    assert image_url("https://pbs.twimg.com/media/AbCdEf.jpg?name=orig") is not None
    assert image_url("https://i.pinimg.com/originals/aa/bb/cc.jpg") is not None
    assert image_url("https://evil.example/photo.jpg") is None
    assert image_url("http://i.pinimg.com/originals/aa/bb/cc.jpg") is None
    assert image_url("https://i.pinimg.com/originals/a.jpg#frag") is None


def test_extract_pinterest_twitter_reddit_tiktok_instagram(monkeypatch):
    # Pinterest
    pin_html = b'<meta property="og:title" content="Minimal Desk"><img src="https://i.pinimg.com/originals/aa/bb/cc/desk.jpg">'
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (pin_html, "text/html"))
    pin = images._extract_pinterest("https://www.pinterest.com/pin/123456789/")
    assert pin["media_id"] == "123456789"
    assert pin["title"] == "Minimal Desk"
    assert pin["images"][0]["url"] == "https://i.pinimg.com/originals/aa/bb/cc/desk.jpg"

    # Twitter multi-photo syndication
    tw_json = json.dumps({
        "text": "Two photos",
        "user": {"name": "Alice"},
        "mediaDetails": [
            {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/img1.jpg", "original_info": {"width": 800, "height": 600}},
            {"type": "photo", "media_url_https": "https://pbs.twimg.com/media/img2.jpg", "original_info": {"width": 1000, "height": 1000}},
        ],
    }).encode()
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (tw_json, "application/json"))
    tw = images._extract_twitter("https://x.com/alice/status/1800000000000000001")
    assert len(tw["images"]) == 2
    assert tw["uploader"] == "Alice"

    # Reddit gallery JSON
    rd_json = json.dumps([{"data": {"children": [{"data": {
        "title": "Gallery Post", "author": "bob", "is_gallery": True,
        "gallery_data": {"items": [{"media_id": "m1"}, {"media_id": "m2"}]},
        "media_metadata": {
            "m1": {"e": "Image", "s": {"u": "https://preview.redd.it/m1.jpg?auto=webp&amp;s=1", "x": 1200, "y": 800}},
            "m2": {"e": "Image", "s": {"u": "https://preview.redd.it/m2.jpg?auto=webp&amp;s=2", "x": 900, "y": 900}},
        },
    }}]}}]).encode()
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (rd_json, "application/json"))
    rd = images._extract_reddit("https://www.reddit.com/r/pics/comments/abc123/gallery/")
    assert len(rd["images"]) == 2
    assert rd["images"][0]["width"] == 1200

    # TikTok slideshow rehydration JSON
    tt_blob = json.dumps({"__DEFAULT_SCOPE__": {"webapp.video-detail": {"itemInfo": {"itemStruct": {
        "desc": "Slide deck",
        "author": {"nickname": "Creator"},
        "imagePost": {"images": [
            {"imageURL": {"urlList": ["https://p16-sign.tiktokcdn-us.com/slide1.jpeg?x=1"]}, "imageWidth": 1080, "imageHeight": 1920},
            {"imageURL": {"urlList": ["https://p16-sign.tiktokcdn-us.com/slide2.jpeg?x=2"]}, "imageWidth": 1080, "imageHeight": 1920},
        ]},
    }}}}}).encode()
    tt_html = b'<html><script id="__UNIVERSAL_DATA_FOR_REHYDRATION__" type="application/json">' + tt_blob + b"</script></html>"
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (tt_html, "text/html"))
    tt = images._extract_tiktok("https://www.tiktok.com/@creator/photo/7300000000000000001")
    assert len(tt["images"]) == 2
    assert tt["title"] == "Slide deck"

    # Instagram embed carousel JSON (fallback format)
    ig_ctx = json.dumps({"gql_data": {"shortcode_media": {
        "owner": {"username": "photographer"},
        "edge_media_to_caption": {"edges": [{"node": {"text": "Sunset"}}]},
        "edge_sidecar_to_children": {"edges": [
            {"node": {"is_video": False, "display_url": "https://scontent.cdninstagram.com/p1.jpg?oh=1", "dimensions": {"width": 1080, "height": 1350}}},
            {"node": {"is_video": False, "display_url": "https://scontent.cdninstagram.com/p2.jpg?oh=2", "dimensions": {"width": 1080, "height": 1350}}},
        ]},
    }}})
    ig_embed = ('"init",[],[' + json.dumps({"contextJSON": ig_ctx}) + ']],').encode()
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (ig_embed, "text/html"))
    ig = images._extract_instagram("https://www.instagram.com/p/AbCdEf123/")
    assert len(ig["images"]) == 2
    assert ig["uploader"] == "photographer"

    # Instagram Polaris RelayPrefetchedStreamCache (single photo + carousel)
    def _wrap_polaris(product):
        payload = {
            "require": [["ScheduledServerJS", "handle", None, [{
                "__bbox": {"require": [[
                    "RelayPrefetchedStreamCache", "next", [],
                    ["adp_key", {"__bbox": {"result": {"data": {"xig_polaris_media": {
                        "if_not_gated_logged_out": product,
                    }}}}}],
                ]]}
            }]]]
        }
        return (f'<script type="application/json" data-sjs>{json.dumps(payload)}</script>').encode()

    ig_single_html = _wrap_polaris({
        "__typename": "XIGPolarisImageMedia",
        "media_type": 1,
        "original_width": 2160,
        "original_height": 2704,
        "user": {"username": "neuron.ml", "full_name": "Neuron ML"},
        "caption": {"text": "Gradient descent by hand"},
        "image_versions2": {"candidates": [
            {"url": "https://scontent-sin2-3.cdninstagram.com/v/full.jpg?stp=dst-jpg_e35_tt6"},
            {"url": "https://scontent-sin2-3.cdninstagram.com/v/small.jpg?stp=dst-jpg_e35_p480x480_tt6"},
        ]},
    })
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (ig_single_html, "text/html"))
    ig_single = images._extract_instagram("https://www.instagram.com/p/DgLJAyrIBzH/")
    assert len(ig_single["images"]) == 1
    assert ig_single["images"][0]["url"].endswith("dst-jpg_e35_tt6")
    assert ig_single["images"][0]["width"] == 2160
    assert ig_single["images"][0]["height"] == 2704
    assert ig_single["title"] == "Gradient descent by hand"
    assert ig_single["uploader"] == "Neuron ML"

    ig_carousel_html = _wrap_polaris({
        "__typename": "XIGPolarisCarouselMedia",
        "media_type": 8,
        "user": {"username": "neuron.ml", "full_name": "Neuron ML"},
        "caption": {"text": "Carousel study"},
        "carousel_media": [
            {
                "media_type": 1, "original_width": 1080, "original_height": 1350,
                "image_versions2": {"candidates": [{"url": "https://scontent-sin2-3.cdninstagram.com/v/s1.jpg?stp=dst-jpg_e35_tt6"}]},
            },
            {
                "media_type": 2, "video_versions": [{"url": "https://scontent-sin2-3.cdninstagram.com/v/vid.mp4"}],
                "image_versions2": {"candidates": [{"url": "https://scontent-sin2-3.cdninstagram.com/v/poster.jpg"}]},
            },
            {
                "media_type": 1, "original_width": 1080, "original_height": 1350,
                "image_versions2": {"candidates": [{"url": "https://scontent-sin2-3.cdninstagram.com/v/s3.jpg?stp=dst-jpg_e35_tt6"}]},
            },
        ],
    })
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (ig_carousel_html, "text/html"))
    ig_car = images._extract_instagram("https://www.instagram.com/p/Dfhra2kiJLR/")
    assert [img["url"] for img in ig_car["images"]] == [
        "https://scontent-sin2-3.cdninstagram.com/v/s1.jpg?stp=dst-jpg_e35_tt6",
        "https://scontent-sin2-3.cdninstagram.com/v/s3.jpg?stp=dst-jpg_e35_tt6",
    ]


    assert images._gdl_meta_fields({
        "desc": "TikTok Slide",
        "author": {"nickname": "Creator TT", "uniqueId": "creatortt"},
        "id": 7704868167608290580,
    }) == ("TikTok Slide", "Creator TT", "7704868167608290580")

    from app.services.aliases import RESOLVED_ALIASES
    assert "vt.tiktok.com" in RESOLVED_ALIASES
    assert "vm.tiktok.com" in RESOLVED_ALIASES


def test_download_images_formats_zip_and_svg_rejection(monkeypatch, tmp_path):
    sample = _png_bytes()
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (sample, "image/png"))
    info = {
        "title": "Album",
        "images": [
            {"index": 1, "url": "https://i.pinimg.com/originals/a.png", "width": 32, "height": 24},
            {"index": 2, "url": "https://i.pinimg.com/originals/b.png", "width": 32, "height": 24},
        ],
    }
    events = []
    emit = lambda ev, **kw: events.append((ev, kw))

    for fmt in ("jpg", "png", "webp"):
        stage = tmp_path / fmt
        stage.mkdir()
        images.download_images(info, stage, {"kind": "image", "container": fmt, "image_index": 2}, 10 * 1024**2, emit)
        out = stage / f"media.{fmt}"
        assert out.is_file()
        files.validate_media(out, fmt, 10 * 1024**2, None)

    zip_stage = tmp_path / "zip"
    zip_stage.mkdir()
    images.download_images(info, zip_stage, {"kind": "image", "container": "zip", "image_index": None}, 10 * 1024**2, emit)
    zpath = zip_stage / "media.zip"
    assert zpath.is_file()
    files.validate_media(zpath, "zip", 10 * 1024**2, None)
    with zipfile.ZipFile(zpath) as zf:
        assert zf.namelist() == ["photo_01.jpg", "photo_02.jpg"]

    # Reject SVG payloads (XSS prevention)
    svg = b'<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>'
    monkeypatch.setattr(images, "_fetch_bytes", lambda url, **kw: (svg, "image/svg+xml"))
    bad_stage = tmp_path / "svg"
    bad_stage.mkdir()
    with pytest.raises(AppError) as exc:
        images.download_images(info, bad_stage, {"kind": "image", "container": "png", "image_index": 1}, 10 * 1024**2, emit)
    assert exc.value.code == "FORMAT_UNAVAILABLE"


def test_image_download_api_end_to_end(tmp_path):
    sample_jpg = images._encode_image(_png_bytes(), "jpg")

    class FakeImageEngine:
        def __init__(self, config, health, runtime):
            self.config = config

        def check_available(self, kind=None):
            pass

        def analyze(self, url):
            return {
                "url": url, "platform": "pinterest", "title": "Desk Setup", "uploader": None,
                "duration_seconds": None, "thumbnail_url": "https://i.pinimg.com/originals/a.jpg",
                "media_id": "123", "media_type": "image", "format_count": 2,
                "available_qualities": [], "has_audio": False, "supports_mp4": False,
                "available_audio_formats": [], "available_image_formats": ["jpg", "png", "webp", "zip"],
                "images": [
                    {"index": 1, "url": "https://i.pinimg.com/originals/a.jpg", "width": 32, "height": 24},
                    {"index": 2, "url": "https://i.pinimg.com/originals/b.jpg", "width": 32, "height": 24},
                ],
                "estimated_size_bytes": None, "warnings": [],
            }

        def download(self, row, stage, cancel, observe):
            (stage / f"media.{row['container']}").write_bytes(sample_jpg)
            return self.analyze(row["source_url"])

    cfg = Config(database=tmp_path / "app.db", download_dir=tmp_path / "dl", temp_dir=tmp_path / "tmp", port=8000)
    app = create_app(cfg, FakeImageEngine)
    with TestClient(app, base_url="http://127.0.0.1:8000") as client:
        an = client.post("/api/media/analyze", json={"url": "https://www.pinterest.com/pin/123/"})
        assert an.status_code == 200
        an_data = an.json()
        assert an_data["media_type"] == "image"
        assert "zip" in an_data["available_image_formats"]
        assert an_data["preview_url"].startswith("/api/media/preview/")
        assert an_data["images"][0]["preview_url"].startswith("/api/media/preview/")

        # Unknown / forged preview token must return 404 (no arbitrary URL proxy)
        assert client.get("/api/media/preview/not-a-valid-registered-token").status_code == 404

        # Valid token returns Pillow-reencoded JPEG thumbnail
        import app.services.media as media_mod
        orig_fetch = media_mod._fetch_public_image_bytes
        media_mod._fetch_public_image_bytes = lambda _u: sample_jpg
        try:
            pv = client.get(an_data["preview_url"])
            assert pv.status_code == 200
            assert pv.headers["content-type"] == "image/jpeg"
            assert pv.content[:2] == b"\xff\xd8"
        finally:
            media_mod._fetch_public_image_bytes = orig_fetch

        created = client.post("/api/downloads", json={
            "url": "https://www.pinterest.com/pin/123/",
            "kind": "image", "quality": "best", "container": "jpg", "image_index": 2,
        })
        assert created.status_code == 202
        job_id = created.json()["id"]

        for _ in range(50):
            detail = client.get(f"/api/downloads/{job_id}").json()
            if detail["status"] == "completed":
                break
            import time
            time.sleep(0.05)
        assert detail["status"] == "completed"
        assert detail["container"] == "jpg"
        assert detail["image_index"] == 2
        assert detail["file_available"] is True

        file_resp = client.get(f"/api/downloads/{job_id}/file")
        assert file_resp.status_code == 200
        assert file_resp.headers["content-type"] == "image/jpeg"
        assert file_resp.content == sample_jpg

        listed = client.get("/api/downloads?kind=image&limit=5&offset=0").json()
        assert listed["total"] == 1
        assert listed["counts"] == {"all": 1, "active": 0, "video": 0, "audio": 0, "image": 1}
        assert client.get("/api/downloads?kind=video").json()["total"] == 0
