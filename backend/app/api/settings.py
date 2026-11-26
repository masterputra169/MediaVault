from fastapi import APIRouter, Request

from ..schemas import Preferences
from .downloads import get_jobs

router = APIRouter()


def response(request, changes=None):
    jobs = get_jobs(request)
    config = request.app.state.config
    preferences = Preferences().model_dump() | jobs.repo.preferences(changes)
    download_dir = "storage/downloads" if config.public_mode else str(config.download_dir)
    return {"preferences": preferences, "download_dir": download_dir,
            "max_active_downloads": config.active, "max_queue_length": config.queue,
            "max_file_size_mb": config.max_bytes // 1024**2}


@router.get("/settings")
def get_settings(request: Request):
    return response(request)


@router.patch("/settings")
def update_settings(body: Preferences, request: Request):
    return response(request, body.model_dump(exclude_unset=True))
