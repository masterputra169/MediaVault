import pytest

from app.errors import AppError, extractor_error
from app.services.extractor import normalize
from app.services.url_policy import is_public, validate


@pytest.mark.parametrize("url", [
    "https://youtube.com.evil.tld/watch?v=x", "http://127.0.0.1", "http://10.0.0.1",
    "http://localhost", "ftp://youtube.com/x", "file:///a", "https://u:p@youtube.com/x",
    "https://youtube.com:8443/x", "https://youtube.com\\@example.com", "https://youtu.be/" + "a"*2048,
    "https://www.youtube.com/playlist?list=example",
])
def test_url_rejected(url):
    with pytest.raises(AppError):
        validate(url)


def test_official_url_sanitized():
    assert validate("https://www.YouTube.com/watch?v=demo&token=secret#fragment") == (
        "https://www.youtube.com/watch?v=demo", "youtube")
    assert validate("https://vm.tiktok.com/Zabc/")[1] == "tiktok"


@pytest.mark.parametrize("host", ["127.0.0.1", "10.0.0.2", "169.254.169.254", "::1", "::ffff:127.0.0.1",
                                  "224.0.0.1", "ff02::1", "64:ff9b::7f00:1"])
def test_non_public_addresses(host):
    assert not is_public(host)


def test_normalize_null_and_unavailable_quality():
    info = {"title": "Demo", "formats": [
        {"format_id": "v", "url": "https://media.example/v", "protocol": "https", "vcodec": "avc1",
         "acodec": "none", "height": 720, "ext": "mp4"},
        {"format_id": "a", "url": "https://media.example/a", "protocol": "https", "vcodec": "none",
         "acodec": "mp4a", "ext": "m4a"},
    ]}
    result = normalize(info, "https://youtu.be/demo", "youtube", True)
    assert result["available_qualities"] == ["best", "720"]
    assert result["duration_seconds"] is None
    assert result["thumbnail_url"] is None
    assert normalize({"formats": ["bad"]}, "https://youtu.be/demo", "youtube", True)["available_qualities"] == []
    assert normalize(info, "https://youtu.be/demo", "youtube", False)["available_qualities"] == []


@pytest.mark.parametrize("info", [None, {"_type": "playlist"}, {"entries": []}, {"is_live": True}])
def test_unsupported_response(info):
    with pytest.raises(AppError):
        normalize(info, "https://youtu.be/demo", "youtube", True)


def test_error_mapping_excludes_source_detail():
    error = extractor_error("Sign in to continue https://example.org/?key=secret")
    assert error.code == "LOGIN_REQUIRED"
    assert "secret" not in str(error.payload())


def test_instagram_reel_vp9_and_progressive_mp4_supported():
    info = {
        "id": "DgKrEuFSXL7",
        "title": "Video by maxims.dailylife",
        "thumbnail": "https://scontent-sin6-1.cdninstagram.com/v/t51.82787-15/demo.jpg?stp=dst-jpg&oh=abc",
        "formats": [
            {"format_id": "0", "url": "https://scontent.cdninstagram.com/0.mp4", "protocol": "https",
             "vcodec": None, "acodec": "none", "ext": "mp4"},
            {"format_id": "dash-720", "url": "https://scontent.cdninstagram.com/720.mp4", "protocol": "https",
             "vcodec": "vp09.00.31.08.00.01.01.01.00", "acodec": "none", "height": 1280, "width": 720, "ext": "mp4"},
            {"format_id": "dash-1080", "url": "https://scontent.cdninstagram.com/1080.mp4", "protocol": "https",
             "vcodec": "vp09.00.40.08.00.01.01.01.00", "acodec": "none", "height": 1920, "width": 1080, "ext": "mp4"},
        ],
    }
    result = normalize(info, "https://www.instagram.com/reel/DgKrEuFSXL7/", "instagram", True)
    assert result["supports_mp4"] is True
    assert result["available_qualities"] == ["best", "1080", "720"]
    assert result["warnings"] == []
    assert result["thumbnail_url"] == "https://scontent-sin6-1.cdninstagram.com/v/t51.82787-15/demo.jpg?stp=dst-jpg&oh=abc"


def test_safe_logger_captures_login_and_geo_warnings_only():
    from app.services.engine import SafeLogger

    logger = SafeLogger()
    logger.warning("[Instagram] There is no video in this post")
    assert logger.warning_code is None
    logger.warning("[youtube] Sign in to confirm you’re not a bot.")
    assert logger.warning_code == "LOGIN_REQUIRED"
    logger2 = SafeLogger()
    logger2.warning("This video is not available in your country")
    assert logger2.warning_code == "GEO_RESTRICTED"
