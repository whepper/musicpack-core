// xfade_oracle.ts — generates the committed Phase 11 crossfade-mixer oracle
// used by crates/musicpack-engine/tests/xfade_oracle.rs.
//
// It imports the REAL reference `MusicPackPcmProcessor`
// (web/app/src/lib/playback/audio-worklet.ts), feeds deterministic scripted
// PCM through its crossfade lane, drives `process()`, and emits compact JSONL
// with SHA-256 digests of the inputs and the mixed output. The Rust mixer must
// reproduce the output digests.
//
// Run through the reference checkout's esbuild (extensionless imports):
//
//   cd ../musicpack/web
//   ./node_modules/.bin/esbuild \
//       /abs/path/to/musicpack-core/tools/xfade_oracle.ts \
//       --bundle --platform=node --format=esm --outfile=/tmp/xfade_oracle.mjs
//   node /tmp/xfade_oracle.mjs > /abs/path/to/musicpack-core/tests/data/xfade_oracle.jsonl

import { createHash } from 'node:crypto';

class FakePort {
  onmessage: ((event: { data: unknown }) => void) | null = null;
  messages: unknown[] = [];
  postMessage(message: unknown): void {
    this.messages.push(message);
  }
}

class FakeAudioWorkletProcessor {
  readonly port = new FakePort();
}

let ProcessorCtor: (new () => { port: FakePort; process(i: Float32Array[][], o: Float32Array[][]): boolean }) | null = null;

(globalThis as Record<string, unknown>).AudioWorkletProcessor = FakeAudioWorkletProcessor;
(globalThis as Record<string, unknown>).currentTime = 0;
(globalThis as Record<string, unknown>).registerProcessor = (_name: string, ctor: unknown) => {
  ProcessorCtor = ctor as typeof ProcessorCtor;
};

// The worklet module registers itself on import; it extends
// `AudioWorkletProcessor` at module-eval time, so the stub must be installed
// first (a static import would be hoisted above the assignments).
await import('../../musicpack/web/app/src/lib/playback/audio-worklet.ts');

const RATE = 44100;
const CALLBACK = 64;

function lcg(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s;
  };
}

/** Deterministic interleaved PCM: integer-derived so Rust matches exactly. */
function gen(kind: string, frames: number, channels: number): Float32Array {
  const out = new Float32Array(frames * channels);
  const next = lcg(0x12345678);
  for (let i = 0; i < frames; i++) {
    for (let c = 0; c < channels; c++) {
      let v: number;
      switch (kind) {
        case 'silence':
          v = 0;
          break;
        case 'dc':
          v = c === 0 ? 0.5 : -0.5;
          break;
        case 'ramp':
          v = ((i * 2 - frames) / frames) + c * 0.1;
          break;
        case 'impulse':
          v = i === 0 ? 1 : 0;
          break;
        default: {
          const r = next();
          v = ((r >>> 8) % 512) / 256 - 1;
        }
      }
      out[i * channels + c] = v;
    }
  }
  return out;
}

function digest(bytes: Float32Array): string {
  const buf = Buffer.alloc(bytes.length * 4);
  for (let i = 0; i < bytes.length; i++) buf.writeFloatLE(bytes[i]!, i * 4);
  return createHash('sha256').update(buf).digest('hex');
}

function interleave(planar: Float32Array[], frames: number, channels: number): Float32Array {
  const out = new Float32Array(frames * channels);
  for (let i = 0; i < frames; i++) {
    for (let c = 0; c < channels; c++) out[i * channels + c] = planar[c]![i] ?? 0;
  }
  return out;
}

interface Case {
  id: string;
  kind: string;
  channels: number;
  fadeFrames: number;
  outFrames: number;
  inFrames: number;
}

