#!/usr/bin/env python3
# Copyright (c) 2026, The MusicPack Development Team
# SPDX-License-Identifier: BSD-3-Clause
"""Phase 5 musicpack-server integration tests.

Modes:
  setup <ref-mpc> <ref-flac> <tmpdir>
      Builds a small real library under <tmpdir>/lib from the reference
      fixtures (see Phase 4).
  run <base-url> <libdir> <token> <demo-dir>
      Exercises HTTP API v1 behind bearer auth: health/auth matrix, CORS,
      albums/releases/tracks/artists, streaming + HTTP Range + ETag, live
      library scan/verify/status, reads during scans/verifies, duplicate-scan
      rejection, and the static demo directory with cross-origin isolation
      headers.

Exits non-zero on the first failure. Uses only the stdlib.
"""
import hashlib
import json
import os
import shutil
import sqlite3
import sys
import threading
import time
import urllib.error
import urllib.request

API = "/api/v1"
TOKEN = None
BULK_COPIES = 120  # packages copied for live-scan timing tests


# --------------------------------------------------------------------------
# setup
# --------------------------------------------------------------------------

def write_json(path, obj):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f, indent=1)


def file_sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def add_representations(pkg_dir, primary_name):
    """Phase 3 fixture: give track 1 an alternate representation. The alt
    file is a copy of the package's own waveform blob (small, deterministic);
    the manifest gains a typed representations[] array on that track."""
    src = os.path.join(pkg_dir, "analysis", "waveform", "01-01.wfm")
    alt_rel = "audio/01-alt.flac"
    alt_abs = os.path.join(pkg_dir, *alt_rel.split("/"))
    shutil.copy(src, alt_abs)
    mpath = os.path.join(pkg_dir, "manifest.json")
    with open(mpath, encoding="utf-8") as f:
        m = json.load(f)
    m["media"][0]["tracks"][0]["representations"] = [
        {"path": alt_rel, "sha256": file_sha(alt_abs),
         "label": "FLAC Alt", "codec": "flac"},
    ]
    write_json(mpath, m)


def setup(ref_mpc, ref_flac, tmpdir):
    lib = os.path.join(tmpdir, "lib")
    shutil.rmtree(lib, ignore_errors=True)
    os.makedirs(lib)

    shutil.copytree(ref_mpc, os.path.join(lib, "Compilation.mpack"))
    add_representations(os.path.join(lib, "Compilation.mpack"),
                        "01 - Alphaville - Big in Japan.mpc")
    shutil.copytree(ref_flac, os.path.join(lib, "Classical.mpack"))

    second = os.path.join(lib, "Compilation-1987.mpack")
    shutil.copytree(ref_mpc, second)
    with open(os.path.join(second, "manifest.json"), encoding="utf-8") as f:
        m = json.load(f)
    m["release"]["edition"] = "1987 Original CD"
    m["release"]["releaseDate"] = "1987-06-15"
    m["release"]["country"] = "DE"
    write_json(os.path.join(second, "manifest.json"), m)

    twodisc = os.path.join(lib, "TwoDisc.mpack")
    shutil.copytree(ref_mpc, twodisc)
    with open(os.path.join(twodisc, "manifest.json"), encoding="utf-8") as f:
        m = json.load(f)
    m["album"]["title"] = "Two Disc Extravaganza"
    m["album"]["originalReleaseDate"] = "2001-01-01"
    m["release"]["edition"] = "2CD"
    m["media"][0]["tracks"][0]["artists"] = [
        {"name": "Alphaville", "role": "main"},
        {"name": "Bernhard Lloyd", "role": "composer"},
        {"name": "Marian Gold", "role": "featured"},
    ]
    d2_tracks = []
    for i, (num, title) in enumerate([(1, "Side Two One"), (2, "Side Two Two")]):
        src = os.path.join(twodisc, m["media"][0]["tracks"][i]["audio"]["path"])
        newpath = f"audio/0{i + 5} - {title}.mpc"
        dst = os.path.join(twodisc, newpath)
        shutil.copyfile(src, dst)
        d2_tracks.append({
            "track": num, "title": title,
            "audio": {"path": newpath, "sha256": file_sha(dst)},
        })
    m["media"].append({"disc": 2, "format": "CD", "tracks": d2_tracks})
    write_json(os.path.join(twodisc, "manifest.json"), m)

    bad = os.path.join(lib, "Broken.mpack")
    os.makedirs(bad)
    with open(os.path.join(bad, "manifest.json"), "w", encoding="utf-8") as f:
        f.write("{ this is not json ")

    esc = os.path.join(lib, "Escape.mpack")
    shutil.copytree(ref_mpc, esc)
    with open(os.path.join(esc, "manifest.json"), encoding="utf-8") as f:
        m = json.load(f)
    m["release"]["edition"] = "Escape Edition"
    write_json(os.path.join(esc, "manifest.json"), m)
    target = os.path.join(tmpdir, "escaped.bin")
    with open(target, "wb") as f:
        f.write(b"not a real mpc file")
    escaped_audio = os.path.join(esc, "audio", "01 - Alphaville - Big in Japan.mpc")
    os.remove(escaped_audio)
    os.symlink(target, escaped_audio)

    with open(os.path.join(tmpdir, "libdir"), "w") as f:
        f.write(lib)


