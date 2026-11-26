import logging
import threading
from uuid import UUID

from ..db.jobs import METADATA, now
from ..errors import AppError, storage_error
from ..schemas import ACTIVE
from . import files
from .downloader import Progress
from .processes import Cancelled

LOG = logging.getLogger("mediavault")


class Jobs:
    def __init__(self, config, repository, engine):
        self.config = config
        self.repo = repository
        self.engine = engine
        self.cv = threading.Condition()
        self.running = {}
        self.closed = False
        self.dispatch = threading.Thread(target=self._dispatch, daemon=True)

    def start(self):
        self.repo.recover()
        try:
            for item in self.config.temp_dir.iterdir():
                try:
                    if str(UUID(item.name)) == item.name:
                        # Only directories with a job in this database belong to this instance.
                        self.repo.get(item.name)
                        files.cleanup(self.config, item.name)
                except ValueError:
                    continue
                except AppError as error:
                    if error.code != "JOB_NOT_FOUND":
                        raise
        except OSError:
            pass
        self.dispatch.start()

    def close(self):
        with self.cv:
            self.closed = True
            running = list(self.running.values())
            for event, _ in running:
                event.set()
            self.cv.notify_all()
        self.dispatch.join()
        for _, thread in running:
            thread.join()

    def create(self, request, platform, metadata=None, retry_of=None):
        self.engine.check_available(request["kind"])
        with self.cv:
            if self.closed:
                raise AppError("INTERRUPTED")
            result = self.repo.enqueue(request, platform, metadata, retry_of)
            self.cv.notify()
            return result

    def retry(self, job_id):
        with self.cv:
            row = self.repo.get(job_id)
            if row["status"] in ACTIVE:
                raise AppError("JOB_STATE")
            return self.create(row | {"url": row["source_url"]}, row["platform"], row, retry_of=job_id)

    def cancel(self, job_id):
        with self.cv:
            row = self.repo.get(job_id)
            if row["status"] == "queued":
                self.repo.update(job_id, status="cancelled")
            elif job_id in self.running and row["status"] in ACTIVE:
                self.running[job_id][0].set()
            else:
                raise AppError("JOB_STATE")

    def delete(self, job_id, delete_file):
        with self.cv:
            row = self.repo.get(job_id)
            if row["status"] in ACTIVE:
                raise AppError("JOB_STATE")
            if delete_file and row["status"] == "completed":
                try:
                    files.output_path(self.config, row).unlink()
                except AppError as error:
                    if error.code != "FILE_NOT_FOUND":
                        raise
                except FileNotFoundError:
                    pass
                except OSError as error:
                    raise storage_error(error) from None
            self.repo.delete(job_id)

    def _dispatch(self):
        with self.cv:
            while not self.closed:
                job_id = self.repo.next_queued() if len(self.running) < self.config.active else None
                if job_id is None:
                    self.cv.wait()
                    continue
                event = threading.Event()
                thread = threading.Thread(target=self._run, args=(job_id, event), daemon=True)
                self.repo.update(job_id, status="downloading")
                self.running[job_id] = event, thread
                thread.start()

    def _run(self, job_id, stop):
        error = None
        result_path = None
        stage = None
        try:
            row = self.repo.get(job_id)
            stage = files.stage_for(self.config, job_id)
            progress = Progress(lambda **values: self.repo.update(job_id, **values), self.config.max_bytes)
            metadata = self.engine.download(row, stage, stop, progress)
            progress.finish()
            extension = row["container"]
            source = stage / ("media." + extension)
            size = files.output_size(source, self.config.max_bytes)
            name = files.filename(metadata.get("title"), metadata.get("media_id"), job_id, extension)
            if stop.is_set():
                raise Cancelled()
            # The copy may cross filesystems. Keep queue operations available during that work.
            result_path = files.publish(self.config, source, name)
            with self.cv:
                if stop.is_set():
                    raise Cancelled()
                self.repo.update(job_id, status="completed", output_path=str(result_path),
                                 output_filename=result_path.name, file_size_bytes=size, completed_at=now(),
                                 progress_percent=100, speed_bytes_per_second=None, eta_seconds=None,
                                 **{key: metadata.get(key) for key in METADATA})
                result_path = None
        except Cancelled:
            pass
        except AppError as caught:
            error = caught
        except OSError as caught:
            error = storage_error(caught)
        except Exception:
            error = AppError("INTERNAL_ERROR")
        finally:
            if result_path is not None:
                try:
                    result_path.unlink(missing_ok=True)
                except OSError:
                    LOG.warning("Output cleanup failed for job %s", job_id)
            if stage is not None:
                files.cleanup(self.config, job_id)
            with self.cv:
                try:
                    if self.repo.get(job_id)["status"] in ACTIVE:
                        if self.closed:
                            status, error = "interrupted", AppError("INTERRUPTED")
                        elif stop.is_set():
                            status, error = "cancelled", None
                        else:
                            status = "failed"
                            error = error or AppError("INTERNAL_ERROR")
                        if error:
                            LOG.warning("Job %s ended with %s", job_id, error.code)
                        self.repo.update(job_id, status=status, error_code=error.code if error else None,
                                         error_message=error.message if error else None,
                                         speed_bytes_per_second=None, eta_seconds=None)
                finally:
                    self.running.pop(job_id, None)
                    self.cv.notify_all()
