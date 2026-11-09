#!/usr/bin/env python3
"""Populate a throwaway Sonarr/Tanukarr instance with realistic configuration.

Used only to generate migration fixtures for Kitsunarr. All secrets are fake.
usage: python3 -I populate.py <sonarr|tanukarr>
"""
import json
import re
import sys
import time
import urllib.error
import urllib.request

SP = "/tmp/claude-1000/-home-tirael-Documenti-Kitsunarr/d15c3075-78a3-48fc-a456-a864450dce6f/scratchpad"
APP = sys.argv[1]
PORT = {"sonarr": 18989, "tanukarr": 18990}[APP]
with open(f"{SP}/live/{APP}-config/config.xml") as fh:
    KEY = re.search(r"<ApiKey>(.*)</ApiKey>", fh.read()).group(1)


def call(method, path, body=None, ok_errors=False):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        f"http://127.0.0.1:{PORT}{path}",
        data=data,
        method=method,
        headers={"X-Api-Key": KEY, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as err:
        msg = err.read().decode(errors="replace")[:800]
        if ok_errors:
            print(f"  ! {method} {path} -> {err.code}: {msg}")
            return None
        raise SystemExit(f"{method} {path} -> {err.code}: {msg}")


def set_fields(resource, values):
    names = {f["name"] for f in resource["fields"]}
    for k in values:
        if k not in names:
            print(f"  ! field {k} not in schema of {resource['implementation']}: {sorted(names)}")
    for f in resource["fields"]:
        if f["name"] in values:
            f["value"] = values[f["name"]]
    return resource


def from_schema(kind, implementation, name, values, **extra):
    schema = call("GET", f"/api/v3/{kind}/schema")
    res = next(s for s in schema if s["implementation"] == implementation)
    res = set_fields(res, values)
    res["name"] = name
    res.update(extra)
    return call("POST", f"/api/v3/{kind}?forceSave=true", res, ok_errors=True)


print(f"== populating {APP}")
for p in ["/media/tv", "/media/anime"] + (["/media/movies"] if APP == "tanukarr" else []):
    call("POST", "/api/v3/rootfolder", {"path": p}, ok_errors=True)

tags = {}
for label in ["4k", "anime", "kids", "requested"]:
    t = call("POST", "/api/v3/tag", {"label": label}, ok_errors=True)
    if t:
        tags[label] = t["id"]
print("tags", tags)

# Custom formats
cfs = [
    {"name": "x265", "includeCustomFormatWhenRenaming": True, "specifications": [
        {"name": "x265/HEVC", "implementation": "ReleaseTitleSpecification", "negate": False, "required": True,
         "fields": [{"name": "value", "value": r"[xh][ ._-]?265|\bHEVC\b"}]}]},
    {"name": "Remux", "includeCustomFormatWhenRenaming": False, "specifications": [
        {"name": "Remux", "implementation": "SourceSpecification", "negate": False, "required": True,
         "fields": [{"name": "value", "value": 7}]}]},
    {"name": "Not English", "includeCustomFormatWhenRenaming": False, "specifications": [
        {"name": "English", "implementation": "LanguageSpecification", "negate": True, "required": True,
         "fields": [{"name": "value", "value": 1}]}]},
    {"name": "Bad Groups", "includeCustomFormatWhenRenaming": False, "specifications": [
        {"name": "BadGroup", "implementation": "ReleaseGroupSpecification", "negate": False, "required": False,
         "fields": [{"name": "value", "value": r"^(EVO|YIFY)$"}]},
        {"name": "Small", "implementation": "SizeSpecification", "negate": False, "required": False,
         "fields": [{"name": "min", "value": 0}, {"name": "max", "value": 0.4}]}]},
]
cf_ids = {}
for cf in cfs:
    r = call("POST", "/api/v3/customformat", cf, ok_errors=True)
    if r:
        cf_ids[cf["name"]] = r["id"]
print("custom formats", cf_ids)

# Quality profile: copy the first default profile and customise it.
profiles = call("GET", "/api/v3/qualityprofile")
base = json.loads(json.dumps(profiles[0]))
base.pop("id", None)
base["name"] = "Kitsunarr Fixture 1080p"
base["upgradeAllowed"] = True
base["minFormatScore"] = -100
base["cutoffFormatScore"] = 500
for fi in base.get("formatItems", []):
    fi["score"] = {"x265": 100, "Remux": 50, "Not English": -1000, "Bad Groups": -500}.get(fi["name"], 0)
for item in base["items"]:
    q = item.get("quality")
    if q and q["name"] in ("HDTV-1080p", "WEBDL-1080p", "Bluray-1080p", "WEBRip-1080p"):
        item["allowed"] = True
    for sub in item.get("items", []):
        if sub.get("quality", {}).get("name") in ("WEBDL-1080p", "WEBRip-1080p"):
            sub["allowed"] = True
            item["allowed"] = True
custom_profile = call("POST", "/api/v3/qualityprofile", base, ok_errors=True)
profile_id = custom_profile["id"] if custom_profile else profiles[0]["id"]
anime_profile_id = profiles[-1]["id"]
print("profiles", [(p["id"], p["name"]) for p in call("GET", "/api/v3/qualityprofile")])

# Quality definition tweak
qdefs = call("GET", "/api/v3/qualitydefinition")
for qd in qdefs:
    if qd["quality"]["name"] == "WEBDL-1080p":
        qd["minSize"] = 5
        qd["maxSize"] = 120
        call("PUT", f"/api/v3/qualitydefinition/{qd['id']}", qd, ok_errors=True)

# Indexers (fake endpoints, forceSave skips the connection test)
from_schema("indexer", "Torznab", "Prowlarr Nyaa (fixture)", {
    "baseUrl": "http://prowlarr:9696/1/", "apiPath": "/api", "apiKey": "fixture-torznab-api-key-0001",
    "categories": [5000, 5040], "animeCategories": [5070], "minimumSeeders": 3,
}, enableRss=True, enableAutomaticSearch=True, enableInteractiveSearch=True, priority=10, tags=[tags.get("anime")])
from_schema("indexer", "Newznab", "NZBgeek (fixture)", {
    "baseUrl": "http://nzbindexer:8090/newznab/", "apiPath": "/api", "apiKey": "fixture-newznab-api-key-0002",
    "categories": [5030, 5040],
}, enableRss=True, enableAutomaticSearch=False, enableInteractiveSearch=True, priority=25, tags=[])

# Download clients
from_schema("downloadclient", "QBittorrent", "qBittorrent (fixture)", {
    "host": "qbittorrent", "port": 8080, "useSsl": False, "urlBase": "", "username": "admin",
    "password": "fixture-qbit-password", "tvCategory": "tv-sonarr", "tvImportedCategory": "tv-imported",
    "recentTvPriority": 0, "olderTvPriority": 0, "initialState": 0, "sequentialOrder": False, "firstAndLast": False,
}, enable=True, priority=1, removeCompletedDownloads=True, removeFailedDownloads=True, tags=[])
from_schema("downloadclient", "Sabnzbd", "SABnzbd (fixture)", {
    "host": "sabnzbd", "port": 8090, "useSsl": False, "urlBase": "/sabnzbd", "apiKey": "fixture-sab-api-key-0003",
    "tvCategory": "tv",
}, enable=True, priority=2, removeCompletedDownloads=False, removeFailedDownloads=True, tags=[tags.get("4k")])
from_schema("downloadclient", "Transmission", "Transmission (fixture)", {
    "host": "transmission", "port": 9091, "urlBase": "/transmission/", "username": "tx",
    "password": "fixture-tx-password", "tvCategory": "sonarr",
}, enable=False, priority=3, removeCompletedDownloads=True, removeFailedDownloads=True, tags=[])

# Notifications
from_schema("notification", "Webhook", "Webhook (fixture)", {
    "url": "http://hooks.local:18999/kitsunarr", "method": 1, "username": "", "password": "",
}, onGrab=True, onDownload=True, onUpgrade=True, onHealthIssue=True, tags=[])
from_schema("notification", "Discord", "Discord (fixture)", {
    "webHookUrl": "http://hooks.local:18999/api/webhooks/000/fixture-token", "username": "Kitsunarr Fixture",
}, onGrab=False, onDownload=True, onUpgrade=True, onSeriesAdd=True, tags=[])

# Delay + release profiles, remote path mapping, naming, media management
call("POST", "/api/v3/delayprofile", {
    "enableUsenet": True, "enableTorrent": True, "preferredProtocol": "torrent", "usenetDelay": 60,
    "torrentDelay": 120, "bypassIfHighestQuality": True, "bypassIfAboveCustomFormatScore": False,
    "minimumCustomFormatScore": 0, "order": 1, "tags": [tags.get("4k")]}, ok_errors=True)
call("POST", "/api/v3/releaseprofile", {
    "name": "No CAM", "enabled": True, "required": [], "ignored": ["CAM", "TS"], "indexerId": 0, "tags": []},
    ok_errors=True)
import os
os.makedirs(f"{SP}/live/media/downloads", exist_ok=True)
call("POST", "/api/v3/remotepathmapping", {
    "host": "qbittorrent", "remotePath": "/downloads/", "localPath": "/media/downloads/"}, ok_errors=True)
call("POST", "/api/v3/importlistexclusion", {"tvdbId": 73255, "title": "House (excluded fixture)"}, ok_errors=True)
naming = call("GET", "/api/v3/config/naming")
naming.update({
    "renameEpisodes": True, "replaceIllegalCharacters": True, "colonReplacementFormat": 4,
    "multiEpisodeStyle": 5,
    "standardEpisodeFormat": "{Series Title} - S{season:00}E{episode:00} - {Episode Title} [{Quality Full}]",
    "animeEpisodeFormat": "{Series Title} - S{season:00}E{episode:00} - {absolute:000} - {Episode Title} [{Quality Full}]",
})
call("PUT", f"/api/v3/config/naming/{naming['id']}", naming, ok_errors=True)
mm = call("GET", "/api/v3/config/mediamanagement")
mm.update({"recycleBin": "/media/recycle", "copyUsingHardlinks": True, "deleteEmptyFolders": True})
call("PUT", f"/api/v3/config/mediamanagement/{mm['id']}", mm, ok_errors=True)

# Series
existing = {s["tvdbId"] for s in call("GET", "/api/v3/series")}
wanted = [
    ("tvdb:81189", "/media/tv", "standard", profile_id, [tags.get("4k")], "all"),
    ("tvdb:360893", "/media/tv", "standard", profile_id, [tags.get("requested")], "all"),
    ("tvdb:76885", "/media/anime", "anime", anime_profile_id, [tags.get("anime")], "future"),
]
for term, root, stype, qp, stags, monitor in wanted:
    found = call("GET", f"/api/v3/series/lookup?term={term}")
    if not found:
        print("  ! lookup failed", term)
        continue
    s = found[0]
    if s.get("tvdbId") in existing:
        continue
    s.update({"qualityProfileId": qp, "rootFolderPath": root, "monitored": True, "seasonFolder": True,
              "seriesType": stype, "tags": [t for t in stags if t],
              "addOptions": {"monitor": monitor, "searchForMissingEpisodes": False,
                             "searchForCutoffUnmetEpisodes": False}})
    r = call("POST", "/api/v3/series", s, ok_errors=True)
    if r:
        print("  + series", r["id"], r["title"], r["path"])
print("done")