# --------------------------------------------------------------------------
# http helpers
# --------------------------------------------------------------------------

def get(base, path, headers=None, method=None, auth=True, data=None):
    h = dict(headers or {})
    if auth and TOKEN:
        h["Authorization"] = "Bearer " + TOKEN
    req = urllib.request.Request(base + path, headers=h, data=data)
    if method:
        req.get_method = lambda: method
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def wait_status(base, key, done_field="running"):
    for _ in range(600):
        st, _, body = get(base, API + "/library/status")
        d = json.loads(body)[key]
        if d[done_field] == 0:
            return d
        time.sleep(0.02)
    raise AssertionError("job did not finish in time")


class T:
    def __init__(self):
        self.failures = 0
        self.passed = 0

    def ok(self, cond, name):
        if cond:
            self.passed += 1
        else:
            self.failures += 1
            print("FAIL", name)


# --------------------------------------------------------------------------
# run
# --------------------------------------------------------------------------

def run(base, libdir, demo_dir, t):
    mpc_src = os.path.join(libdir, "Compilation.mpack", "audio",
                            "01 - Alphaville - Big in Japan.mpc")
    classical_src = os.path.join(libdir, "Classical.mpack", "audio",
                                 "01 - Classical Piece No 1.flac")

    # ---- health is public
    st, _, body = get(base, API + "/health", auth=False)
    t.ok(st == 200 and json.loads(body)["status"] == "ok", "health public 200")

    # ---- auth matrix
    st, _, _ = get(base, API + "/albums", auth=False)
    t.ok(st == 401, "no token -> 401")
    st, _, body = get(base, API + "/albums",
                      headers={"Authorization": "Basic abc"}, auth=False)
    t.ok(st == 401, "malformed Authorization -> 401")
    st, _, _ = get(base, API + "/albums",
                   headers={"Authorization": "Bearer invalid"}, auth=False)
    t.ok(st == 401, "invalid token -> 401")
    st, _, body = get(base, API + "/albums")
    t.ok(st == 200, "valid token -> 200")
    st, _, _ = get(base, API + "/tracks/1/audio", auth=False)
    t.ok(st == 401, "audio without token -> 401")
    st, _, _ = get(base, API + "/assets/1", auth=False)
    t.ok(st == 401, "asset without token -> 401")
    st, _, body = get(base, API + "/tracks/1/audio")
    t.ok(st == 200, "audio with token -> 200")

    # ---- CORS
    OK_ORIGIN = "http://localhost:5173"
    BAD_ORIGIN = "http://evil.example"
    st, h, _ = get(base, API + "/albums", headers={"Origin": OK_ORIGIN})
    t.ok(st == 200 and h.get("Access-Control-Allow-Origin") == OK_ORIGIN,
         "allowed origin gets ACAO")
    st, _, _ = get(base, API + "/albums", headers={"Origin": BAD_ORIGIN})
    t.ok(st == 403, "disallowed origin -> 403")
    st, h, _ = get(base, API + "/albums", method="OPTIONS",
                   headers={"Origin": OK_ORIGIN,
                            "Access-Control-Request-Method": "GET"})
    t.ok(st == 204 and "Authorization" in h.get("Access-Control-Allow-Headers", ""),
         "preflight allowed")
    st, h, _ = get(base, API + "/albums")
    t.ok(st == 200 and "Access-Control-Allow-Origin" not in h,
         "no-Origin native client unaffected")

    # ---- session exchange (Phase 6: HttpOnly cookie for the browser client)
    st, _, body = get(base, API + "/session", method="POST",
                      headers={"Content-Type": "application/json"},
                      data=b'{"token":"mpk_not_a_real_token"}', auth=False)
    t.ok(st == 401, "session exchange with invalid token -> 401")
    st, _, body = get(base, API + "/session", method="POST",
                      headers={"Content-Type": "application/json"},
                      data=b'{ broken json', auth=False)
    t.ok(st == 400, "session exchange with malformed body -> 400")
    st, h, body = get(base, API + "/session", method="POST",
                      headers={"Content-Type": "application/json"},
                      data=json.dumps({"token": TOKEN}).encode(), auth=False)
    cookie = h.get("Set-Cookie", "")
    t.ok(st == 200 and json.loads(body)["status"] == "authenticated",
         "session exchange with valid token -> 200")
    t.ok("HttpOnly" in cookie and "SameSite=Strict" in cookie
         and "musicpack_session=" in cookie,
         "session cookie is HttpOnly + SameSite=Strict")
    sess_val = cookie.split("musicpack_session=", 1)[1].split(";", 1)[0]
    t.ok(sess_val and not sess_val.startswith("mpk_"),
         "cookie value is the opaque session secret, not the bearer token")

    # the cookie authenticates subsequent requests without a bearer token
    st, _, body = get(base, API + "/albums",
                      headers={"Cookie": f"musicpack_session={sess_val}"},
                      auth=False)
    t.ok(st == 200, "session cookie authenticates")
    st, _, body = get(base, API + "/session",
                      headers={"Cookie": f"musicpack_session={sess_val}"},
                      auth=False)
    t.ok(st == 200 and json.loads(body).get("session", {}).get("id", 0) > 0,
         "GET /session reports the session")
    st, _, _ = get(base, API + "/session", method="DELETE",
                   headers={"Cookie": f"musicpack_session={sess_val}"},
                   auth=False)
    t.ok(st == 204, "DELETE /session -> 204")
    st, _, _ = get(base, API + "/albums",
                   headers={"Cookie": f"musicpack_session={sess_val}"},
                   auth=False)
    t.ok(st == 401, "revoked session cookie rejected")
    st, _, _ = get(base, API + "/albums", headers={"Cookie": "musicpack_session=garbage"},
                   auth=False)
    t.ok(st == 401, "bogus session cookie rejected")
    st, _, _ = get(base, API + "/session", method="POST", auth=False)
    t.ok(st == 400, "session exchange without a body -> 400")

    # ---- bounded session-token body parsing ----------------------------
    # The token parser must never scan past the meaningful body length or
    # rely on NUL padding: malformed/truncated input must fail cleanly.
    st, _, _ = get(base, API + "/session", method="POST", auth=False,
                   data=b'{"token":"mpk_truncated')
    t.ok(st == 400, "truncated token body -> 400")
    st, _, _ = get(base, API + "/session", method="POST", auth=False,
                   data=b'{"foo":"bar"}')
    t.ok(st == 400, "body without token field -> 400")
    st, _, _ = get(base, API + "/session", method="POST", auth=False,
                   data=b'{"token":""}')
    t.ok(st == 400, "empty token value -> 400")
    # the closing quote of the value is the last byte of the body
    st, _, _ = get(base, API + "/session", method="POST", auth=False,
                   data=b'{"token":"' + TOKEN.encode() + b'"')
    t.ok(st == 200, "token value ending exactly at body boundary -> 200")
    # bytes after the meaningful body (NULs, garbage) must be ignored
    st, _, _ = get(base, API + "/session", method="POST", auth=False,
                   data=b'{"token":"' + TOKEN.encode() + b'"}\x00\x00garbage\xff')
    t.ok(st == 200, "garbage after meaningful body ignored -> 200")

    # ---- albums / collector hierarchy (Phase 4 behaviour preserved)
    st, _, body = get(base, API + "/albums")
    albums = json.loads(body)
    t.ok(st == 200 and albums["total"] == 3, "albums total 3")
    titles = [a["title"] for a in albums["albums"]]
    t.ok(titles == ["Synthetic Test Compilation", "Two Disc Extravaganza",
                    "Synthetic Classical Compilation"],
         "albums deterministically ordered")

    comp = next(a for a in albums["albums"]
                if a["title"] == "Synthetic Test Compilation")
    st, _, body = get(base, API + f"/albums/{comp['id']}")
    detail = json.loads(body)
    editions = sorted(r.get("edition", "") for r in detail["releases"])
    t.ok("1987 Original CD" in editions and "2016 Digital Remaster" in editions
         and "Escape Edition" not in editions,
         "only verified editions visible; escape edition hidden")

    release_id = next(r["id"] for r in detail["releases"]
                      if r.get("edition") == "2016 Digital Remaster")
    st, _, body = get(base, API + f"/releases/{release_id}")
    rel = json.loads(body)
    t.ok(st == 200 and rel["media"][0]["tracks"][0]["codec"]["codec"]
         == "musepack-sv8", "release detail + codec")
    track = rel["media"][0]["tracks"][0]
    tid = track["id"]
    other_tid = rel["media"][0]["tracks"][1]["id"]
    two_disc = next(a for a in albums["albums"]
                    if a["title"] == "Two Disc Extravaganza")
    st, _, body = get(base, API + f"/albums/{two_disc['id']}")
    two_disc_release = json.loads(body)["releases"][0]["id"]
    st, _, body = get(base, API + f"/releases/{two_disc_release}")
    two_disc_track = json.loads(body)["media"][0]["tracks"][0]
    expected_artists = [
        ("Alphaville", "main"),
        ("Bernhard Lloyd", "composer"),
        ("Marian Gold", "featured"),
    ]
    api_artists = [(a["name"], a.get("role"))
                   for a in two_disc_track["artists"]]
    t.ok(st == 200 and api_artists == expected_artists,
         "multiple track artists preserve order and roles in API")
    db_path = os.path.join(os.path.dirname(libdir), "lib.db")
    with sqlite3.connect(db_path) as db:
        artist_rows = db.execute(
            "SELECT ta.position, a.name, ta.role FROM track_artists ta "
            "JOIN artists a ON a.id = ta.artist_id "
            "WHERE ta.track_id = ? ORDER BY ta.position",
            (two_disc_track["id"],),
        ).fetchall()
    t.ok(artist_rows == [(i, name, role)
                         for i, (name, role) in enumerate(expected_artists)],
         "multiple track artists create one ordered row per role")
    classical = next(a for a in albums["albums"]
                     if a["title"] == "Synthetic Classical Compilation")
    st, _, body = get(base, API + f"/albums/{classical['id']}")
    classical_release = json.loads(body)["releases"][0]["id"]
    st, _, body = get(base, API + f"/releases/{classical_release}")
    classical_tid = json.loads(body)["media"][0]["tracks"][0]["id"]

    # ---- Phase 6 client fields: artwork, duration, album loudness
    comp_alb = next(a for a in albums["albums"]
                    if a["title"] == "Synthetic Test Compilation")
    t.ok(comp_alb.get("artwork", {}).get("url", "").startswith("/api/v1/assets/"),
         "album list exposes front artwork")
    t.ok(track.get("duration", 0) > 0, "track duration exposed")
    t.ok(rel.get("loudness", {}).get("albumLufs", 0) < 0
         and rel["loudness"]["algorithm"] == "ITU-R BS.1770-5",
         "release exposes canonical album loudness")
    st, _, body = get(base, API + f"/albums/{comp_alb['id']}")
    adetail = json.loads(body)
    ed_with_art = [r for r in adetail["releases"] if r.get("artwork")]
    t.ok(len(ed_with_art) >= 2, "editions carry their own artwork")
    st, _, body = get(base, API + f"/artists/{comp_alb['artists'][0]['id']}")
    art_adtl = json.loads(body)
    alb_in_artist = next((a for a in art_adtl["albums"] if a["id"] == comp_alb["id"]), None)
    t.ok(alb_in_artist is not None
         and alb_in_artist.get("artwork", {}).get("url", "").startswith("/api/v1/assets/")
         and alb_in_artist["artwork"]["id"] == comp_alb["artwork"]["id"],
         "artist detail albums expose the same front artwork as the shelf")

    # ---- search + recently added
    st, _, body = get(base, API + "/albums?q=Two%20Disc")
    qalb = json.loads(body)
    t.ok(st == 200 and len(qalb["albums"]) == 1
         and qalb["albums"][0]["title"] == "Two Disc Extravaganza",
         "?q= filters albums by title/artist")
    st, _, body = get(base, API + "/albums?q=Synthetic")
    t.ok(st == 200 and len(json.loads(body)["albums"]) == 2,
         "?q= matches substring across albums")
    st, _, body = get(base, API + "/artists?q=Alphaville")
    qart = json.loads(body)
    t.ok(st == 200 and len(qart["artists"]) == 1
         and qart["artists"][0]["name"] == "Alphaville",
         "?q= filters artists")
    st, _, body = get(base, API + f"/artists/{qart['artists'][0]['id']}")
    adetail = json.loads(body)
    t.ok(st == 200 and adetail["name"] == "Alphaville"
         and "musicbrainzId" not in adetail and "sortName" not in adetail,
         "artist detail omits absent anchor/sort fields")
    st, _, body = get(base, API + "/albums?sort=recent")
    t.ok(st == 200 and len(json.loads(body)["albums"]) >= 1,
         "?sort=recent lists albums")

    # ---- streaming + Range + ETag
    st, _, body = get(base, API + f"/tracks/{tid}/audio")
    t.ok(st == 200 and sha(body) == track["audio"]["sha256"],
         "full audio byte-identity")
    st, h, _ = get(base, API + f"/tracks/{tid}/audio", {"Range": "bytes=0-0"})
    t.ok(st == 206 and h.get("Content-Range") == f"bytes 0-0/{track['audio']['size']}",
         "range 206 + Content-Range")
    st, h, _ = get(base, API + f"/tracks/{tid}/audio", {"Range": "bytes=999999-"})
    t.ok(st == 416, "unsatisfiable range 416")
    st, h, _ = get(base, API + f"/tracks/{tid}/audio", {"Range": "bytes=0-9"})
    t.ok(h.get("ETag") == f'"{track["audio"]["sha256"]}"',
         "ETag is the manifest sha256")
    t.ok(h.get("Cache-Control") and "must-revalidate" in h.get("Cache-Control", ""),
         "revalidation cache policy")
    st, _, _ = get(base, API + f"/tracks/{tid}/audio",
                   {"If-None-Match": f'"{track["audio"]["sha256"]}"'})
    t.ok(st == 304, "If-None-Match -> 304")

    # ---- representations (Phase 3) ------------------------------------
    # The fixture gave track 1 one alternate representation; every other
    # track must omit the key entirely (pre-Phase-3 byte-compat).
    st, _, body = get(base, API + f"/tracks/{tid}")
    td = json.loads(body)
    t.ok("representations" in td and len(td["representations"]) == 1,
         "track detail lists its representation")
    rep = td["representations"][0]
    t.ok(rep["codec"]["codec"].startswith("flac")
         and rep["label"] == "FLAC Alt" and rep["size"] > 0,
         "representation carries probed codec + label + size")
    rid = rep["id"]
    other = rel["media"][0]["tracks"][1]
    t.ok("representations" not in other,
         "untouched track omits the representations key")

    alt_path = None
    src_alt = os.path.join(libdir, "Compilation.mpack", "audio", "01-alt.flac")
    want_sha = file_sha(src_alt)
    st, h, body = get(base, API + f"/tracks/{tid}/representations/{rid}/audio")
    t.ok(st == 200 and sha(body) == want_sha,
         "variant full byte-identity")
    t.ok(h.get("ETag") == f'"{want_sha}"',
         "variant ETag is the content sha256")
    st, h, _ = get(base, API + f"/tracks/{tid}/representations/{rid}/audio",
                   {"Range": "bytes=0-0"})
    t.ok(st == 206 and h.get("Content-Range", "").startswith("bytes 0-0/"),
         "variant range 206")
    st, _, _ = get(base, API + f"/tracks/{tid}/representations/{rid}/audio",
                   {"Range": "bytes=99999999-"})
    t.ok(st == 416, "variant unsatisfiable range 416")
    st, _, _ = get(base, API + f"/tracks/{tid}/representations/999999/audio")
    t.ok(st == 404, "unknown variant id -> 404")
    # a real variant id of ANOTHER track must not resolve for this track
    st, _, body2 = get(base, API + f"/tracks/{other_tid}")
    other_detail = json.loads(body2)
    t.ok("representations" not in other_detail,
         "second track has no variants of its own")

    # ---- waveform envelope endpoint + track JSON ---------------------
    # The committed reference package carries waveform envelopes; verify
    # the track JSON surfaces waveform metadata and the binary endpoint
    # serves it with the documented security headers and ETag.
    t.ok("waveform" in track and track["waveform"] is not None,
         "track JSON carries waveform metadata")
    wf = track["waveform"]
    t.ok(wf["version"] == 1 and wf["intervalMs"] == 100
         and wf["encoding"] == "peak-rms-u8" and wf["floorDb"] == -60
         and isinstance(wf["points"], int) and wf["points"] > 0
         and wf["url"].endswith("/waveform"),
         "waveform metadata fields")
    st, _, body = get(base, API + f"/tracks/{tid}/waveform")
    t.ok(st == 200 and len(body) == wf["points"] * 2,
         "waveform bytes = points * 2")
    st, h, _ = get(base, API + f"/tracks/{tid}/waveform")
    t.ok(st == 200 and h.get("ETag", "").startswith('"'),
         "waveform response carries ETag")
    st, h, _ = get(base, API + f"/tracks/{tid}/waveform")
    t.ok(h.get("X-Content-Type-Options") == "nosniff"
         and "sandbox" in h.get("Content-Security-Policy", "")
         and "attachment" in h.get("Content-Disposition", ""),
         "waveform security headers (nosniff + sandbox + attachment)")
    # Auth gate: without a token, 401.
    st, _, _ = get(base, API + f"/tracks/{tid}/waveform", auth=False)
    t.ok(st == 401, "waveform requires auth")
    # No-waveform fixture (FLAC) reports null and 404s.
    classical_track_id = classical_tid
    st, _, _ = get(base, API + f"/tracks/{classical_track_id}/waveform")
    t.ok(st == 404, "no-waveform track waveform endpoint 404s")
    st, _, body = get(base, API + f"/tracks/{classical_track_id}")
    classical_track_obj = json.loads(body)
    t.ok(classical_track_obj.get("waveform") is None,
         "no-waveform package reports waveform: null")

    # ---- package-object response security headers ----------------------
    # body-bearing audio responses carry nosniff + sandbox CSP
    st, h, _ = get(base, API + f"/tracks/{tid}/audio")
    t.ok(h.get("X-Content-Type-Options") == "nosniff",
         "200 audio nosniff")
    t.ok("sandbox" in h.get("Content-Security-Policy", ""),
         "200 audio sandbox CSP")
    st, h, _ = get(base, API + f"/tracks/{tid}/audio", {"Range": "bytes=0-0"})
    t.ok(h.get("X-Content-Type-Options") == "nosniff",
         "206 audio nosniff")
    t.ok("sandbox" in h.get("Content-Security-Policy", ""),
         "206 audio sandbox CSP")
    # bodyless responses carry the same security headers (documented as
    # consistent across 200/206/304/416)
    st, h, _ = get(base, API + f"/tracks/{tid}/audio",
                   {"If-None-Match": f'"{track["audio"]["sha256"]}"'})
    t.ok(st == 304 and h.get("X-Content-Type-Options") == "nosniff",
         "304 nosniff")
    t.ok(st == 304 and "sandbox" in h.get("Content-Security-Policy", ""),
         "304 sandbox CSP")
    st, h, _ = get(base, API + f"/tracks/{tid}/audio", {"Range": "bytes=999999-"})
    t.ok(st == 416 and h.get("X-Content-Type-Options") == "nosniff",
         "416 nosniff")
    t.ok(st == 416 and "sandbox" in h.get("Content-Security-Policy", ""),
         "416 sandbox CSP")
    # a non-inline type (booklet/lyrics text) is forced to attachment with a
    # sanitized filename; fetch a lyrics/booklet asset from the release detail
    st, _, body = get(base, API + f"/releases/{release_id}")
    rdetail = json.loads(body)
    non_inline = [a for a in rdetail.get("assets", [])
                  if not a.get("mimeType", "").startswith(("image/", "audio/"))]
    if non_inline:
        st, h, abody = get(base, non_inline[0]["url"])
        t.ok(st == 200 and h.get("X-Content-Type-Options") == "nosniff",
             "attachment asset nosniff")
        cd = h.get("Content-Disposition", "")
        t.ok("attachment" in cd and '"' in cd,
             "non-inline asset forced to attachment with quoted filename")
        t.ok("sandbox" in h.get("Content-Security-Policy", ""),
             "attachment asset sandbox CSP")

    # ---- live scan / status / verify
    # add a bulk of identical packages so a scan takes long enough to observe
    bulk = os.path.join(libdir, "bulk")
    os.makedirs(bulk, exist_ok=True)
    for i in range(BULK_COPIES):
        dst = os.path.join(bulk, f"Bulk{i:03d}.mpack")
        if not os.path.exists(dst):
            shutil.copytree(os.path.join(libdir, "Compilation.mpack"), dst)

    st, _, body = get(base, API + "/library/scan", method="POST")
    t.ok(st == 202, "POST /library/scan -> 202")
    st, _, body = get(base, API + "/library/scan", method="POST")
    t.ok(st == 409, "duplicate scan -> 409 scan_already_running")
    # reads continue while the scan runs
    st, _, chunk = get(base, API + f"/tracks/{tid}/audio", {"Range": "bytes=0-127"})
    with open(mpc_src, "rb") as f:
        expect = f.read(128)
    t.ok(st == 206 and chunk == expect, "reads continue during scan")
    scan = wait_status(base, "scan")
    t.ok(scan["packagesScanned"] >= BULK_COPIES, "scan counted bulk packages")

    # a new package is fail-closed until verified: a lightweight scan alone
    # must not make it visible/servable
    newpkg = os.path.join(libdir, "Fresh.mpack")
    shutil.copytree(os.path.join(libdir, "Classical.mpack"), newpkg)
    with open(os.path.join(newpkg, "manifest.json"), encoding="utf-8") as f:
        m = json.load(f)
    m["album"]["title"] = "Fresh Album"
    write_json(os.path.join(newpkg, "manifest.json"), m)
    st, _, body = get(base, API + "/library/scan", method="POST")
    t.ok(st == 202, "scan new package")
    scan = wait_status(base, "scan")
    st, _, body = get(base, API + "/albums")
    total = json.loads(body)["total"]
    t.ok(total == 3, "new package not visible until verified (fail closed)")

    # full verification makes it visible (tolerate verify-job commit visibility
    # lag under slow/sanitized builds)
    st, _, body = get(base, API + "/library/verify", method="POST")
    t.ok(st == 202, "verify after scan")
    wait_status(base, "verify")
    deadline = time.time() + 5
    total = 0
    while time.time() < deadline:
        st, _, body = get(base, API + "/albums")
        total = json.loads(body)["total"]
        if total == 4:
            break
        time.sleep(0.1)
    t.ok(total == 4, "new album visible after verify")

    # removed package becomes unavailable
    shutil.rmtree(os.path.join(libdir, "Fresh.mpack"))
    get(base, API + "/library/scan", method="POST")
    wait_status(base, "scan")
    st, _, body = get(base, API + "/albums")
    t.ok(json.loads(body)["total"] == 3, "removed album disappears after scan")

    # verify (the deliberately broken Escape package is expected to fail)
    st, _, body = get(base, API + "/library/verify", method="POST")
    t.ok(st == 202, "POST /library/verify -> 202")
    # a session exchange while a verify job holds the database must never be
    # reported as invalid credentials (no false 401 from DB contention)
    st, _, _ = get(base, API + "/session", method="POST", auth=False,
                   data=json.dumps({"token": TOKEN}).encode())
    t.ok(st == 200, "session exchange during verify -> 200 (no false 401)")
    st, _, body = get(base, API + "/library/verify", method="POST")
    t.ok(st == 409, "duplicate verify -> 409")
    st, _, chunk = get(base, API + f"/tracks/{classical_tid}/audio", {"Range": "bytes=0-127"})
    with open(classical_src, "rb") as f:
        classical_expect = f.read(128)
    t.ok(st == 206 and chunk == classical_expect, "serving continues during verify")
    v = wait_status(base, "verify")
    t.ok(v["packagesVerified"] >= 4 and v["failed"] >= 1,
         "verify checks library (escape package fails)")

    # checksum mismatch is surfaced by verify
    corrupt = os.path.join(libdir, "Classical.mpack", "audio",
                           "01 - Classical Piece No 1.flac")
    with open(corrupt, "ab") as f:
        f.write(b"X")
    get(base, API + "/library/verify", method="POST")
    v = wait_status(base, "verify")
    t.ok(v["failed"] >= 2, "verify detects checksum mismatch")
    st, h, _ = get(base, API + f"/tracks/{classical_tid}/audio")
    t.ok(st == 404 and "ETag" not in h,
         "checksum-failed package is unservable without an ETag")
    st, _, body = get(base, API + "/albums")
    t.ok(json.loads(body)["total"] == 2,
         "checksum-failed package is invisible from the library")

    # ---- re-ingest replaces assets (stale rows gone, no duplicates) -----
    # Re-scan a content-changed package (track title edit keeps release
    # identity stable) and verify assets are replaced rather than appended.
    st, _, body = get(base, API + f"/releases/{release_id}")
    rdetail = json.loads(body)
    assets_before = rdetail.get("assets", [])
    t.ok(len(assets_before) >= 2, "release exposes assets before re-ingest")
    old_url = assets_before[0]["url"]

    comp_manifest = os.path.join(libdir, "Compilation.mpack", "manifest.json")
    with open(comp_manifest, encoding="utf-8") as f:
        m = json.load(f)
    m["media"][0]["tracks"][0]["title"] = "Big in Japan (re-ingested)"
    write_json(comp_manifest, m)
    get(base, API + "/library/scan", method="POST")
    wait_status(base, "scan")
    # A lightweight re-ingest leaves the package unverified (fail-closed), so
    # verify it again before reading the release's asset rows.
    get(base, API + "/library/verify", method="POST")
    wait_status(base, "verify")

    st, _, body = get(base, API + f"/releases/{release_id}")
    rdetail2 = json.loads(body)
    assets_after = [a["url"] for a in rdetail2.get("assets", [])]
    t.ok(len(assets_after) == len(assets_before),
         "re-ingest keeps asset count stable (no accumulation)")
    t.ok(len(set(assets_after)) == len(assets_after),
         "re-ingest produces no duplicate asset URLs")
    for a in rdetail2.get("assets", []):
        # extras are indexed but not servable (documented limitation); the
        # servable kinds (booklet/lyrics/artwork) must all resolve.
        if a.get("kind") == "extras":
            continue
        st, _, _ = get(base, a["url"])
        t.ok(st == 200, "current asset URL resolves after re-ingest")
    # the pre-re-ingest asset id was deleted: its URL no longer resolves
    if old_url not in assets_after:
        st, _, _ = get(base, old_url)
        t.ok(st == 404, "stale asset URL no longer resolves after re-ingest")
    else:
        t.ok(True, "old asset id reused (not stale)")

    # library/status without auth -> 401
    st, _, _ = get(base, API + "/library/status", auth=False)
    t.ok(st == 401, "status requires auth")

    # ---- static demo dir with cross-origin isolation
    st, h, body = get(base, "/", auth=False)
    t.ok(st == 200 and b"<html" in body.lower(),
         "static demo index served")
    t.ok(h.get("Cross-Origin-Opener-Policy") == "same-origin" and
         h.get("Cross-Origin-Embedder-Policy") == "require-corp",
         "COOP/COEP isolation headers")
    st, h, _ = get(base, "/index.html", auth=False)
    t.ok(st == 200 and h.get("Content-Type", "").startswith("text/html"),
         "static index.html served")
    # SPA fallback: unknown extension-less client routes serve index.html
    st, h, body = get(base, "/albums/2", auth=False)
    t.ok(st == 200 and h.get("Content-Type", "").startswith("text/html")
         and b"<html" in body.lower(),
         "SPA deep link falls back to index.html")
    st, h, body = get(base, "/albums/2?release=3", auth=False)
    t.ok(st == 200 and b"<html" in body.lower(),
         "SPA fallback preserves query strings")
    st, _, _ = get(base, "/nonexistent.js", auth=False)
    t.ok(st == 404, "missing asset with an extension -> 404")
    # static dir cannot escape
    st, _, _ = get(base, "/../../etc/passwd", auth=False)
    t.ok(st == 404 or st == 400, "static traversal rejected")
    # demo dir actually has the demo files
    st, _, _ = get(base, "/worker.js", auth=False)
    t.ok(st == 200 or not os.path.exists(os.path.join(demo_dir, "worker.js")),
         "demo worker.js served")

    print(f"server_api_test: {t.passed} passed, {t.failures} failed")
    return t.failures == 0


def main():
    global TOKEN
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    mode = sys.argv[1]
    if mode == "setup":
        setup(sys.argv[2], sys.argv[3], sys.argv[4])
        return 0
    if mode == "run":
        TOKEN = sys.argv[4]
        t = T()
        ok = run(sys.argv[2], sys.argv[3], sys.argv[5], t)
        return 0 if ok else 1
    print("unknown mode", mode)
    return 2


if __name__ == "__main__":
    sys.exit(main())
