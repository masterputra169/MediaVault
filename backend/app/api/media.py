from fastapi import APIRouter, Request, Response

from ..schemas import Analyze

router = APIRouter()


@router.post("/media/analyze")
def analyze(body: Analyze, request: Request):
    return request.app.state.media.analyze(body.url)


@router.get("/media/preview/{token}")
def preview(token: str, request: Request):
    data = request.app.state.media.fetch_preview(token)
    return Response(
        content=data,
        media_type="image/jpeg",
        headers={
            "Cache-Control": "private, max-age=300",
            "X-Content-Type-Options": "nosniff",
        },
    )
