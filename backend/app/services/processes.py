import ctypes
import json
import os
import queue
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

from ..errors import AppError


class Cancelled(Exception):
    pass


class ProcessTree:
    def __init__(self, proc):
        self.proc = proc
        self.handle = None
        if os.name == "nt":
            self._attach_windows_job()

    def _attach_windows_job(self):
        from ctypes import wintypes as win

        class BasicLimits(ctypes.Structure):
            _fields_ = [
                ("process_time", ctypes.c_longlong), ("job_time", ctypes.c_longlong),
                ("flags", win.DWORD), ("min_working_set", ctypes.c_size_t),
                ("max_working_set", ctypes.c_size_t), ("active_processes", win.DWORD),
                ("affinity", ctypes.c_size_t), ("priority", win.DWORD), ("scheduling", win.DWORD),
            ]

        class ExtendedLimits(ctypes.Structure):
            _fields_ = [
                ("basic", BasicLimits), ("io", ctypes.c_ulonglong * 6),
                ("process_memory", ctypes.c_size_t), ("job_memory", ctypes.c_size_t),
                ("peak_process_memory", ctypes.c_size_t), ("peak_job_memory", ctypes.c_size_t),
            ]

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.CreateJobObjectW.argtypes = [ctypes.c_void_p, win.LPCWSTR]
        kernel.CreateJobObjectW.restype = win.HANDLE
        kernel.SetInformationJobObject.argtypes = [win.HANDLE, ctypes.c_int, ctypes.c_void_p, win.DWORD]
        kernel.OpenProcess.argtypes = [win.DWORD, win.BOOL, win.DWORD]
        kernel.OpenProcess.restype = win.HANDLE
        kernel.AssignProcessToJobObject.argtypes = [win.HANDLE, win.HANDLE]
        kernel.CloseHandle.argtypes = [win.HANDLE]

        handle = kernel.CreateJobObjectW(None, None)
        if not handle:
            raise ctypes.WinError(ctypes.get_last_error())
        process = kernel.OpenProcess(0x0100 | 0x0001, False, self.proc.pid)
        limits = ExtendedLimits()
        limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        try:
            if not (process and kernel.SetInformationJobObject(handle, 9, ctypes.byref(limits),
                                                               ctypes.sizeof(limits))
                    and kernel.AssignProcessToJobObject(handle, process)):
                raise ctypes.WinError(ctypes.get_last_error())
        except BaseException:
            kernel.CloseHandle(handle)
            raise
        finally:
            if process:
                kernel.CloseHandle(process)
        self.kernel, self.handle = kernel, handle

    def stop(self):
        if self.handle:
            self.kernel.CloseHandle(self.handle)
            self.handle = None
        elif os.name != "nt":
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def _read(stream, messages):
    try:
        while line := stream.readline(65536):
            if len(line) < 65536:
                try:
                    messages.put_nowait(line)
                except queue.Full:
                    # Progress is disposable. Ensure the terminal frame still reaches the consumer.
                    if b'"progress"' not in line:
                        messages.put(line, timeout=2)
    except (OSError, queue.Full, ValueError):
        pass


ALLOWED_WORKER_ENV = {
    "PATH", "SYSTEMROOT", "COMSPEC", "WINDIR", "PATHEXT",
    "TEMP", "TMP", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE",
    "HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA",
}


def run_engine(payload, timeout, *, cancel=None, observe=None, check=None, command=None):
    if cancel and cancel.is_set():
        raise Cancelled()
    cmd = command or [sys.executable, "-m", "app.services.engine"]
    settings = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {"start_new_session": True}
    clean_env = {k: v for k, v in os.environ.items() if k.upper() in ALLOWED_WORKER_ENV}
    clean_env.update(PYTHONUTF8="1", YTDLP_NO_PLUGINS="1", MEDIAVAULT_PARENT_PID=str(os.getpid()))
    proc = subprocess.Popen(
        cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        cwd=Path(__file__).resolve().parents[2], env=clean_env, shell=False, **settings,
    )
    tree = None
    reader = None
    messages = queue.Queue(maxsize=128)
    try:
        # The child waits for stdin, so it cannot start descendants before containment.
        tree = ProcessTree(proc)
        reader = threading.Thread(target=_read, args=(proc.stdout, messages), daemon=True)
        reader.start()
        proc.stdin.write(json.dumps(payload).encode("utf-8") + b"\n")
        proc.stdin.close()
        deadline = time.monotonic() + timeout
        next_check = 0.0
        result = None
        while True:
            now = time.monotonic()
            if cancel and cancel.is_set():
                raise Cancelled()
            if now >= deadline:
                raise AppError("TIMEOUT")
            if check and now >= next_check:
                check()
                next_check = now + 0.5
            try:
                event = json.loads(messages.get(timeout=0.1))
            except queue.Empty:
                if proc.poll() is not None and not reader.is_alive() and messages.empty():
                    break
                continue
            except (ValueError, UnicodeError):
                continue
            if not isinstance(event, dict):
                continue
            if event.get("event") == "error":
                from ..errors import ERRORS
                code = event.get("code")
                raise AppError(code if isinstance(code, str) and code in ERRORS else "EXTRACTOR_FAILED")
            if event.get("event") == "result":
                result = event.get("data")
            elif observe:
                observe(event)
        if proc.returncode != 0 or not isinstance(result, dict):
            raise AppError("EXTRACTOR_FAILED")
        return result
    finally:
        if tree is not None:
            tree.stop()
        if proc.poll() is None:
            proc.kill()
        proc.wait()
        if reader is not None:
            reader.join(timeout=3)
        if not proc.stdin.closed:
            proc.stdin.close()
        proc.stdout.close()
