import errno
import json
import os
import re
import shutil
import stat
import subprocess
import unicodedata
from pathlib import Path
from tempfile import NamedTemporaryFile

from ..errors import AppError, storage_error
from .extractor import LOCAL_FORMATS

REPARSE = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
RESERVED = re.compile(r"^(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)", re.IGNORECASE)


def component(value, maximum):
    value = unicodedata.normalize("NFC", value or "")
    value = "".join("_" if char in '<>:"/\\|?*' or unicodedata.category(char).startswith("C")
                    else char for char in value)
    value = " ".join(value.split()).strip(" .") or "media"
    if RESERVED.match(value):
        value = "_" + value
    return value.encode("utf-8")[:maximum].decode("utf-8", "ignore").rstrip(". ") or "media"


def filename(title, media_id, job_id, extension):
    return f"{component(title, 135)}_{component(media_id, 40)}_{job_id[:8]}.{extension}"


def safe_stat(path):
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & REPARSE:
        raise OSError("Unexpected link")
    return info


def plain_root(root):
    if root.resolve() != root or not stat.S_ISDIR(safe_stat(root).st_mode):
        raise OSError("Unexpected directory")


def stage_for(config, job_id):
    plain_root(config.temp_dir)
    path = config.temp_dir / job_id
    path.mkdir(mode=0o700)
    return path


def cleanup(config, job_id):
    try:
        plain_root(config.temp_dir)
        target = config.temp_dir / job_id
        if stat.S_ISDIR(safe_stat(target).st_mode):
            shutil.rmtree(target)
    except OSError:
        pass


def check_capacity(config, stage):
    try:
        if min(shutil.disk_usage(stage).free, shutil.disk_usage(config.download_dir).free) < 1024**2:
            raise AppError("DISK_FULL")
        total = 0
        largest = 0
        for child in stage.iterdir():
            try:
                info = safe_stat(child)
            except FileNotFoundError:
                continue
            if stat.S_ISREG(info.st_mode):
                total += info.st_size
                largest = max(largest, info.st_size)
        # During a merge, source and merged bytes coexist.
        if largest > config.max_bytes or total > config.max_bytes * 2:
            raise AppError("FILE_TOO_LARGE")
    except OSError as error:
        raise storage_error(error) from None


def output_size(source, maximum):
    try:
        info = safe_stat(source)
    except OSError:
        raise AppError("FILE_NOT_FOUND") from None
    if not stat.S_ISREG(info.st_mode) or not info.st_size:
        raise AppError("FILE_NOT_FOUND")
    if info.st_size > maximum:
        raise AppError("FILE_TOO_LARGE")
    return info.st_size


def validate_media(source, extension, maximum, probe):
    try:
        output_size(source, maximum)
        if extension in {"jpg", "png", "webp"}:
            from PIL import Image, UnidentifiedImageError

            expected = {"jpg": "JPEG", "png": "PNG", "webp": "WEBP"}[extension]
            Image.MAX_IMAGE_PIXELS = 40_000_000
            try:
                with Image.open(source) as img:
                    valid = img.format == expected
                    img.verify()
            except (UnidentifiedImageError, Image.DecompressionBombError, OSError, ValueError):
                valid = False
        elif extension == "zip":
            import zipfile

            valid = False
            if zipfile.is_zipfile(source):
                with zipfile.ZipFile(source, "r") as archive:
                    names = archive.namelist()
                    valid = bool(names) and all(
                        re.fullmatch(r"photo_\d{2}\.(?:jpg|png|webp)", name) for name in names
                    )
        elif probe:
            result = subprocess.run([
                probe, "-v", "error", "-protocol_whitelist", "file,pipe",
                "-format_whitelist", LOCAL_FORMATS, "-show_entries",
                "stream=codec_type,codec_name:format=format_name", "-of", "json", str(source),
            ], capture_output=True, check=True, timeout=15)
            metadata = json.loads(result.stdout)
            streams = metadata.get("streams", [])
            tracks = {stream.get("codec_type") for stream in streams}
            formats = metadata.get("format", {}).get("format_name", "").split(",")
            if extension == "mp3":
                valid = "mp3" in formats and any(s.get("codec_name") == "mp3" for s in streams)
            else:
                required = "video" if extension == "mp4" else "audio"
                valid = "mp4" in formats and required in tracks and (extension != "m4a" or "video" not in tracks)
        else:
            with source.open("rb") as stream:
                header = stream.read(12)
            valid = header.startswith(b"ID3") or (len(header) > 1 and header[0] == 0xff
                    and header[1] & 0xe0 == 0xe0) if extension == "mp3" else header[4:8] == b"ftyp"
        if not valid:
            raise AppError("FORMAT_UNAVAILABLE")
    except (ValueError, subprocess.SubprocessError):
        raise AppError("FORMAT_UNAVAILABLE") from None
    except OSError:
        raise AppError("FILE_NOT_FOUND") from None


def publish(config, source, name):
    plain_root(config.download_dir)
    staging_copy = None
    try:
        for index in range(1, 1001):
            stem, extension = name.rsplit(".", 1)
            target = config.download_dir / (name if index == 1 else f"{stem}_{index}.{extension}")
            try:
                if staging_copy is None:
                    try:
                        os.link(source, target)
                        return target
                    except OSError as error:
                        if error.errno == errno.EEXIST:
                            continue
                        if error.errno not in {errno.EXDEV, errno.EPERM, errno.EACCES, errno.ENOSYS, errno.ENOTSUP}:
                            raise
                    with NamedTemporaryFile(dir=config.download_dir, prefix=".mediavault-", suffix=".tmp",
                                            delete=False) as copy:
                        staging_copy = Path(copy.name)
                        with source.open("rb") as original:
                            shutil.copyfileobj(original, copy)
                        copy.flush()
                        os.fsync(copy.fileno())
                if os.name == "nt":
                    os.rename(staging_copy, target)
                    staging_copy = None
                else:
                    os.link(staging_copy, target)
                return target
            except FileExistsError:
                continue
        raise AppError("STORAGE_UNAVAILABLE")
    except OSError as error:
        raise storage_error(error) from None
    finally:
        if staging_copy is not None:
            staging_copy.unlink(missing_ok=True)


def output_path(config, row):
    if row["status"] != "completed" or not row["output_path"] or not row["output_filename"]:
        raise AppError("FILE_NOT_FOUND")
    path = Path(row["output_path"])
    try:
        plain_root(config.download_dir)
        if (path.parent != config.download_dir or path.name != row["output_filename"]
                or path.suffix != "." + row["container"] or path.resolve(strict=True) != path
                or not stat.S_ISREG(safe_stat(path).st_mode)):
            raise AppError("FILE_NOT_FOUND")
    except (OSError, RuntimeError, ValueError):
        raise AppError("FILE_NOT_FOUND") from None
    return path
