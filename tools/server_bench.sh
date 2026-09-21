#!/usr/bin/env bash
# Copyright (c) 2026, The MusicPack Development Team
# SPDX-License-Identifier: BSD-3-Clause
#
# R4.5 production performance sanity check against the *release* server.
#
# Not a benchmark suite: it measures the handful of operations that would
# reveal an obvious production regression (startup, scan/verify, API, byte
# range serving, concurrent range reads, graceful shutdown) on a committed
# fixture library. Run it manually:
#
#   cargo build --release -p musicpack-server
#   bash tools/server_bench.sh
#
# MUSICPACK_SERVER overrides the binary (default target/release/…).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${MUSICPACK_SERVER:-$ROOT/target/release/musicpack-server}"
FIXTURE="$ROOT/web/tests/fixtures/test-musicpack-album.mpack"

if [ ! -x "$BIN" ]; then
  echo "missing release binary: $BIN (cargo build --release -p musicpack-server)" >&2
  exit 1
fi

now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f\n", time*1000'; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/mp-bench.XXXXXX")"
cleanup() {
  [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK/lib"
cp -R "$FIXTURE" "$WORK/lib/Fixture.mpack"
PORT="${MUSICPACK_BENCH_PORT:-18099}"
BASE="http://127.0.0.1:$PORT"

t0="$(now_ms)"
"$BIN" scan --library "$WORK/lib" --database "$WORK/db" --verify >/dev/null
t1="$(now_ms)"
echo "scan+verify (1 package, 4 tracks): $((t1 - t0)) ms"

TOKEN="$("$BIN" token create --name bench --database "$WORK/db" | grep '^mpk_')"

t0="$(now_ms)"
"$BIN" serve --library "$WORK/lib" --database "$WORK/db" --no-scan --port "$PORT" \
  --shutdown-file "$WORK/stop" >/dev/null 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 500); do
  if curl -fsS "$BASE/api/v1/health" >/dev/null 2>&1; then break; fi
  sleep 0.01
done
t1="$(now_ms)"
echo "startup -> health (release, --no-scan): $((t1 - t0)) ms"

timed() { # label curl-args...
  local label="$1"; shift
  local out
  out="$(curl -sS -o /dev/null -w '%{time_total}' "$@")"
  echo "$label: $(perl -e "printf '%.2f', $out*1000") ms"
}

timed "GET /api/v1/health" "$BASE/api/v1/health"
timed "GET /api/v1/albums?limit=50 (auth)" \
  -H "Authorization: Bearer $TOKEN" "$BASE/api/v1/albums?limit=50"
timed "GET /api/v1/tracks/1 (auth)" \
  -H "Authorization: Bearer $TOKEN" "$BASE/api/v1/tracks/1"
timed "GET asset range 0-65535" \
  -H "Authorization: Bearer $TOKEN" -H "Range: bytes=0-65535" "$BASE/api/v1/tracks/1/audio"

# 8 concurrent range reads over the same asset.
t0="$(now_ms)"
pids=()
for _ in $(seq 1 8); do
  curl -sS -o /dev/null -H "Authorization: Bearer $TOKEN" \
    -H "Range: bytes=0-65535" "$BASE/api/v1/tracks/1/audio" &
  pids+=("$!")
done
for pid in "${pids[@]}"; do wait "$pid" || true; done
t1="$(now_ms)"
echo "8 concurrent range reads: $((t1 - t0)) ms total"

t0="$(now_ms)"
: > "$WORK/stop"
for _ in $(seq 1 500); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then break; fi
  sleep 0.01
done
t1="$(now_ms)"
if kill -0 "$SERVER_PID" 2>/dev/null; then
  echo "shutdown: did not exit"
else
  echo "graceful shutdown (shutdown-file drain): $((t1 - t0)) ms"
fi
SERVER_PID=""
