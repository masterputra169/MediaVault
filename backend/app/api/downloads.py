from typing import Annotated, Literal
from uuid import UUID

from fastapi import APIRouter, Query, Request, Response
from fastapi.responses import FileResponse

from ..errors import AppError
from ..schemas import Create, Status
from ..services.files import output_path
from ..services.url_policy import DOMAINS, validate

router = APIRouter(prefix="/downloads")


def get_jobs(request):
    if request.app.state.jobs is None:
        raise AppError("STORAGE_UNAVAILABLE")
    return request.app.state.jobs


def public_row(row, config, media=None):
    result = {key: value for key, value in row.items()
              if key not in {"output_path", "error_code", "error_message", "source_url"}}
    result["url"] = row["source_url"]
    result["allow_quality_fallback"] = bool(result["allow_quality_fallback"])
    result["error"] = AppError(row["error_code"]).payload() if row["error_code"] else None
    try:
        output_path(config, row)
        result["file_available"] = True
    except AppError:
        result["file_available"] = False
    if row["kind"] == "image" and result["file_available"] and row["container"] in {"jpg", "png", "webp"}:
        result["thumbnail_preview_url"] = f"/api/downloads/{row['id']}/file"
    elif media is not None:
        result["thumbnail_preview_url"] = media.register_preview(row.get("thumbnail_url"))
    else:
        result["thumbnail_preview_url"] = None
    return result


@router.post("", status_code=202)
def create(body: Create, request: Request):
    jobs = get_jobs(request)
    url, platform = validate(body.url)
    options = body.model_dump() | {"url": url}
    jobs.engine.check_available(body.kind)
    return jobs.create(options, platform, request.app.state.media.options_for(options))


@router.get("")
def list_jobs(
    request: Request, limit: Annotated[int, Query(ge=1, le=50)] = 50,
    offset: Annotated[int, Query(ge=0, lt=2**31)] = 0,
    status: Status | None = None, search: Annotated[str | None, Query(max_length=240)] = None,
    platform: str | None = None, kind: Literal["video", "audio", "image"] | None = None,
    active: bool = False,
):
    jobs = get_jobs(request)
    if platform is not None and platform not in DOMAINS:
        raise AppError("INVALID_REQUEST")
    rows, total, active_count, counts = jobs.repo.list(
        limit=limit, offset=offset, status=status, search=search, platform=platform, kind=kind, active=active)
    media = getattr(request.app.state, "media", None)
    return {"items": [public_row(row, jobs.config, media) for row in rows],
            "limit": limit, "offset": offset, "total": total, "active_count": active_count, "counts": counts}


@router.get("/{job_id}")
def detail(job_id: UUID, request: Request):
    jobs = get_jobs(request)
    media = getattr(request.app.state, "media", None)
    return public_row(jobs.repo.get(str(job_id)), jobs.config, media)


@router.post("/{job_id}/cancel", status_code=202)
def cancel(job_id: UUID, request: Request):
    get_jobs(request).cancel(str(job_id))
    return {"id": str(job_id)}


@router.post("/{job_id}/retry", status_code=202)
def retry(job_id: UUID, request: Request):
    return get_jobs(request).retry(str(job_id))


@router.get("/{job_id}/file")
def file(job_id: UUID, request: Request):
    jobs = get_jobs(request)
    row = jobs.repo.get(str(job_id))
    path = output_path(jobs.config, row)
    return FileResponse(path, filename=path.name, media_type={
        "mp4": "video/mp4",
        "mp3": "audio/mpeg",
        "m4a": "audio/mp4",
        "jpg": "image/jpeg",
        "png": "image/png",
        "webp": "image/webp",
        "zip": "application/zip",
    }[row["container"]])


@router.get("/{job_id}/location")
def location(job_id: UUID, request: Request):
    jobs = get_jobs(request)
    path = output_path(jobs.config, jobs.repo.get(str(job_id)))
    display = f"storage/downloads/{path.name}" if jobs.config.public_mode else str(path)
    return {"path": display}


@router.delete("/{job_id}", status_code=204)
def delete(job_id: UUID, request: Request, delete_file: bool = False):
    get_jobs(request).delete(str(job_id), delete_file)
    return Response(status_code=204)
