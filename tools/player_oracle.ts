// player_oracle.ts — generates the committed Phase 10 player-core oracle
// fixtures used by tests/player_oracle.rs.
//
// The reference behavioural oracle is the TypeScript package
// `web/player-core` in the sibling MusicPack repository. This tool imports it
// directly and emits deterministic JSONL covering:
//
//   - the Sweet-Fade transition planner
//   - the BS.1770 gain policy
//   - the session snapshot codec
//   - deterministic queue operations
//   - scripted player scenarios driven through a fake engine
//
// Run through the reference checkout's bundled esbuild (the package uses
// extensionless relative imports, which Node ESM cannot resolve directly):
//
//   cd ../musicpack/web
//   ./node_modules/.bin/esbuild \
//       /abs/path/to/musicpack-core/tools/player_oracle.ts \
//       --bundle --platform=node --format=esm --outfile=/tmp/player_oracle.mjs
//   node /tmp/player_oracle.mjs > /abs/path/to/musicpack-core/tests/data/player_oracle.jsonl
//
// The fixtures are committed, so CI never runs this tool. Node only strips
// types at build time via esbuild; it does not type-check.

import { planTransition } from '../../musicpack/web/player-core/src/transition.ts';
import type { BoundaryProfile, TransitionQuery } from '../../musicpack/web/player-core/src/transition.ts';
import { normalizationGainDb, dbToLinear, combinedGain } from '../../musicpack/web/player-core/src/gain.ts';
import { decodeSnapshot } from '../../musicpack/web/player-core/src/snapshot.ts';
import { createQueueModel } from '../../musicpack/web/player-core/src/queue.ts';
import { Player } from '../../musicpack/web/player-core/src/player.ts';
import type { Engine, EngineCapabilities, PreloadEngine, CrossfadeEngine, CrossfadeResult } from '../../musicpack/web/player-core/src/engine.ts';
import type { PlaybackItem, StreamInfo } from '../../musicpack/web/player-core/src/types.ts';

const RATE = 44100;
const out: string[] = [];
function emit(obj: unknown): void {
  out.push(JSON.stringify(obj));
}

function item(n: number, secs?: number): PlaybackItem {
  return {
    id: `t${n}`,
    trackId: n,
    source: { kind: 'http-range', url: `/api/v1/tracks/${n}/audio`, byteSize: 100 },
    ...(secs !== undefined ? { durationHintSeconds: secs } : {}),
    title: `T${n}`,
    artist: 'A',
    albumTitle: 'AL',
  };
}

// ---- transition planner ---------------------------------------------------

function tq(maxFadeSeconds: number, repeatOne: boolean, sameRelease?: boolean): TransitionQuery {
  return { outgoing: item(1), incoming: item(2), maxFadeSeconds, repeatOne, sameRelease };
}
function profile(tail: number[], head: number[], lengthSeconds = 200): BoundaryProfile {
  return { lengthSeconds, tail, head };
}

const BPS = 0.1;
const transitionCases: Array<[string, TransitionQuery, BoundaryProfile | null, BoundaryProfile | null]> = [
  ['off', tq(0, false), profile([0.8, 0.8, 0.8], []), profile([0.8, 0.8, 0.8], [])],
  ['repeat-one', tq(8, true), profile([0.8, 0.8, 0.8], []), profile([0.8, 0.8, 0.8], [])],
  ['missing-profiles', tq(8, false), null, null],
  ['partial-outgoing', tq(4, false), profile([], []), null],
  [
    'trailing-silence-gapless',
    tq(8, false, true),
    profile([...Array(20).fill(0.7), ...Array(50).fill(0)], []),
    profile([], []),
  ],
  [
    'same-release-clean-loud',
    tq(8, false, true),
    profile(Array(100).fill(0.8), []),
    profile([], Array(50).fill(0.7)),
  ],
  [
    'hard-cut',
    tq(8, false, false),
    profile(Array(100).fill(0.9), []),
    profile([], [0.85, 0.9, 0.9]),
  ],
  [
    'hug-outro',
    tq(12, false),
    profile([...Array(50).fill(0.9), ...Array.from({ length: 50 }, (_, i) => 0.5 * (1 - i / 50))], []),
    profile([], [0.1, 0.2, 0.6]),
  ],
  ['cap', tq(3, false), profile([...Array(80).fill(0.9), ...Array(20).fill(0.05)], []), profile([], [0.1])],
];
for (const [id, query, o, i] of transitionCases) {
  emit({
    kind: 'transition',
    id,
    secondsPerBucket: BPS,
    query: { maxFadeSeconds: query.maxFadeSeconds, repeatOne: query.repeatOne, sameRelease: query.sameRelease ?? null },
    outgoing: o,
    incoming: i,
    plan: planTransition(query, o, i, BPS),
  });
}

