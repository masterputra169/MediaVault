import ipaddress

from fastapi import APIRouter, Request, Response

from ..errors import AppError
from ..schemas import Login

router = APIRouter(prefix="/auth")


def _auth_service(request: Request):
    service = getattr(request.app.state, "auth", None)
    if service is None:
        raise AppError("STORAGE_UNAVAILABLE")
    return service


def _client_key(request: Request) -> str:
    config = getattr(request.app.state, "config", None)
    if config is not None and getattr(config, "trust_proxy_headers", False):
        real_ip = (request.headers.get("x-real-ip") or "").strip()
        if real_ip:
            try:
                return str(ipaddress.ip_address(real_ip))
            except ValueError:
                pass
    return (request.client.host if request.client else "unknown")[:64]


@router.get("/session")
def session_status(request: Request):
    config = request.app.state.config
    if not config.auth_required:
        return {"auth_required": False, "authenticated": True, "username": None}
    service = _auth_service(request)
    token = request.cookies.get(config.auth_cookie_name)
    username = service.authenticate(token)
    return {
        "auth_required": True,
        "authenticated": username is not None,
        "username": username,
    }


@router.post("/login")
def login(body: Login, request: Request, response: Response):
    config = request.app.state.config
    if not config.auth_required:
        raise AppError("NOT_FOUND")
    service = _auth_service(request)
    token, max_age, username = service.login(body.username, body.password, _client_key(request))
    response.set_cookie(
        key=config.auth_cookie_name,
        value=token,
        max_age=max_age,
        path="/",
        httponly=True,
        secure=config.auth_cookie_secure,
        samesite="strict",
    )
    return {"auth_required": True, "authenticated": True, "username": username}


@router.post("/logout", status_code=204)
def logout(request: Request):
    config = request.app.state.config
    service = getattr(request.app.state, "auth", None)
    if config.auth_required and service is not None:
        token = request.cookies.get(config.auth_cookie_name)
        service.logout(token)
    response = Response(status_code=204)
    if config.auth_required:
        response.delete_cookie(
            key=config.auth_cookie_name,
            path="/",
            httponly=True,
            secure=config.auth_cookie_secure,
            samesite="strict",
        )
    return response
