import ipaddress
import re
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

from ..errors import AppError

DOMAINS = {
    "youtube": ("youtube.com", "youtu.be"),
    "tiktok": ("tiktok.com",),
    "instagram": ("instagram.com",),
    "facebook": ("facebook.com", "fb.watch", "fb.com"),
    "x": ("x.com", "twitter.com", "t.co"),
    "reddit": ("reddit.com", "redd.it", "i.redd.it"),
    "pinterest": ("pinterest.com", "pin.it"),
    "vimeo": ("vimeo.com",),
    "threads": ("threads.net", "threads.com"),
}
SHORT_DOMAINS = {"youtu.be", "fb.watch", "fb.com", "t.co", "redd.it", "pin.it"}
QUERY_KEYS = {"youtube": {"v"}, "facebook": {"v", "id", "story_fbid", "video_id"}}
THUMBNAIL_HOSTS = ("ytimg.com", "ggpht.com", "fbcdn.net", "cdninstagram.com",
                   "twimg.com", "redd.it", "redditmedia.com", "pinimg.com", "vimeocdn.com",
                   "tiktokcdn.com", "tiktokcdn-us.com", "tiktokcdn-eu.com", "tiktokcdn-in.com",
                   "ibyteimg.com")
HOST_LABEL = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", re.ASCII)


def matches(host, domain):
    return host == domain or (domain not in SHORT_DOMAINS and host.endswith("." + domain))


def validate(url):
    url = url.strip()
    if not url or len(url) > 2048 or "\\" in url or any(c.isspace() or ord(c) < 32 or ord(c) == 127 for c in url):
        raise AppError("INVALID_URL")
    try:
        parsed = urlsplit(url)
        host = (parsed.hostname or "").lower()
        port = parsed.port
    except ValueError:
        raise AppError("INVALID_URL") from None
    if (parsed.scheme not in {"http", "https"} or not host or parsed.username is not None
            or port not in (None, 443 if parsed.scheme == "https" else 80)
            or not all(HOST_LABEL.fullmatch(label) for label in host.split("."))):
        raise AppError("INVALID_URL")
    platform = next((key for key, domains in DOMAINS.items()
                     if any(matches(host, domain) for domain in domains)), None)
    if not platform:
        raise AppError("UNSUPPORTED_PLATFORM")
    if platform == "youtube":
        if parsed.path.startswith(("/playlist", "/channel/", "/user/", "/c/", "/@")):
            raise AppError("UNAVAILABLE_MEDIA")
    query = urlencode([
        (key, value) for key, value in parse_qsl(parsed.query)
        if key in QUERY_KEYS.get(platform, ()) and re.fullmatch(r"[A-Za-z0-9_-]{1,100}", value)
    ])
    return urlunsplit((parsed.scheme, host, parsed.path or "/", query, "")), platform


def is_public(address):
    try:
        ip = ipaddress.ip_address(address)
    except ValueError:
        return False
    if ip.version == 6:
        if ip.ipv4_mapped:
            return is_public(str(ip.ipv4_mapped))
        if any(ip in ipaddress.ip_network(prefix) for prefix in ("64:ff9b::/96", "64:ff9b:1::/48", "2002::/16", "2001::/32")):
            return False
    return ip.is_global and not (ip.is_multicast or ip.is_reserved or ip.is_loopback or ip.is_link_local)


def _matches_host(host, domain):
    return host == domain or host.endswith("." + domain)


def thumbnail(url):
    if not isinstance(url, str) or len(url) > 2048 or "\\" in url or any(
        c.isspace() or ord(c) < 32 or ord(c) == 127 for c in url
    ):
        return None
    try:
        parsed = urlsplit(url)
        host = (parsed.hostname or "").lower()
        if (parsed.scheme == "https" and host and parsed.port in (None, 443)
                and parsed.username is None and not parsed.fragment
                and all(HOST_LABEL.fullmatch(label) for label in host.split("."))
                and any(_matches_host(host, d) for d in THUMBNAIL_HOSTS)):
            return urlunsplit((parsed.scheme, host, parsed.path or "/", parsed.query, ""))
    except ValueError:
        pass
    return None


def image_url(url):
    return thumbnail(url)