// ---- gain -----------------------------------------------------------------

const tTrack = { lufs: -7.19, truePeakDb: -4.19 };
const tAlbum = { albumLufs: -7.28, albumTruePeakDb: -4.19 };
const gainCases: Array<[string, string, typeof tTrack | undefined, typeof tAlbum | undefined]> = [
  ['off', 'off', tTrack, tAlbum],
  ['album', 'album', tTrack, tAlbum],
  ['track', 'track', tTrack, tAlbum],
  ['album-missing', 'album', tTrack, undefined],
  ['track-missing-track', 'track', undefined, tAlbum],
  ['peak-cap', 'album', tTrack, { albumLufs: -30, albumTruePeakDb: -3 }],
];
for (const [id, mode, track, album] of gainCases) {
  emit({
    kind: 'gain',
    id,
    mode,
    track: track ?? null,
    album: album ?? null,
    gain: normalizationGainDb(mode as never, track as never, album as never),
  });
}
for (const db of [0, 6.0206, -6.0206, -16, 2.5]) {
  emit({ kind: 'db-to-linear', db, linear: dbToLinear(db) });
}
emit({ kind: 'combined-gain', volume: 0.5, normDb: -8.81, gain: combinedGain(0.5, -8.81) });

// ---- snapshot -------------------------------------------------------------

const snapshotPayloads: Array<[string, string]> = [
  ['v1', JSON.stringify({
    v: 1,
    items: [
      { id: 't1', source: { kind: 'http-range', url: '/api/v1/tracks/1/audio', byteSize: 1000 }, track: { id: 1, audio: { url: '/api/v1/tracks/1/audio' } } },
      { id: 't2', source: { kind: 'http-range', url: '/api/v1/tracks/2/audio', byteSize: 1000 }, track: { id: 2, audio: { url: '/api/v1/tracks/2/audio' } } },
    ],
    index: 1, positionSeconds: 5.5, volume: 0.8, normalizeMode: 'album',
  })],
  ['v2', JSON.stringify({
    v: 2,
    items: [{ id: 't7', source: { kind: 'http-range', url: '/x', byteSize: 1 }, track: { id: 7, audio: { url: '/x' }, duration: 20 } }],
    index: 0, positionSeconds: 3, volume: 0.9, normalizeMode: 'track', repeat: 'all', shuffle: true,
  })],
  ['corrupt', 'not json {'],
  ['wrong-version', JSON.stringify({ v: 9, items: [] })],
  ['pre-modern', JSON.stringify({ v: 1, items: [{ track: { id: 1, audio: { url: '/x' } } }], index: 0, positionSeconds: 0 })],
  ['clamped-index', JSON.stringify({
    v: 1,
    items: [{ id: 't7', source: { kind: 'http-range', url: '/x', byteSize: 1 }, track: { id: 7, audio: { url: '/x' }, duration: 15 } }],
    index: 99, positionSeconds: -4, volume: 5,
  })],
];
for (const [id, raw] of snapshotPayloads) {
  const s = decodeSnapshot(raw);
  emit({
    kind: 'snapshot',
    id,
    raw,
    decoded: s === null ? null : {
      v: s.v,
      index: s.index,
      positionSeconds: s.positionSeconds,
      volume: s.volume ?? null,
      normalizeMode: s.normalizeMode ?? null,
      repeat: s.repeat,
      shuffle: s.shuffle,
      crossfadeSeconds: s.crossfadeSeconds ?? 0,
      itemIds: s.items.map((it: { id?: string }) => it.id ?? null),
    },
  });
}

