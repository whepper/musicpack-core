#!/usr/bin/env python3
# Copyright (c) 2026, The MusicPack Development Team
# SPDX-License-Identifier: LGPL-2.1-or-later
"""Extract a compact, reviewable SV8 container fixture from a reference `.mpc`.

This is a provenance/maintenance tool, not part of the test runtime. It parses
an SV8 stream with the Python standard library only (it never invokes the C
encoder) and prints a textual fixture capturing each block's key, encoded size
field, CRC (where present) and payload bytes, plus the seek-table parameters.

The committed fixtures are frozen: do not regenerate them to accommodate a Rust
result. This script exists so a future engineer can audit or deliberately add a
case and see exactly where the bytes came from.

Usage:
    python3 make_container_fixtures.py <reference.mpc> [source-label]
"""

import sys


def read_varint(buf, at):
    """SV8 base-128 size: 7 value bits per byte, high bit set except the last."""
    size = 0
    count = 0
    while True:
        byte = buf[at]
        at += 1
        count += 1
        size = size * 128 + (byte & 0x7F)
        if not (byte & 0x80):
            return size, count


class BitReader:
    def __init__(self, data):
        self.data = data
        self.bit = 0

    def read(self, nbits):
        value = 0
        for _ in range(nbits):
            byte = self.data[self.bit >> 3]
            value = (value << 1) | ((byte >> (7 - (self.bit & 7))) & 1)
            self.bit += 1
        return value

    def get_size(self):
        size = 0
        while True:
            byte = self.read(8)
            size = size * 128 + (byte & 0x7F)
            if not (byte & 0x80):
                return size


def parse_blocks(data):
    assert data[:4] == b"MPCK", "not an SV8 stream"
    blocks = []
    at = 4
    while at + 2 <= len(data):
        offset = at
        key = data[at:at + 2].decode("ascii")
        at += 2
        declared, size_bytes = read_varint(data, at)
        at += size_bytes
        header_len = 2 + size_bytes
        payload_len = declared - header_len
        region = data[at:at + payload_len]
        at += payload_len
        if key == "SH":
            crc = int.from_bytes(region[:4], "big")
            payload = bytes(region[4:])
        else:
            crc = None
            payload = bytes(region)
        blocks.append({
            "key": key,
            "offset": offset,
            "declared": declared,
            "size_bytes": size_bytes,
            "payload_len": len(payload),
            "crc": crc,
            "payload": payload,
        })
        if key == "SE":
            break
    return blocks


def decode_stream_info(payload):
    r = BitReader(payload)
    version = r.read(8)
    assert version == 8, f"unexpected stream version {version}"
    samples = r.get_size()
    skip = r.get_size()
    rates = [44100, 48000, 37800, 32000]
    rate = rates[r.read(3)]
    max_band = r.read(5) + 1
    channels = r.read(4) + 1
    ms = r.read(1)
    block_pwr = r.read(3) * 2
    return {
        "samples": samples, "skip": skip, "sample_rate": rate,
        "max_band": max_band, "channels": channels, "ms": ms,
        "block_pwr": block_pwr,
    }


def decode_encoder_info(payload):
    r = BitReader(payload)
    profile = r.read(7)
    pns = r.read(1)
    major = r.read(8)
    minor = r.read(8)
    build = r.read(8)
    return {"profile": profile // 8, "pns": pns,
            "major": major, "minor": minor, "build": build}


def decode_seek_table(payload):
    seek_pos, n = read_varint(payload, 0)
    r = BitReader(payload)
    r.bit = n * 8
    seek_pwr = r.read(4)
    return seek_pos, seek_pwr


def main():
    path = sys.argv[1]
    source = sys.argv[2] if len(sys.argv) > 2 else path
    data = open(path, "rb").read()
    blocks = parse_blocks(data)

    ap_offsets = [b["offset"] for b in blocks if b["key"] == "AP"]
    sh = next(b for b in blocks if b["key"] == "SH")
    st = next((b for b in blocks if b["key"] == "ST"), None)
    so = next((b for b in blocks if b["key"] == "SO"), None)

    print("# musicpack-sv8-container-fixture v1")
    print(f"# source: {source}")
    print("# frozen artifact - do not regenerate to accommodate a Rust result")
    print("seek_ref 0")
    if so is not None:
        print(f"seek_ptr {so['offset']}")
    if st is not None:
        seek_pos, seek_pwr = decode_seek_table(st["payload"])
        entries = [off for idx, off in enumerate(ap_offsets)
                   if idx % (1 << seek_pwr) == 0]
        print(f"seek_table_offset {st['offset']}")
        print(f"seek_pos {seek_pos}")
        print(f"seek_pwr {seek_pwr}")
        print("seek_entries " + " ".join(str(e) for e in entries))
    info = decode_stream_info(sh["payload"])
    print("stream_info " + " ".join(f"{k}={v}" for k, v in info.items()))
    ei = next((b for b in blocks if b["key"] == "EI"), None)
    if ei is not None:
        enc = decode_encoder_info(ei["payload"])
        print("encoder_info " + " ".join(f"{k}={v}" for k, v in enc.items()))
    for b in blocks:
        crc = "-" if b["crc"] is None else f"{b['crc']:08x}"
        size = data[b["offset"] + 2:b["offset"] + 2 + b["size_bytes"]].hex()
        if b["key"] == "AP":
            import hashlib
            digest = hashlib.sha256(b["payload"]).hexdigest()
            print(f"block {b['key']} offset={b['offset']} size={size} crc={crc} "
                  f"payload_len={b['payload_len']} payload=- payload_sha256={digest}")
        else:
            print(f"block {b['key']} offset={b['offset']} size={size} crc={crc} "
                  f"payload_len={b['payload_len']} payload={b['payload'].hex()}")


if __name__ == "__main__":
    main()
