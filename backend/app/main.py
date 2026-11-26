import logging
import sqlite3
from contextlib import asynccontextmanager
from urllib.parse import urlsplit

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from starlette.exceptions import HTTPException

from .api import auth, downloads, health, media, settings
from .config import Config
from .db.connection import Database
from .db.jobs import Repository
from .db.lock import InstanceLock
from .errors import AppError
from .services.auth import AuthService
from .services.dependency_checks import inspect
from .services.downloader import Engine
from .services.jobs import Jobs
from .services.media import MediaService

LOG = logging.getLogger("mediavault")
PUBLIC_AUTH_PATHS = {"/api/auth/session", "/api/auth/login", "/api/auth/logout"}


def _apply_security_headers(response):
    response.headers["Cache-Control"] = "no-store"
    response.headers["X-Content-Type-Options"] = "nosniff"
    response.headers["X-Frame-Options"] = "DENY"
    return response


def failure(error, status=None):
    return _apply_security_headers(
        JSONResponse({"error": error.payload()}, status_code=status or error.status)
    )


def create_app(config=None, engine_factory=Engine):
    config = config or Config.load()

    @asynccontextmanager
    async def lifespan(app):
        db = Database(config.database)
        app.state.config = config
        app.state.health, runtime = inspect(config)
        engine = engine_factory(config, app.state.health, runtime)
        app.state.media = MediaService(engine)
        app.state.jobs = None
        app.state.auth = None
        lock = InstanceLock(config.database)
        try:
            config.database.parent.mkdir(parents=True, exist_ok=True)
            lock.acquire()
            db.migrate()
            app.state.auth = AuthService(config, db)
            jobs = Jobs(config, Repository(db, config.queue), engine)
            jobs.start()
            app.state.jobs = jobs
        except (OSError, sqlite3.Error):
            lock.close()
            LOG.warning("MediaVault database unavailable or another dispatcher owns it.")
        try:
            yield
        finally:
            try:
                if app.state.jobs:
                    app.state.jobs.close()
            finally:
                lock.close()

    app = FastAPI(lifespan=lifespan, docs_url=None, redoc_url=None, openapi_url=None)

    @app.exception_handler(AppError)
    async def known_error(request, error):
        return failure(error)

    @app.exception_handler(RequestValidationError)
    async def invalid_request(request, error):
        invalid_url = any(entry["loc"][-1] == "url" for entry in error.errors() if entry["loc"])
        invalid_id = any(entry["loc"][-1] == "job_id" for entry in error.errors() if entry["loc"])
        code = "INVALID_URL" if invalid_url else "JOB_NOT_FOUND" if invalid_id else "INVALID_REQUEST"
        return failure(AppError(code))

    @app.exception_handler(HTTPException)
    async def http_error(request, error):
        return failure(AppError("NOT_FOUND" if error.status_code == 404 else "INVALID_REQUEST"), error.status_code)

    @app.middleware("http")
    async def local_boundary(request: Request, call_next):
        hosts = request.headers.getlist("host")
        origin = request.headers.getlist("origin")
        fetch_sites = [v.strip().lower() for v in request.headers.getlist("sec-fetch-site")]
        try:
            raw = hosts[0] if len(hosts) == 1 else ""
            host = urlsplit("//" + raw)
            valid_host = (
                host.hostname in {"127.0.0.1", "localhost", "::1"}
                and host.username is None and not (host.path or host.query or host.fragment)
                and host.netloc == raw and host.port in (None, config.port)
            )
        except ValueError:
            valid_host = False
        if not valid_host or len(fetch_sites) > 1 or any(v == "cross-site" for v in fetch_sites):
            return failure(AppError("FORBIDDEN"))
        if origin and (len(origin) != 1 or origin[0] not in config.origins):
            return failure(AppError("FORBIDDEN"))
        if config.auth_required and request.method in {"POST", "PATCH", "DELETE"}:
            if not origin or len(origin) != 1 or origin[0] not in config.origins:
                return failure(AppError("FORBIDDEN"))
        if request.method in {"POST", "PATCH"}:
            content_type = request.headers.get("content-type", "").partition(";")[0].strip().lower()
            if content_type != "application/json":
                return failure(AppError("FORBIDDEN"))
        if config.auth_required and request.method != "OPTIONS" and request.url.path not in PUBLIC_AUTH_PATHS:
            auth_service = getattr(request.app.state, "auth", None)
            if auth_service is None:
                return failure(AppError("STORAGE_UNAVAILABLE"))
            token = request.cookies.get(config.auth_cookie_name)
            if auth_service.authenticate(token) is None:
                return failure(AppError("UNAUTHORIZED"))
        try:
            response = await call_next(request)
        except Exception:
            LOG.error("Unhandled API failure")
            response = failure(AppError("INTERNAL_ERROR"))
        return _apply_security_headers(response)

    # The boundary runs before CORS so foreign Host/Origin is rejected on preflights too.
    from fastapi.middleware import Middleware
    app.user_middleware.append(Middleware(
        CORSMiddleware, allow_origins=list(config.origins), allow_credentials=config.auth_required,
        allow_methods=["GET", "POST", "DELETE", "PATCH"], allow_headers=["Content-Type"],
    ))
    for router in (auth.router, health.router, media.router, downloads.router, settings.router):
        app.include_router(router, prefix="/api")
    return app


app = create_app()