// ---- queue operations -----------------------------------------------------

function queueScript(ops: Array<[string, ...number[]]>, repeat = 'off', shuffle = false) {
  const q = createQueueModel({ rng: () => 0.5 });
  q.playSequence([item(1), item(2), item(3), item(4)], 0);
  if (shuffle) q.setShuffle(true);
  q.setRepeat(repeat as never);
  for (const [op, a, b] of ops) {
    if (op === 'next') q.next();
    else if (op === 'previous') q.previous();
    else if (op === 'moveTo') q.moveTo(a!);
    else if (op === 'move') q.move(a!, b!);
    else if (op === 'removeAt') q.removeAt(a!);
    else if (op === 'playNext') q.playNext(item(a!));
    else if (op === 'clear') q.clear();
  }
  return {
    items: q.get().items.map((i: PlaybackItem) => i.id),
    index: q.get().index,
    repeat: q.repeat,
    shuffle: q.shuffle,
  };
}
const queueCases: Array<[string, Array<[string, ...number[]]>, string?, boolean?]> = [
  ['next-previous', [['next'], ['next'], ['previous']]],
  ['remove-before-cursor', [['next'], ['removeAt', 0]]],
  ['remove-current', [['next'], ['removeAt', 1]]],
  ['move-follows-item', [['moveTo', 2], ['move', 2, 0]]],
  ['play-next-insert', [['playNext', 9]]],
  ['repeat-all-wrap', [['next'], ['next'], ['next']], 'all'],
  ['clear', [['clear']]],
];
for (const [id, ops, repeat, shuffle] of queueCases) {
  emit({
    kind: 'queue',
    id,
    ops,
    repeat: repeat ?? 'off',
    shuffle: shuffle === true,
    final: queueScript(ops, repeat, shuffle),
  });
}

// ---- player scenarios -----------------------------------------------------

interface ScenarioEngineConfig {
  crossfade: boolean;
  fade: 'immediate' | 'deferred' | 'null';
  defaultSeconds: number;
  hints: Record<number, number>;
  overlapFrames: number;
  plan: 'none' | 'gapless' | 'hard-cut' | { sweet: number };
  standbyMismatch?: boolean;
}

interface Scenario {
  id: string;
  engine: ScenarioEngineConfig;
  actions: string[];
}

