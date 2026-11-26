import threading
import time

from fastapi import APIRouter, Request

from ..services.dependency_checks import storage

router = APIRouter()
STORAGE_CACHE_TTL = 5.0
_LOCK = threading.Lock()


def _cached_storage(state):
    now = time.monotonic()
    with _LOCK:
        cached = getattr(state, "_storage_cache", None)
        if cached is not None and cached[0] > now:
            return dict(cached[1])
        directories = storage(state.config)
        state._storage_cache = (now + STORAGE_CACHE_TTL, dict(directories))
        state.health["storage"] = dict(directories)
        return dict(directories)


@router.get("/health")
def get_health(request: Request):
    state = request.app.state
    directories = _cached_storage(state)
    public_directories = dict(directories)
    if state.config.public_mode:
        public_directories["download_dir"] = "storage/downloads"
    status = "ok" if directories["writable"] and state.jobs is not None and all(
        item["available"] for item in state.health["dependencies"].values()) else "degraded"
    return {
        **state.health,
        "storage": public_directories,
        "status": status,
        "queue_available": state.jobs is not None,
    }
