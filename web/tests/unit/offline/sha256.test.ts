// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

import { describe, expect, it } from 'vitest';
import { Sha256, isSha256Hex, sha256Hex } from '../../../app/src/lib/offline/sha256';

const ABC = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';
const ABCD = '88d4266fd4e6338d13b845fcf289579d209c897823b9217da3e161936f031589';
const EMPTY = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855';

describe('offline sha256', () => {
  it('matches the FIPS 180-4 vectors', () => {
    const enc = new TextEncoder();
    expect(sha256Hex(enc.encode(''))).toBe(EMPTY);
    expect(sha256Hex(enc.encode('abc'))).toBe(ABC);
    expect(sha256Hex(enc.encode('abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq'))).toBe(
      '248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1',
    );
    const millionA = new Uint8Array(1_000_000).fill(0x61); // 'a'
    expect(sha256Hex(millionA)).toBe(
      'cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0',
    );
  });

  it('is chunking-invariant across every boundary size', () => {
    const data = new Uint8Array(257);
    for (let i = 0; i < data.length; i++) data[i] = (i * 37) & 0xff;
    const whole = sha256Hex(data);
    for (let split = 0; split <= data.length; split += 17) {
      const h = new Sha256().update(data.subarray(0, split)).update(data.subarray(split));
      expect(h.hex(), `split at ${split}`).toBe(whole);
    }
  });

  it('cross-checks against crypto.subtle (when available)', async () => {
    if (typeof crypto === 'undefined' || !crypto.subtle) return;
    const data = new TextEncoder().encode('cross-check musicpack offline');
    const subtleBuf = await crypto.subtle.digest('SHA-256', data);
    const subtle = [...new Uint8Array(subtleBuf)]
      .map((b) => b.toString(16).padStart(2, '0'))
      .join('');
    expect(sha256Hex(data)).toBe(subtle);
  });

  it('validates hash shape', () => {
    expect(isSha256Hex('a'.repeat(64))).toBe(true);
    expect(isSha256Hex('A'.repeat(64))).toBe(false);
    expect(isSha256Hex('a'.repeat(63))).toBe(false);
    expect(isSha256Hex(undefined)).toBe(false);
  });
});
