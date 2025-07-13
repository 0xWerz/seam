import ipaddress
from urllib.parse import urlparse

import requests


def is_public_host(url):
    host = urlparse(url).hostname or ""
    try:
        return not ipaddress.ip_address(host).is_private
    except ValueError:
        # Not an IP literal: a hostname is let through without being resolved.
        return True


def download(url):
    if not is_public_host(url):
        raise ValueError("private address")
    return requests.get(url, timeout=5).text
