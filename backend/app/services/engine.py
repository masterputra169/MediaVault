"""Subprocess entry point. Only sanitized JSON events cross back to FastAPI."""
import json
import os
import re
import signal
import socket
import subprocess
import sys
from pathlib import Path

from ..errors import AppError, extractor_error, storage_error
from . import images as image_service
from .aliases import resolve_alias
from .extractor import HLS_PROTOCOLS, LOCAL_FORMATS, clean_text, ensure_single, nonnegative, normalize, select, size
from .files import output_size, validate_media
from .url_policy import is_public, validate


def outbound_policy(event, args):
    if event == "socket.connect":
        sock, address = args
        if (sock.family not in {socket.AF_INET, socket.AF_INET6}
                or not isinstance(address, tuple) or not is_public(address[0])
                or address[1] not in {80, 443}):
            raise OSError("Media network connection rejected")


def emit(event, **values):
    print(json.dumps({"event": event, **values}, ensure_ascii=True), flush=True)


def convert(ffmpeg, source, target, args):
    command = [ffmpeg, "-nostdin", "-hide_banner", "-loglevel", "error", "-xerror", "-n",
               "-protocol_whitelist", "file,pipe", "-format_whitelist", LOCAL_FORMATS,
               "-i", str(source), *args, str(target)]
    try:
        subprocess.run(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
    except subprocess.CalledProcessError as error:
        raise extractor_error(error.stderr.decode("utf-8", "replace")) from None


def extract_audio(stage, request, maximum, ffmpeg):
    sources = [path for path in stage.iterdir() if re.fullmatch(r"media\.[A-Za-z0-9]{1,16}", path.name)]
    if len(sources) != 1:
        raise AppError("FORMAT_UNAVAILABLE")
    source = sources[0]
    output_size(source, maximum)
    extension = request["container"]
    target = stage / ("audio." + extension)
    if extension == "mp3":
        codec = ["-c:a", "libmp3lame", "-b:a", f'{request["audio_bitrate_kbps"] or 192}k']
    else:
        codec = ["-c:a", "aac"]
    convert(ffmpeg, source, target, ["-map", "0:a:0", "-vn", *codec])
    target.replace(stage / ("media." + extension))


class SafeLogger:
    def __init__(self):
        self.error_code = None
        self.warning_code = None

    def debug(self, message):
        pass

    def warning(self, message):
        code = extractor_error(message).code
        if code in {"LOGIN_REQUIRED", "GEO_RESTRICTED"}:
            self.warning_code = code

    def error(self, message):
        self.error_code = extractor_error(message).code


def execute(payload):
    from yt_dlp import YoutubeDL
    from yt_dlp.downloader.external import FFmpegFD
    from yt_dlp.postprocessor.ffmpeg import FFmpegPostProcessor

    class PublicDownloader(YoutubeDL):
        def build_request_director(self, handlers, preferences=None):
            # Only socket-backed urllib. Native curl can bypass Python audit hooks.
            return super().build_request_director(
                [handler for handler in handlers if handler.RH_KEY == "Urllib"], preferences)

        def process_ie_result(self, ie_result, *args, **kwargs):
            ensure_single(ie_result)
            return super().process_ie_result(ie_result, *args, **kwargs)

    def no_external_download(*args, **kwargs):
        # Native HLS may fall back to FFmpeg for unsupported playlists.
        raise AppError("FORMAT_UNAVAILABLE")

    def guarded_audio_codec(_self, path):
        if not (probe := payload.get("ffprobe")):
            return None
        result = subprocess.run([
            probe, "-v", "error", "-protocol_whitelist", "file,pipe",
            "-format_whitelist", LOCAL_FORMATS, "-select_streams", "a:0",
            "-show_entries", "stream=codec_name", "-of", "csv=p=0", path,
        ], capture_output=True, timeout=15)
        return result.stdout.decode("utf-8", "replace").strip() if result.returncode == 0 else None

    _orig_real_run_ffmpeg = FFmpegPostProcessor.real_run_ffmpeg
    _whitelist = ["-protocol_whitelist", "file,pipe", "-format_whitelist", LOCAL_FORMATS]

    def guarded_real_run_ffmpeg(self, input_path_opts, output_path_opts):
        safe_inputs = [
            (path, [*_whitelist, *(opts or [])])
            for path, opts in input_path_opts
        ]
        return _orig_real_run_ffmpeg(self, safe_inputs, output_path_opts)

    FFmpegFD.real_download = no_external_download
    FFmpegPostProcessor.get_audio_codec = guarded_audio_codec
    FFmpegPostProcessor.real_run_ffmpeg = guarded_real_run_ffmpeg
    sys.addaudithook(outbound_policy)

    url, platform = validate(payload["url"])
    resolved_url = resolve_alias(url, platform)
    mode = payload["mode"]
    logger = SafeLogger()
    allowed_extractors = [
        r"(?i)youtube.*", r"(?i)tiktok.*", r"(?i)instagram.*", r"(?i)facebook.*",
        r"(?i)twitter(?!:shortener).*", r"(?i)reddit.*", r"(?i)pinterest.*", r"(?i)vimeo.*", r"(?i)threads.*",
    ]
    runtime = payload.get("runtime")
    runtimes = {}
    if runtime:
        name, path = runtime.split(":", 1)
        runtimes[name] = {"path": path}
    options = {
        "quiet": True, "logger": logger, "noplaylist": True, "cachedir": False, "proxy": "",
        "enable_file_urls": False, "js_runtimes": runtimes, "remote_components": set(),
        "allowed_extractors": allowed_extractors, "socket_timeout": 15, "retries": 1,
        "fragment_retries": 1, "extractor_retries": 1, "hls_prefer_native": True,
        "fixup": "never", "dynamic_mpd": False, "allow_unplayable_formats": False,
        "postprocessor_args": {
            "ffmpeg_i": _whitelist,
            "ffprobe_i": _whitelist,
            "merger+ffmpeg_i": _whitelist,
            "merger+ffmpeg_i1": _whitelist,
            "merger+ffmpeg_i2": _whitelist,
            "merger+ffprobe_i": _whitelist,
        },
    }
    if payload.get("ffmpeg_location"):
        options["ffmpeg_location"] = payload["ffmpeg_location"]
    if mode == "analyze":
        result = None
        yt_error = None
        try:
            with PublicDownloader(options | {"skip_download": True, "ignore_no_formats_error": True}) as ydl:
                info = ydl.extract_info(resolved_url, download=False)
            result = normalize(info, url, platform, payload["ffmpeg"])
        except AppError as error:
            yt_error = error
        except Exception as error:
            yt_error = AppError(logger.error_code or extractor_error(str(error)).code)

        need_images = (
            result is None
            or not result["available_qualities"]
            or image_service.looks_like_photo_url(resolved_url, platform)
        )
        photo_info = image_service.extract_images(resolved_url, platform) if need_images else None
        if photo_info and photo_info.get("images"):
            photos = photo_info["images"]
            image_formats = list(image_service.IMAGE_CONTAINERS) + (["zip"] if len(photos) > 1 else [])
            if result is None:
                return dict(
                    url=url, platform=platform,
                    title=photo_info.get("title"), uploader=photo_info.get("uploader"),
                    duration_seconds=None, thumbnail_url=photos[0]["url"],
                    media_id=photo_info.get("media_id"), media_type="image",
                    format_count=len(photos), available_qualities=[],
                    has_audio=False, supports_mp4=False, available_audio_formats=[],
                    available_image_formats=image_formats, images=photos,
                    estimated_size_bytes=None, warnings=[],
                )
            result["images"] = photos
            result["available_image_formats"] = image_formats
            if not result["thumbnail_url"]:
                result["thumbnail_url"] = photos[0]["url"]
            if not result["title"] and photo_info.get("title"):
                result["title"] = photo_info["title"]
            if not result["uploader"] and photo_info.get("uploader"):
                result["uploader"] = photo_info["uploader"]
            if not result["media_id"] and photo_info.get("media_id"):
                result["media_id"] = photo_info["media_id"]
            if not result["available_qualities"]:
                result["media_type"] = "image"
                result["format_count"] = max(result["format_count"], len(photos))
                result["warnings"] = []
                if photo_info.get("title"):
                    result["title"] = photo_info["title"]
            return result
        if result is not None:
            if (not result["available_qualities"]
                    and not result["available_audio_formats"]
                    and not result.get("available_image_formats")):
                if logger.warning_code in {"LOGIN_REQUIRED", "GEO_RESTRICTED"}:
                    raise AppError(logger.warning_code)
            return result
        raise yt_error or AppError("EXTRACTOR_FAILED")

    request = payload["request"]
    maximum = payload["max_bytes"]
    stage = Path(payload["stage"])
    if request["kind"] == "image":
        photo_info = image_service.extract_images(resolved_url, platform)
        if not photo_info or not photo_info.get("images"):
            raise AppError("FORMAT_UNAVAILABLE")
        extension = request["container"]
        image_service.download_images(photo_info, stage, request, maximum, emit)
        source = stage / ("media." + extension)
        if not source.is_file():
            raise AppError("FORMAT_UNAVAILABLE")
        validate_media(source, extension, maximum, payload.get("ffprobe"))
        photos = photo_info["images"]
        image_formats = list(image_service.IMAGE_CONTAINERS) + (["zip"] if len(photos) > 1 else [])
        return dict(
            url=url, platform=platform,
            title=photo_info.get("title"), uploader=photo_info.get("uploader"),
            duration_seconds=None, thumbnail_url=photos[0]["url"],
            media_id=photo_info.get("media_id"), media_type="image",
            format_count=len(photos), available_qualities=[],
            has_audio=False, supports_mp4=False, available_audio_formats=[],
            available_image_formats=image_formats, images=photos,
            estimated_size_bytes=None, warnings=[],
        )

    remux_hls = False

    def selection(context):
        nonlocal remux_hls
        chosen = select(context, request, payload["ffmpeg"])
        if (estimate := size(chosen)) is not None and estimate > maximum:
            raise AppError("FILE_TOO_LARGE")
        emit("selection", tracks=len(chosen))
        remux_hls = request["kind"] == "video" and len(chosen) == 1 and chosen[0].get("protocol") in HLS_PROTOCOLS
        if len(chosen) == 1:
            yield chosen[0]
        else:
            video, audio = chosen
            yield {
                "format_id": f'{video["format_id"]}+{audio["format_id"]}',
                "ext": "mp4", "requested_formats": list(chosen),
                "protocol": f'{video["protocol"]}+{audio["protocol"]}',
            }

    def progress(event):
        if event.get("status") != "downloading":
            return
        emit("progress", **{key: nonnegative(event.get(key)) for key in (
            "downloaded_bytes", "total_bytes", "speed", "eta")},
            track=clean_text(event.get("info_dict", {}).get("format_id"), 80))

    def processing(event):
        if event.get("status") == "started":
            emit("processing")

    stage = Path(payload["stage"])
    options.update(
        format=selection, paths={"home": str(stage), "temp": str(stage)},
        outtmpl={"default": str(stage / "media.%(ext)s")},
        merge_output_format="mp4", progress_hooks=[progress], postprocessor_hooks=[processing],
        max_filesize=maximum, continuedl=False, overwrites=False,
    )
    if request["kind"] == "audio" and not payload.get("ffmpeg_exe"):
        raise AppError("FFMPEG_MISSING")
    try:
        with PublicDownloader(options) as ydl:
            info = ydl.extract_info(resolved_url, download=True)
    except AppError:
        raise
    except OSError as error:
        if error.errno in (28, 122):
            raise storage_error(error) from None
        raise AppError(logger.error_code or extractor_error(str(error)).code) from None
    except Exception as error:
        raise AppError(logger.error_code or extractor_error(str(error)).code) from None
    extension = request["container"]
    emit("processing")
    if request["kind"] == "audio":
        extract_audio(stage, request, maximum, payload["ffmpeg_exe"])
    source = stage / ("media." + extension)
    if not source.is_file():
        raise AppError("FORMAT_UNAVAILABLE")
    if remux_hls:
        output_size(source, maximum)
        target = stage / "remuxed.mp4"
        convert(payload["ffmpeg_exe"], source, target, ["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy"])
        target.replace(source)
    validate_media(source, extension, maximum, payload.get("ffprobe"))
    return normalize(info, url, platform, payload["ffmpeg"])


def stay_with_parent():
    if sys.platform == "linux" and os.environ.get("MEDIAVAULT_PARENT_PID"):
        import ctypes

        def stop_group(*_):
            os.killpg(os.getpgrp(), signal.SIGKILL)

        signal.signal(signal.SIGTERM, stop_group)
        if ctypes.CDLL(None).prctl(1, signal.SIGTERM, 0, 0, 0) != 0:
            raise OSError("Could not link worker lifecycle")
        if os.getppid() != int(os.environ["MEDIAVAULT_PARENT_PID"]):
            stop_group()


def main():
    try:
        stay_with_parent()
        payload = json.loads(sys.stdin.buffer.readline(65536))
        emit("result", data=execute(payload))
    except ImportError:
        emit("error", code="DEPENDENCY_MISSING")
    except AppError as error:
        emit("error", code=error.code)
    except Exception:
        emit("error", code="INTERNAL_ERROR")


if __name__ == "__main__":
    main()
