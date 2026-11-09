#!/usr/bin/env python3
"""Minimal protocol mocks so Sonarr/Tanukarr accept fixture providers.

Binds to 127.0.0.1 only; reachable from rootless podman containers through
host.containers.internal (pasta maps it to the host loopback).
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

PORTS = [int(a) for a in sys.argv[1:]] or [18999]
BASE = "http://prowlarr:9696"

CAPS = """<?xml version="1.0" encoding="UTF-8"?>
<caps>
  <server version="1.0" title="Kitsunarr fixture mock"/>
  <limits max="100" default="100"/>
  <searching>
    <search available="yes" supportedParams="q"/>
    <tv-search available="yes" supportedParams="q,season,ep,tvdbid,imdbid,tvmazeid,rid"/>
    <movie-search available="yes" supportedParams="q,imdbid"/>
  </searching>
  <categories>
    <category id="5000" name="TV">
      <subcat id="5030" name="TV/SD"/>
      <subcat id="5040" name="TV/HD"/>
      <subcat id="5070" name="TV/Anime"/>
    </category>
    <category id="2000" name="Movies"><subcat id="2040" name="Movies/HD"/></category>
  </categories>
</caps>"""


def rss(kind):
    ns = ('xmlns:torznab="http://torznab.com/schemas/2015/feed"' if kind == "torznab"
          else 'xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/"')
    attr = "torznab:attr" if kind == "torznab" else "newznab:attr"
    mime = "application/x-bittorrent" if kind == "torznab" else "application/x-nzb"
    ext = "torrent" if kind == "torznab" else "nzb"
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom" {ns}>
<channel><title>Kitsunarr fixture mock</title><description>mock</description>
<item>
  <title>Breaking.Bad.S01E01.1080p.BluRay.x264-GROUP</title>
  <guid>{BASE}/details/1</guid>
  <link>{BASE}/dl/1.{ext}</link>
  <comments>{BASE}/details/1</comments>
  <pubDate>Mon, 09 Nov 2026 10:00:00 +0000</pubDate>
  <size>1500000000</size>
  <enclosure url="{BASE}/dl/1.{ext}" length="1500000000" type="{mime}"/>
  <{attr} name="category" value="5040"/>
  <{attr} name="seeders" value="10"/>
  <{attr} name="peers" value="12"/>
  <{attr} name="tvdbid" value="81189"/>
</item>
</channel></rss>"""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        sys.stderr.write("mock: " + (fmt % args) + "\n")

    def send(self, code, body, ctype="application/json", headers=None):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def handle_any(self):
        url = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(url.query).items()}
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        p = url.path
        # Torznab / Newznab
        if p.endswith("/api") and "t" in q:
            kind = "newznab" if "newznab" in p else "torznab"
            if q["t"] == "caps":
                return self.send(200, CAPS, "application/xml")
            return self.send(200, rss(kind), "application/rss+xml")
        # qBittorrent WebUI API v2
        if p == "/api/v2/auth/login":
            return self.send(200, "Ok.", "text/plain", {"Set-Cookie": "SID=fixture; HttpOnly; path=/"})
        if p == "/api/v2/app/webapiVersion":
            return self.send(200, "2.11.4", "text/plain")
        if p == "/api/v2/app/version":
            return self.send(200, "v5.0.4", "text/plain")
        if p == "/api/v2/app/preferences":
            return self.send(200, json.dumps({
                "save_path": "/downloads/", "max_ratio_enabled": False, "max_ratio": -1,
                "max_seeding_time_enabled": False, "max_seeding_time": -1, "max_ratio_act": 0,
                "queueing_enabled": False, "dht": True}))
        if p == "/api/v2/torrents/categories":
            return self.send(200, json.dumps({
                "tv-sonarr": {"name": "tv-sonarr", "savePath": "/downloads/tv-sonarr"},
                "tv-imported": {"name": "tv-imported", "savePath": "/downloads/tv-imported"}}))
        if p.startswith("/api/v2/torrents/"):
            return self.send(200, "[]" if p.endswith("/info") else "")
        # SABnzbd
        if p.endswith("/sabnzbd/api") or p == "/api":
            mode = q.get("mode")
            if mode == "version":
                return self.send(200, json.dumps({"version": "4.3.3"}))
            if mode == "get_config":
                return self.send(200, json.dumps({"config": {
                    "misc": {"complete_dir": "/downloads/complete", "pre_check": False, "history_retention": "",
                             "history_retention_option": "all", "history_retention_number": 0,
                             "enable_tv_sorting": False, "enable_movie_sorting": False,
                             "enable_date_sorting": False, "tv_categories": [], "movie_categories": [],
                             "date_categories": []},
                    "categories": [{"name": "*", "dir": "", "order": 0}, {"name": "tv", "dir": "tv", "order": 1}],
                    "sorters": []}}))
            if mode == "fullstatus":
                return self.send(200, json.dumps({"status": {"completedir": "/downloads/complete"}}))
            if mode == "queue":
                return self.send(200, json.dumps({"queue": {"paused": False, "slots": [], "speedlimit": "0"}}))
            if mode == "history":
                return self.send(200, json.dumps({"history": {"slots": []}}))
            return self.send(200, json.dumps({"status": True}))
        # Webhooks (generic, Discord, ...)
        return self.send(200, "{}")

    do_GET = handle_any
    do_POST = handle_any
    do_PUT = handle_any


if __name__ == "__main__":
    import threading
    servers = [ThreadingHTTPServer(("127.0.0.1", p), Handler) for p in PORTS]
    for srv in servers[1:]:
        threading.Thread(target=srv.serve_forever, daemon=True).start()
    servers[0].serve_forever()
