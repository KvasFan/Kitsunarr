#!/usr/bin/env python3
"""Capture API responses and a real backup archive from a reference instance.

usage: python3 -I capture.py <sonarr|tanukarr> <outdir>
"""
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

SP = "/tmp/claude-1000/-home-tirael-Documenti-Kitsunarr/d15c3075-78a3-48fc-a456-a864450dce6f/scratchpad"
APP, OUT = sys.argv[1], sys.argv[2]
PORT = {"sonarr": 18989, "tanukarr": 18990}[APP]
with open(f"{SP}/live/{APP}-config/config.xml") as fh:
    KEY = re.search(r"<ApiKey>(.*)</ApiKey>", fh.read()).group(1)
os.makedirs(f"{OUT}/api", exist_ok=True)


def req(method, path, body=None, raw=False):
    r = urllib.request.Request(
        f"http://127.0.0.1:{PORT}{path}", method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"X-Api-Key": KEY, "Content-Type": "application/json"})
    with urllib.request.urlopen(r, timeout=120) as resp:
        data = resp.read()
        return data if raw else json.loads(data) if data else None


def save(name, path):
    try:
        data = req("GET", path)
    except urllib.error.HTTPError as err:
        print(f"  ! {path} -> {err.code}")
        return None
    with open(f"{OUT}/api/{name}.json", "w") as fh:
        json.dump(data, fh, indent=2, sort_keys=True)
    return data


endpoints = {
    "system-status": "/api/v3/system/status",
    "series": "/api/v3/series",
    "qualityprofile": "/api/v3/qualityprofile",
    "qualitydefinition": "/api/v3/qualitydefinition",
    "customformat": "/api/v3/customformat",
    "delayprofile": "/api/v3/delayprofile",
    "releaseprofile": "/api/v3/releaseprofile",
    "indexer": "/api/v3/indexer",
    "downloadclient": "/api/v3/downloadclient",
    "notification": "/api/v3/notification",
    "importlist": "/api/v3/importlist",
    "importlistexclusion": "/api/v3/importlistexclusion",
    "importlistexclusion-paged": "/api/v3/importlistexclusion/paged?page=1&pageSize=1000",
    "tag": "/api/v3/tag",
    "tag-detail": "/api/v3/tag/detail",
    "rootfolder": "/api/v3/rootfolder",
    "remotepathmapping": "/api/v3/remotepathmapping",
    "config-naming": "/api/v3/config/naming",
    "config-mediamanagement": "/api/v3/config/mediamanagement",
    "config-host": "/api/v3/config/host",
    "config-indexer": "/api/v3/config/indexer",
    "config-downloadclient": "/api/v3/config/downloadclient",
    "config-ui": "/api/v3/config/ui",
    "language": "/api/v3/language",
    "autotagging": "/api/v3/autotagging",
    "history": "/api/v3/history?page=1&pageSize=200&sortKey=date&sortDirection=descending",
}
if APP == "tanukarr":
    endpoints["config-metadatasource"] = "/api/v3/config/metadatasource"
for name, path in endpoints.items():
    save(name, path)
series = json.load(open(f"{OUT}/api/series.json"))
for s in series:
    save(f"episode-series{s['id']}", f"/api/v3/episode?seriesId={s['id']}")
    save(f"episodefile-series{s['id']}", f"/api/v3/episodefile?seriesId={s['id']}")

# Real backup archive produced by the application itself.
cmd = req("POST", "/api/v3/command", {"name": "Backup"})
for _ in range(60):
    st = req("GET", f"/api/v3/command/{cmd['id']}")
    if st["status"] in ("completed", "failed"):
        break
    time.sleep(1)
print("backup command:", st["status"])
backups = req("GET", "/api/v3/system/backup")
latest = sorted(backups, key=lambda b: b["time"])[-1]
blob = req("GET", latest["path"], raw=True)
with open(f"{OUT}/backup.zip", "wb") as fh:
    fh.write(blob)
print("backup:", latest["name"], len(blob), "bytes")
