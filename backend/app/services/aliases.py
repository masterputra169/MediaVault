from urllib.error import HTTPError, URLError
from urllib.parse import urljoin, urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from ..errors import AppError, extractor_error
from .url_policy import validate

RESOLVED_ALIASES = {"fb.watch", "fb.com", "pin.it", "redd.it", "t.co", "vt.tiktok.com", "vm.tiktok.com"}
REDIRECT_CODES = {301, 302, 303, 307, 308}


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        return None


def resolve_alias(url, platform):
    if urlsplit(url).hostname not in RESOLVED_ALIASES:
        return url
    opener = build_opener(ProxyHandler({}), NoRedirect())
    for _ in range(5):
        try:
            with opener.open(Request(url, headers={"User-Agent": "MediaVault/0.1"}), timeout=10):
                if urlsplit(url).hostname in RESOLVED_ALIASES:
                    raise AppError("UNSUPPORTED_PLATFORM")
                return url
        except HTTPError as error:
            with error:
                target = error.headers.get("Location")
                if error.code not in REDIRECT_CODES or not target:
                    raise extractor_error(f"HTTP error {error.code}") from None
                url, destination = validate(urljoin(url, target))
                if destination != platform:
                    raise AppError("UNSUPPORTED_PLATFORM")
            if urlsplit(url).hostname not in RESOLVED_ALIASES:
                return url
        except (URLError, OSError):
            raise AppError("NETWORK_ERROR") from None
    raise AppError("UNAVAILABLE_MEDIA")
