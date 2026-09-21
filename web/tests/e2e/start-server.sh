#!/usr/bin/env bash
# Copyright (c) 2026, The MusicPack Development Team
# SPDX-License-Identifier: BSD-3-Clause
# Starts the Rust musicpack-server with a fixture library for the Playwright
# e2e suite. Builds the library (two reference albums, plus generated Long
# Player / Fade Rider / Compilation / Shapeshifter packages), verifies it
# (verify makes content servable — see the cutover checklist §5), creates a
# token, and serves the built client from web/app/dist.
#
# Server selection: MUSICPACK_SERVER or the repo's debug build (built on
# demand via cargo).
#
# The token is written to tests/e2e/.server-env.json for the tests to read.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
E2E="$(cd "$(dirname "$0")" && pwd)"
STATIC_DIR="${MUSICPACK_STATIC_DIR:-$ROOT/web/app/dist}"
PORT="${MUSICPACK_E2E_PORT:-8099}"
PY="$(command -v python3 || command -v python)"
FIXTURES="$ROOT/web/tests/fixtures"

SERVER_BIN="${MUSICPACK_SERVER:-$ROOT/target/debug/musicpack-server}"
if [ ! -x "$SERVER_BIN" ]; then
  echo "building Rust musicpack-server (cargo)…" >&2
  (cd "$ROOT" && cargo build -p musicpack-server) >&2
  SERVER_BIN="$ROOT/target/debug/musicpack-server"
fi
[ -x "$SERVER_BIN" ] || { echo "musicpack-server not found at $SERVER_BIN" >&2; exit 1; }
[ -f "$STATIC_DIR/index.html" ] || { echo "client build missing at $STATIC_DIR (npm run build)" >&2; exit 1; }

TMP="$(mktemp -d "${TMPDIR:-/tmp}/mpack-e2e.XXXXXX")"
LIB="$TMP/lib"
DB="$TMP/lib.db"

"$PY" "$E2E/fixtures/server_api_test.py" setup \
    "$FIXTURES/test-musicpack-album.mpack" \
    "$FIXTURES/test-flac-album.mpack" \
    "$TMP" || { echo "FAIL fixture setup" >&2; exit 1; }
LIB="$(cat "$TMP/libdir")"
# NOTE: no `verify` here — every package added below is covered by the
# single full verify after the last fixture, so the library is hashed
# exactly once per harness start.
TOKEN="$("$SERVER_BIN" token create --name "E2E" --database "$DB" 2>/dev/null | grep '^mpk_')"

# Build a "Long Player" album whose first track is the 48-second sine fixture,
# so the seek/fetch-accounting tests operate on a long enough stream.
"$PY" - "$FIXTURES/test-musicpack-album.mpack" "$LIB/Long Player.mpack" \
    "$FIXTURES/sine44-q5-48s.mpc" <<'PY'
import hashlib, json, os, shutil, sys
ref, dst, sine = sys.argv[1], sys.argv[2], sys.argv[3]
shutil.copytree(ref, dst)
sha = hashlib.sha256(open(sine, "rb").read()).hexdigest()
shutil.copy(sine, os.path.join(dst, "audio", "01 - Alphaville - Big in Japan.mpc"))
mpath = os.path.join(dst, "manifest.json")
m = json.load(open(mpath, encoding="utf-8"))
m["album"]["title"] = "Long Player"
m["album"]["originalReleaseDate"] = "1986-06-16"
# Four genres so the shelf card exercises the 3-pills + "+1" overflow while
# the album detail page shows every pill (Phase 2B write-through).
m["album"]["genres"] = ["Rock", "Electronic", "Synthwave", "Pop"]
m["release"]["edition"] = "1986 Original CD"
m["media"][0]["tracks"][0]["audio"]["sha256"] = sha
m["media"][0]["tracks"][0]["duration"] = 48
json.dump(m, open(mpath, "w", encoding="utf-8"), indent=1)
PY

# A two-long-track musepack album ("Fade Rider") for the crossfade e2e:
# 48 s tracks pace their decode, so the audible-boundary crossfade trigger
# (remaining <= fade window) reliably beats the decode-eos handoff.
"$PY" - "$FIXTURES/test-musicpack-album.mpack" "$LIB/Fade Rider.mpack" \
    "$FIXTURES/sine44-q5-48s.mpc" <<'PY'
import hashlib, json, os, shutil, sys
ref, dst, sine = sys.argv[1], sys.argv[2], sys.argv[3]
shutil.copytree(ref, dst)
sha = hashlib.sha256(open(sine, "rb").read()).hexdigest()
for name in ("01 - Fade Rider - Horizon.mpc", "02 - Fade Rider - Sunrise.mpc"):
    shutil.copy(sine, os.path.join(dst, "audio", name))