const cases: Case[] = [
  { id: 'silence', kind: 'silence', channels: 1, fadeFrames: 257, outFrames: 257, inFrames: 257 },
  { id: 'dc-mono', kind: 'dc', channels: 1, fadeFrames: 257, outFrames: 257, inFrames: 257 },
  { id: 'ramp-mono', kind: 'ramp', channels: 1, fadeFrames: 257, outFrames: 257, inFrames: 257 },
  { id: 'prng-mono', kind: 'prng', channels: 1, fadeFrames: 257, outFrames: 257, inFrames: 257 },
  { id: 'ramp-stereo', kind: 'ramp', channels: 2, fadeFrames: 257, outFrames: 257, inFrames: 257 },
  { id: 'prng-stereo', kind: 'prng', channels: 2, fadeFrames: 257, outFrames: 257, inFrames: 257 },
  { id: 'short-outgoing', kind: 'ramp', channels: 1, fadeFrames: 257, outFrames: 64, inFrames: 257 },
  { id: 'short-incoming', kind: 'ramp', channels: 1, fadeFrames: 257, outFrames: 257, inFrames: 64 },
  { id: 'both-short', kind: 'ramp', channels: 1, fadeFrames: 257, outFrames: 32, inFrames: 32 },
  { id: 'impulse', kind: 'impulse', channels: 2, fadeFrames: 257, outFrames: 257, inFrames: 257 },
];

function runCase(c: Case): unknown {
  if (!ProcessorCtor) throw new Error('worklet processor not registered');
  const proc = new ProcessorCtor();
  const port = proc.port;
  const generation = 1;
  port.onmessage?.({
    data: {
      type: 'config',
      sourceRate: RATE,
      sourceChannels: c.channels,
      outputRate: RATE,
      outputChannels: c.channels,
      generation,
    },
  });

  const outgoing = gen(c.kind, c.outFrames, c.channels);
  const incoming = gen(c.kind, c.inFrames, c.channels);

  port.onmessage?.({
    data: { type: 'samples', buffer: outgoing.buffer.slice(0), generation },
  });
  const token = 1;
  port.onmessage?.({
    data: {
      type: 'xfade',
      sourceRate: RATE,
      sourceChannels: c.channels,
      fadeFrames: c.fadeFrames,
      token,
      generation,
    },
  });
  port.onmessage?.({
    data: { type: 'xsamples', buffer: incoming.buffer.slice(0), token, generation },
  });
  port.onmessage?.({ data: { type: 'xend', token, generation } });
  port.onmessage?.({ data: { type: 'xfade-go', token, generation } });

  const collected: Float32Array[] = [];
  let got = 0;
  for (let iter = 0; iter < 64 && got < c.fadeFrames; iter++) {
    const out: Float32Array[][] = [
      Array.from({ length: c.channels }, () => new Float32Array(CALLBACK)),
    ];
    proc.process([], out);
    collected.push(...out[0]!.map((ch) => ch));
    got += CALLBACK;
  }
  // Assemble exactly the fade window, interleaved.
  const planar: Float32Array[] = [];
  for (let ch = 0; ch < c.channels; ch++) {
    planar.push(new Float32Array(c.fadeFrames));
  }
  for (let i = 0; i < collected.length; i++) {
    const src = collected[i]!;
    const ch = i % c.channels;
    const base = Math.floor(i / c.channels) * CALLBACK;
    for (let k = 0; k < CALLBACK && base + k < c.fadeFrames; k++) {
      planar[ch]![base + k] = src[k]!;
    }
  }
  const mixed = interleave(planar, c.fadeFrames, c.channels);
  return {
    kind: 'xfade',
    id: c.id,
    channels: c.channels,
    fadeFrames: c.fadeFrames,
    outFrames: c.outFrames,
    inFrames: c.inFrames,
    inputKind: c.kind,
    inputOutSha: digest(outgoing),
    inputInSha: digest(incoming),
    outputSha: digest(mixed),
    head: Array.from(mixed.slice(0, 8)),
    tail: Array.from(mixed.slice(-8)),
  };
}

const lines: string[] = [];
for (const c of cases) lines.push(JSON.stringify(runCase(c)));
process.stdout.write(lines.join('\n') + '\n');