const scenarios: Scenario[] = [
  { id: 'basic-load-play', engine: cfg(), actions: ['play_seq:1,2;0', 'prime'] },
  { id: 'gapless-advance', engine: cfg(), actions: ['play_seq:1,2;0', 'prime', 'set_rendered:441000', 'eos'] },
  { id: 'repeat-one', engine: cfg(), actions: ['play_seq:1,2;0', 'prime', 'set_repeat:one', 'set_rendered:441000', 'eos'] },
  { id: 'repeat-all', engine: cfg(), actions: ['set_repeat:all', 'play_seq:1,2;0', 'prime', 'set_rendered:441000', 'eos', 'set_rendered:882000', 'eos'] },
  { id: 'shuffle-eos', engine: cfg(), actions: ['play_seq:1,2,3;0', 'prime', 'set_shuffle:true', 'set_order:0,2,1', 'set_rendered:441000', 'eos'] },
  { id: 'crossfade-take', engine: cfg({ crossfade: true, fade: 'immediate', defaultSeconds: 30 }), actions: ['play_seq:1,2;0', 'set_crossfade:8', 'prime', 'set_rendered:1102500', 'tick'] },
  { id: 'crossfade-decline', engine: cfg({ crossfade: true, fade: 'null', defaultSeconds: 30 }), actions: ['play_seq:1,2;0', 'set_crossfade:4', 'prime', 'set_rendered:1278900', 'tick'] },
  { id: 'eos-race', engine: cfg({ crossfade: true, fade: 'immediate', defaultSeconds: 30 }), actions: ['play_seq:1,2;0', 'set_crossfade:8', 'prime', 'eos'] },
  { id: 'pause-during-fade', engine: cfg({ crossfade: true, fade: 'deferred', defaultSeconds: 30 }), actions: ['play_seq:1,2;0', 'set_crossfade:4', 'prime', 'set_rendered:1190700', 'tick', 'pause', 'tick', 'resolve_fade'] },
  { id: 'stale-eos', engine: cfg({ crossfade: true, fade: 'deferred', defaultSeconds: 30 }), actions: ['play_seq:1,2,3;0', 'set_crossfade:4', 'prime', 'set_rendered:1190700', 'tick', 'eos', 'resolve_fade'] },
  { id: 'cross-track-seek', engine: cfg(), actions: ['play_seq:1,2;0', 'prime', 'seek:15'] },
  { id: 'queue-mutation', engine: cfg(), actions: ['play_seq:1,2,3;0', 'prime', 'queue_remove:1', 'queue_changed', 'prime'] },
  { id: 'boundary-drift', engine: cfg({ crossfade: true, fade: 'immediate', defaultSeconds: 30, overlapFrames: 352800 }), actions: ['play_seq:1,2;0', 'set_crossfade:8', 'prime', 'set_rendered:1102500', 'tick', 'set_rendered:882000', 'tick'] },
  { id: 'teardown', engine: cfg(), actions: ['play_seq:1,2;0', 'prime', 'teardown'] },
  { id: 'standby-mismatch', engine: cfg({ crossfade: true, fade: 'immediate', defaultSeconds: 30, standbyMismatch: true }), actions: ['play_seq:1,2;0', 'set_crossfade:8', 'prime', 'set_rendered:1102500', 'tick', 'eos'] },
];

function cfg(over: Partial<ScenarioEngineConfig> = {}): ScenarioEngineConfig {
  return { crossfade: false, fade: 'null', defaultSeconds: 10, hints: {}, overlapFrames: 0, plan: 'none', ...over };
}

interface ScenarioResult {
  state: string;
  index: number;
  currentId: string | null;
  positionSeconds: number;
  durationSeconds: number;
  opened: string[];
  prepared: string[];
  beginCrossfade: Array<[string, number]>;
  crossfadeCalls: number;
  seekCalls: number[];
  drift: boolean;
}

function round6(n: number): number { return Math.round(n * 1e6) / 1e6; }