mpath = os.path.join(dst, "manifest.json")
m = json.load(open(mpath, encoding="utf-8"))
m["album"]["title"] = "Fade Rider"
m["album"]["originalReleaseDate"] = "2026-01-01"
m["release"]["edition"] = "2026 Digital"
t0 = m["media"][0]["tracks"][0]
t1 = dict(t0)
# NOTE: dict(t0) shallow-copies — t1["audio"] would alias t0["audio"].
# Assign fresh objects so each track gets its own asset entry.
t0["title"] = "Fade Rider - Horizon"
t0["audio"] = {"path": "audio/01 - Fade Rider - Horizon.mpc", "sha256": sha}
t0["duration"] = 48
t1["title"] = "Fade Rider - Sunrise"
t1["track"] = 2
t1["audio"] = {"path": "audio/02 - Fade Rider - Sunrise.mpc", "sha256": sha}
t1["duration"] = 48
# The cloned waveform reference would collide with track 1's asset path;
# waveform is optional per-track data, so drop it here.
del t1["waveform"]
m["media"][0]["tracks"] = [t0, t1]
json.dump(m, open(mpath, "w", encoding="utf-8"), indent=1)
PY

# A third, valid edition of the Compilation album. The fixture's "Escape
# Edition" package deliberately contains a symlinked audio object and is
# correctly hidden by verified-only visibility, so the e2e edition-grouping
# test needs a real third edition that verifies.
"$PY" - "$FIXTURES/test-musicpack-album.mpack" "$LIB/Compilation-2001.mpack" <<'PY'
import json, os, shutil, sys
ref, dst = sys.argv[1], sys.argv[2]
shutil.copytree(ref, dst)
mpath = os.path.join(dst, "manifest.json")
m = json.load(open(mpath, encoding="utf-8"))
m["release"]["edition"] = "2001 Reissue"
m["release"]["releaseDate"] = "2001-09-14"
m["release"]["country"] = "GB"
json.dump(m, open(mpath, "w", encoding="utf-8"), indent=1)
PY

# A representation-bearing album ("Shapeshifter"): track 1 carries a real
# 8 s FLAC alternate so the Phase 4 selection specs can flip a track
# between Musepack and native playback and still cross the boundary into
# the following Musepack track within test time.
"$PY" - "$FIXTURES/test-musicpack-album.mpack" "$LIB/Shapeshifter.mpack" \
    "$FIXTURES/sine44-8s.flac" <<'PY'
import hashlib, json, os, shutil, sys
ref, dst, flac = sys.argv[1], sys.argv[2], sys.argv[3]
shutil.copytree(ref, dst)
alt_rel = "audio/01 - Alphaville - Big in Japan.flac"
alt_abs = os.path.join(dst, *alt_rel.split("/"))
shutil.copy(flac, alt_abs)
sha = hashlib.sha256(open(alt_abs, "rb").read()).hexdigest()
mpath = os.path.join(dst, "manifest.json")
m = json.load(open(mpath, encoding="utf-8"))
m["album"]["title"] = "Shapeshifter"
m["media"][0]["tracks"][0]["representations"] = [
    {"path": alt_rel, "sha256": sha, "label": "FLAC", "codec": "flac"},
]
json.dump(m, open(mpath, "w", encoding="utf-8"), indent=1)
PY

# A lyrics-bearing album ("Lyric Shelf", R3.4): track 1 carries one synced
# LRC document (en) on the long 48 s fixture so the synchronized-follow
# test has runway; track 2 carries two references (en first, fr second) so
# the deterministic first-entry selection is observable; track 3 has none.
"$PY" - "$FIXTURES/test-musicpack-album.mpack" "$LIB/Lyric Shelf.mpack" \
    "$FIXTURES/sine44-q5-48s.mpc" <<'PY'
import hashlib, json, os, shutil, sys
ref, dst, sine = sys.argv[1], sys.argv[2], sys.argv[3]
shutil.copytree(ref, dst)

