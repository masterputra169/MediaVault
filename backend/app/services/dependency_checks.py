import importlib.metadata
import re
import shutil
import subprocess
import sys
from tempfile import NamedTemporaryFile


def package(name):
    try:
        return {"available": True, "version": importlib.metadata.version(name)}
    except importlib.metadata.PackageNotFoundError:
        return {"available": False, "version": None}


def binary(command, argument, minimum=None):
    result = {"available": False, "version": None}
    if command:
        try:
            run = subprocess.run([str(command), argument], capture_output=True, timeout=4, check=True)
            line = run.stdout.decode("utf-8", "replace").splitlines()[0]
            match = re.search(r"\d+(?:\.\d+){0,3}", line)
            if match:
                version = match.group()
                result.update(version=version, available=minimum is None or
                              tuple(map(int, version.split("."))) >= minimum)
        except (OSError, subprocess.SubprocessError, IndexError):
            pass
    return result


def executable(config, name):
    if config.ffmpeg_path is None:
        return shutil.which(name)
    directory = config.ffmpeg_path
    if directory.is_file():
        directory = directory.parent
    return shutil.which(name, path=str(directory))


def runtime():
    for name, minimum in (("deno", (2, 3)), ("node", (22,))):
        path = shutil.which(name)
        found = binary(path, "--version", minimum)
        if found["available"]:
            return found | {"name": name}, f"{name}:{path}"
    return {"name": None, "available": False, "version": None}, None


def storage(config):
    result = {"writable": False, "download_dir": str(config.download_dir),
              "free_bytes": None, "temp_free_bytes": None}
    try:
        for path in (config.temp_dir, config.download_dir, config.database.parent):
            path.mkdir(parents=True, exist_ok=True)
            with NamedTemporaryFile(dir=path):
                pass
        result.update(writable=True, free_bytes=shutil.disk_usage(config.download_dir).free,
                      temp_free_bytes=shutil.disk_usage(config.temp_dir).free)
    except OSError:
        pass
    return result


def inspect(config):
    js, js_arg = runtime()
    dependencies = {
        "python": {"available": sys.version_info >= (3, 11), "version": sys.version.split()[0]},
        "yt_dlp": package("yt-dlp"),
        "yt_dlp_ejs": package("yt-dlp-ejs"),
        "ffmpeg": binary(executable(config, "ffmpeg"), "-version"),
        "ffprobe": binary(executable(config, "ffprobe"), "-version"),
        "javascript_runtime": js,
    }
    directories = storage(config)
    status = "ok" if directories["writable"] and all(d["available"] for d in dependencies.values()) else "degraded"
    return {"status": status, "app_version": "0.1.0", "dependencies": dependencies,
            "storage": directories}, js_arg