async function runScenarioWithHandlers(s: Scenario): Promise<ScenarioResult> {
  const e = s.engine;
  const caps: EngineCapabilities = { preloadNext: true, sampleAccurateGapless: true, decodeGate: true, crossfade: e.crossfade };
  const q = createQueueModel({ rng: () => 0.5 });
  let handlers!: { primed(): void; buffering(): void; eos(): void; error(m: string): void; tick(): void };
  const st = {
    rendered: 0,
    opened: [] as string[],
    prepared: [] as string[],
    beginCrossfade: [] as Array<[string, number]>,
    seeks: [] as number[],
    standby: null as PlaybackItem | null,
    standbyInfo: null as StreamInfo | null,
    pendingResolve: null as ((r: CrossfadeResult | null) => void) | null,
  };
  const info = (it: PlaybackItem): StreamInfo => ({
    rate: RATE,
    channels: 2,
    version: 0,
    lengthSamples: Math.floor((e.hints[it.trackId] ?? it.durationHintSeconds ?? e.defaultSeconds) * RATE),
  });
  const engine: Engine & PreloadEngine & CrossfadeEngine = {
    capabilities: caps,
    init: async () => undefined,
    open: async (it: PlaybackItem) => { st.opened.push(it.source.url); st.rendered = 0; return info(it); },
    play: async () => undefined,
    pause: async () => undefined,
    seekSample: async (n: number) => { st.seeks.push(n); st.rendered = 0; },
    setGain: () => undefined,
    renderedSamples: () => st.rendered,
    close: async () => undefined,
    on: () => () => undefined,
    prepareNext: async (it: PlaybackItem) => { st.prepared.push(it.source.url); st.standby = it; st.standbyInfo = info(it); return info(it); },
    advance: async (expected: PlaybackItem | null) => {
      if (!expected || !st.standby || st.standby.id !== expected.id || st.standby.trackId !== expected.trackId) return null;
      const i = st.standbyInfo; st.standby = null; st.standbyInfo = null; return i;
    },
    beginCrossfade: async (next: PlaybackItem, secs: number) => {
      st.beginCrossfade.push([next.id, secs]);
      if (e.standbyMismatch) return null;
      if (e.fade === 'null') return null;
      const r: CrossfadeResult = { info: info(next), overlapFrames: e.overlapFrames };
      if (e.fade === 'deferred') {
        return new Promise<CrossfadeResult | null>((resolve) => { st.pendingResolve = resolve; });
      }
      return r;
    },
  };
  const player = new Player(q, {
    engineFactory: (_k, h) => { handlers = h; return engine; },
    resolveKind: () => 'musepack',
    storage: { get: () => null, set: () => undefined },
    now: () => 0,
    schedulePersist: () => 0,
    planTransition: e.plan === 'none' ? undefined : () => {
      if (e.plan === 'gapless') return { type: 'gapless' } as const;
      if (e.plan === 'hard-cut') return { type: 'hard-cut' } as const;
      return { type: 'sweet-fade', overlapSeconds: e.plan.sweet } as const;
    },
  });
  const drift: boolean[] = [];
  player.on((ev) => { if (ev.t === 'boundary-drift') drift.push(true); });
  player.init();
  const flush = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
  for (const action of s.actions) {
    const [op, arg] = action.split(':');
    switch (op) {
      case 'play_seq': {
        const [list, start] = arg.split(';');
        const items = list.split(',').map((n) => item(Number(n), e.hints[Number(n)]));
        await player.playSequence(items, 'AL', 'A', Number(start));
        await flush();
        handlers.primed();
        await flush();
        break;
      }
      case 'prime': handlers.primed(); await flush(); break;
      case 'tick': handlers.tick(); await flush(); break;
      case 'eos': handlers.eos(); await flush(); break;
      case 'pause': await player.pause(); await flush(); break;
      case 'next': await player.next(); await flush(); handlers.primed(); await flush(); break;
      case 'previous': await player.previous(); await flush(); handlers.primed(); await flush(); break;
      case 'seek': await player.seek(Number(arg)); await flush(); handlers.primed(); await flush(); break;
      case 'teardown': await player.teardown(); await flush(); break;
      case 'set_repeat': player.setRepeat(arg as never); break;
      case 'set_shuffle': player.setShuffle(arg === 'true'); break;
      case 'set_order': q.setPresentationOrderForTest(arg.split(',').map(Number)); break;
      case 'set_crossfade': player.setCrossfade(Number(arg)); break;
      case 'set_rendered': st.rendered = Number(arg); break;
      case 'queue_remove': q.removeAt(Number(arg)); break;
      case 'queue_changed': break;
      case 'resolve_fade': st.pendingResolve?.({ info: info(item(2)), overlapFrames: 0 }); st.pendingResolve = null; await flush(); break;
    }
  }
  return {
    state: player.model.get().state,
    index: q.get().index,
    currentId: player.model.get().current?.id ?? null,
    positionSeconds: round6(player.model.get().positionSeconds),
    durationSeconds: round6(player.model.get().durationSeconds),
    opened: st.opened,
    prepared: st.prepared,
    beginCrossfade: st.beginCrossfade,
    crossfadeCalls: st.beginCrossfade.length,
    seekCalls: st.seeks,
    drift: drift.length > 0,
  };
}

async function main(): Promise<void> {
  for (const s of scenarios) {
    const final = await runScenarioWithHandlers(s);
    emit({ kind: 'scenario', id: s.id, engine: s.engine, actions: s.actions, final });
  }
  process.stdout.write(out.join('\n') + '\n');
}

void main();