def sha_of(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()

# Track 1 plays the long fixture (48 s of runway for seek assertions).
long_sha = sha_of(sine)
shutil.copy(sine, os.path.join(dst, "audio", "01 - Alphaville - Big in Japan.mpc"))

lrc = "\n".join([
    "[ti:Big in Japan]",
    "[ar:Alphaville]",
    "[00:01.00]First line appears here",
    "[00:03.00]Second line for the middle",
    "[00:05.00]Third line for the seek",
    "[00:20.00]Fourth line holds to the end",
]) + "\n"
van_en = "Words without timing one\nWords without timing two\n"
van_fr = "Paroles sans synchronisation\n"
os.makedirs(os.path.join(dst, "lyrics"), exist_ok=True)
files = {
    "lyrics/01-big-in-japan.lrc": lrc,
    "lyrics/02-the-van.en.txt": van_en,
    "lyrics/02-the-van.fr.txt": van_fr,
}
for rel, body in files.items():
    with open(os.path.join(dst, rel), "w", encoding="utf-8") as f:
        f.write(body)

mpath = os.path.join(dst, "manifest.json")
m = json.load(open(mpath, encoding="utf-8"), )
m["album"]["title"] = "Lyric Shelf"
tracks = m["media"][0]["tracks"]
t0 = tracks[0]
t0["audio"] = {"path": "audio/01 - Alphaville - Big in Japan.mpc", "sha256": long_sha}
t0["duration"] = 48
t0["lyrics"] = [{"path": "lyrics/01-big-in-japan.lrc",
                 "sha256": sha_of(os.path.join(dst, "lyrics/01-big-in-japan.lrc")),
                 "lang": "en"}]
t1 = tracks[1]
t1["lyrics"] = [
    {"path": "lyrics/02-the-van.en.txt",
     "sha256": sha_of(os.path.join(dst, "lyrics/02-the-van.en.txt")), "lang": "en"},
    {"path": "lyrics/02-the-van.fr.txt",
     "sha256": sha_of(os.path.join(dst, "lyrics/02-the-van.fr.txt")), "lang": "fr"},
]
# tracks[2] ("Synthwave - Night Drive") deliberately carries no lyrics.
json.dump(m, open(mpath, "w", encoding="utf-8"), indent=1)
PY

# A package produced by the **Rust** authoring pipeline ("Rust Authored"):
# this makes the e2e library carry the full vertical
#   Rust Author -> .mpack -> Rust server -> R3.6 offline planner -> OPFS
#   -> Rust/WASM decoder -> player
# The package is built here (not committed) so the vertical stays live.
AUTHOR_BIN="${MUSICPACK_AUTHOR:-$ROOT/target/debug/musicpack-author}"
if [ ! -x "$AUTHOR_BIN" ]; then
  echo "building Rust musicpack-author (cargo)…" >&2
  (cd "$ROOT" && cargo build -p musicpack-author) >&2
  AUTHOR_BIN="$ROOT/target/debug/musicpack-author"
fi
AUTHOR_SRC="$TMP/rust-author-src"
mkdir -p "$AUTHOR_SRC"
cp "$FIXTURES/sine44-8s.flac" "$AUTHOR_SRC/sine.flac"
cp "$FIXTURES/test-musicpack-album.mpack/artwork/front.jpg" "$AUTHOR_SRC/front.jpg"
"$PY" - "$AUTHOR_SRC" <<'PY'
import json, sys
root = sys.argv[1]
draft = {
    "schema": "musicpack-draft",
    "version": 1,
    "sourceRoot": root,
    "album": {"title": "Rust Authored", "artists": [{"name": "Fixture Artist"}],
              "releaseType": "album"},
    "identifiers": {
        "musicbrainzReleaseGroupId": "33333333-3333-3333-3333-333333333333",
        "musicbrainzReleaseId": "44444444-4444-4444-4444-444444444444",
    },
    "media": [{"disc": 1, "tracks": [
        {"track": 1, "title": "Rust One", "audioPath": "sine.flac"}]}],
    "artwork": [{"role": "front", "path": "front.jpg"}],
    "booklet": [], "lyrics": [], "extras": [],
}
json.dump(draft, open(root + "/draft.json", "w", encoding="utf-8"), indent=1)
PY
"$AUTHOR_BIN" build "$AUTHOR_SRC/draft.json" -o "$LIB/Rust Authored.mpack" --json >/dev/null \
    || { echo "FAIL Rust author build" >&2; exit 1; }

"$SERVER_BIN" verify --library "$LIB" --database "$DB" >/dev/null 2>&1

cat > "$E2E/.server-env.json" <<JSON
{ "token": "$TOKEN", "baseUrl": "http://127.0.0.1:$PORT", "libdir": "$LIB" }
JSON

exec "$SERVER_BIN" serve --library "$LIB" --database "$DB" \
    --listen 127.0.0.1 --port "$PORT" --no-scan \
    --static-dir "$STATIC_DIR"
