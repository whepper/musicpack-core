#!/usr/bin/env bash
# Copyright (c) 2026, The MusicPack Development Team
# SPDX-License-Identifier: BSD-3-Clause
# Prepends rustup's cargo bin to PATH when the calling shell predates the
# Rust install (long-lived IDE terminals inherit a stale environment), then
# execs the arguments. Usage: with-cargo.sh <command> [args...]
if ! command -v cargo >/dev/null 2>&1 && [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
exec "$@"
