#!/usr/bin/env python3
"""Sanitise captured Sonarr/Tanukarr data before committing it as test fixtures.

* replaces the instance API key with a fixed fake key (config.xml, API captures),
* removes third-party descriptive text (overviews, actors, translations),
* drops endpoints that may contain host secrets,
* vacuums the database so the archive stays small.

Development tool only; Kitsunarr itself never runs Python.

usage: python3 -I sanitize.py <captured-dir> <fixture-dir> <fake-api-key>
"""
import json
import os
import re
import shutil
import sqlite3
import sys
import tempfile
import zipfile

SRC, DST, FAKE_KEY = sys.argv[1], sys.argv[2], sys.argv[3]
SCRUB_KEYS = {"overview", "actors", "translations"}
SKIP_ENDPOINTS = {"config-host"}


def scrub(value):
    if isinstance(value, dict):
        return {k: ("" if k in SCRUB_KEYS and isinstance(v, str) else [] if k in SCRUB_KEYS and isinstance(v, list) else scrub(v))
                for k, v in value.items() if not (k in SCRUB_KEYS and v is None)}
    if isinstance(value, list):
        return [scrub(v) for v in value]
    return value


os.makedirs(os.path.join(DST, "api"), exist_ok=True)
for name in sorted(os.listdir(os.path.join(SRC, "api"))):
    stem = name[:-5]
    if stem in SKIP_ENDPOINTS:
        continue
    with open(os.path.join(SRC, "api", name)) as fh:
        data = scrub(json.load(fh))
    with open(os.path.join(DST, "api", name), "w") as fh:
        json.dump(data, fh, indent=1, sort_keys=True, ensure_ascii=False)
        fh.write("\n")

with tempfile.TemporaryDirectory() as tmp:
    with zipfile.ZipFile(os.path.join(SRC, "backup.zip")) as zf:
        names = zf.namelist()
        for n in names:
            if n in ("sonarr.db", "config.xml", "INFO"):
                zf.extract(n, tmp)
    db = os.path.join(tmp, "sonarr.db")
    con = sqlite3.connect(db)
    cols = lambda t: {r[1] for r in con.execute(f"PRAGMA table_info('{t}')")}
    if "Overview" in cols("Series"):
        con.execute("UPDATE Series SET Overview = ''")
    if "Actors" in cols("Series"):
        con.execute("UPDATE Series SET Actors = '[]'")
    if "Overview" in cols("Episodes"):
        con.execute("UPDATE Episodes SET Overview = NULL")
    con.execute("UPDATE Config SET Value = 'fixture-redacted' WHERE Key IN "
                "('rijndaelpassphrase', 'rijndaelsalt', 'hmacpassphrase', 'hmacsalt', 'plexclientidentifier')")
    con.execute("DELETE FROM Users")
    con.commit()
    con.execute("VACUUM")
    con.close()
    cfg = os.path.join(tmp, "config.xml")
    with open(cfg) as fh:
        xml = fh.read()
    xml = re.sub(r"<ApiKey>.*?</ApiKey>", f"<ApiKey>{FAKE_KEY}</ApiKey>", xml)
    with open(cfg, "w") as fh:
        fh.write(xml)
    out = os.path.join(DST, "backup.zip")
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for n in ("config.xml", "sonarr.db", "INFO"):
            if os.path.exists(os.path.join(tmp, n)):
                zf.write(os.path.join(tmp, n), n)
print("wrote", DST, os.path.getsize(os.path.join(DST, "backup.zip")), "bytes")
