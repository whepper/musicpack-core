// policy_oracle.ts — generates the committed Phase 12 representation-policy
// oracle used by tests/policy_oracle.rs.
//
// The reference behavioural oracle is the Phase 4 TypeScript resolver
// `web/app/src/lib/state/representation-selection.ts` in the sibling MusicPack
// repository. This tool imports it directly and emits deterministic JSONL:
// for each scripted (track, preference, playability) record it records the
// normalized preference and the selected representation id (null = primary).
//
// Run through the reference checkout's esbuild (extensionless imports):
//
//   cd ../musicpack/web
//   ./node_modules/.bin/esbuild \
//       /abs/path/to/musicpack-core/tools/policy_oracle.ts \
//       --bundle --platform=node --format=esm --outfile=/tmp/policy_oracle.mjs
//   node /tmp/policy_oracle.mjs > /abs/path/to/musicpack-core/tests/data/policy_oracle.jsonl
//
// The fixtures are committed, so CI never runs this tool.

import {
  parseAudioPreference,
  resolveAudio,
} from '../../musicpack/web/app/src/lib/state/representation-selection.ts';
import type { CanPlay } from '../../musicpack/web/app/src/lib/state/representation-selection.ts';
import type { RepresentationRef, Track } from '../../musicpack/web/app/src/lib/api/types.ts';

interface RepSpec {
  id: number;
  codec?: string;
  mimeType?: string;
}
interface TrackSpec {
  codec?: string;
  mimeType?: string;
  reps: RepSpec[];
}
interface PredSpec {
  /** When present, accept only these codecs (case-insensitive). */
  codecs?: string[];
  /** Reject these MIME types (case-insensitive). */
  rejectMimes?: string[];
  /** Reject these representation ids (the primary is never rejected by id). */
  rejectIds?: number[];
}
interface Case {
  id: string;
  track: TrackSpec;
  pref?: unknown;
  predicate: PredSpec;
}

function buildTrack(spec: TrackSpec): Track {
  const reps: RepresentationRef[] = spec.reps.map((r) => ({
    id: r.id,
    size: 500 + r.id,
    url: `/api/v1/tracks/14/representations/${r.id}/audio`,
    ...(r.codec === undefined
      ? {}
      : { codec: { codec: r.codec, mimeType: r.mimeType ?? `audio/${r.codec}` } }),
  }));
  return {
    id: 14,
    number: 2,
    title: 'T4',
    artists: [],
    audio: { id: 14, size: 1014, url: '/api/v1/tracks/14/audio' },
    ...(spec.codec === undefined
      ? {}
      : { codec: { codec: spec.codec, mimeType: spec.mimeType ?? `audio/${spec.codec}` } }),
    representations: reps,
  } as Track;
}

function makeCanPlay(spec: PredSpec): CanPlay {
  return (c) => {
    const source = c.source ?? { kind: 'primary' as const };
    if (
      spec.rejectIds &&
      source.kind === 'representation' &&
      spec.rejectIds.includes(source.id)
    ) {
      return false;
    }
    const mime = (c.mimeType ?? '').toLowerCase();
    if (spec.rejectMimes && spec.rejectMimes.map((m) => m.toLowerCase()).includes(mime)) {
      return false;
    }
    if (spec.codecs) {
      const codec = (c.codec ?? '').toLowerCase();
      return spec.codecs.map((x) => x.toLowerCase()).includes(codec);
    }
    return true;
  };
}

const PRIMARY = { codec: 'musepack-sv8', mimeType: 'audio/musepack' };

const cases: Case[] = [
  // Default / undefined behaviour.
  { id: 'default-undefined', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, predicate: {} },
  { id: 'default-explicit', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }, { id: 78, codec: 'flac' }] }, pref: { mode: 'default' }, predicate: {} },
  { id: 'no-representations-lossless', track: { ...PRIMARY, reps: [] }, pref: { mode: 'lossless' }, predicate: {} },
  { id: 'no-representations-codec', track: { ...PRIMARY, reps: [] }, pref: { mode: 'codec', codec: 'flac' }, predicate: {} },
  { id: 'no-representations-none-playable', track: { ...PRIMARY, reps: [] }, predicate: { codecs: [] } },

  // Malformed preferences.
  { id: 'malformed-null', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: null, predicate: {} },
  { id: 'malformed-mode', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: { mode: 'shiny' }, predicate: {} },
  { id: 'malformed-id-type', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: { mode: 'representation', id: '77' }, predicate: {} },
  { id: 'malformed-empty-codec', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: { mode: 'codec', codec: '' }, predicate: {} },
  { id: 'malformed-string', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: 'lossless', predicate: {} },

  // Explicit representation.
  { id: 'representation-exists', track: { ...PRIMARY, reps: [{ id: 76, codec: 'flac' }, { id: 77, codec: 'wav' }] }, pref: { mode: 'representation', id: 77 }, predicate: {} },
  { id: 'representation-absent', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: { mode: 'representation', id: 99 }, predicate: {} },
  { id: 'representation-unplayable', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: { mode: 'representation', id: 77 }, predicate: { codecs: ['musepack-sv8'] } },
  { id: 'representation-rejected-by-id', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: { mode: 'representation', id: 77 }, predicate: { rejectIds: [77] } },
  { id: 'representation-fractional', track: { ...PRIMARY, reps: [{ id: 76.5, codec: 'flac' }] }, pref: { mode: 'representation', id: 76.5 }, predicate: {} },

  // Codec preference.
  { id: 'codec-case-insensitive-first', track: { ...PRIMARY, reps: [{ id: 75, codec: 'mp3' }, { id: 76, codec: 'FLAC' }, { id: 77, codec: 'flac' }] }, pref: { mode: 'codec', codec: 'flac' }, predicate: {} },
  { id: 'codec-no-match', track: { ...PRIMARY, reps: [{ id: 75, codec: 'mp3' }] }, pref: { mode: 'codec', codec: 'ogg' }, predicate: {} },
  { id: 'codec-skips-unplayable', track: { ...PRIMARY, reps: [{ id: 75, codec: 'flac', mimeType: 'audio/x-broken' }, { id: 76, codec: 'flac', mimeType: 'audio/flac' }] }, pref: { mode: 'codec', codec: 'flac' }, predicate: { rejectMimes: ['audio/x-broken'] } },
  { id: 'codec-all-unplayable', track: { ...PRIMARY, reps: [{ id: 75, codec: 'flac' }, { id: 76, codec: 'flac' }] }, pref: { mode: 'codec', codec: 'flac' }, predicate: { codecs: ['mp3'] } },
  { id: 'codec-uppercase-preference', track: { ...PRIMARY, reps: [{ id: 75, codec: 'flac' }] }, pref: { mode: 'codec', codec: 'FLAC' }, predicate: {} },

  // Lossless preference.
  { id: 'lossless-first-in-order', track: { ...PRIMARY, reps: [{ id: 73, codec: 'mp3' }, { id: 74, codec: 'aiff' }, { id: 75, codec: 'flac' }] }, pref: { mode: 'lossless' }, predicate: {} },
  { id: 'lossless-ignores-lossy', track: { ...PRIMARY, reps: [{ id: 73, codec: 'mp3' }, { id: 74, codec: 'opus' }] }, pref: { mode: 'lossless' }, predicate: {} },
  { id: 'lossless-wav', track: { ...PRIMARY, reps: [{ id: 79, codec: 'wav' }] }, pref: { mode: 'lossless' }, predicate: {} },
  { id: 'lossless-unplayable-then-playable', track: { ...PRIMARY, reps: [{ id: 74, codec: 'aiff' }, { id: 75, codec: 'flac' }] }, pref: { mode: 'lossless' }, predicate: { rejectIds: [74] } },

  // Availability fallback / rescue.
  { id: 'rescue-unplayable-primary', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, predicate: { codecs: ['musepack-x', 'flac'] } },
  { id: 'primary-preferred-when-playable', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, predicate: { codecs: ['musepack-sv8'] } },
  { id: 'nothing-playable-primary', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }] }, pref: { mode: 'lossless' }, predicate: { codecs: [] } },
  { id: 'rescue-order', track: { ...PRIMARY, reps: [{ id: 76, codec: 'wav' }, { id: 77, codec: 'flac' }] }, predicate: { codecs: ['flac'] } },
  { id: 'primary-codec-absent-playable', track: { reps: [{ id: 77, codec: 'flac' }] }, predicate: {} },
  { id: 'primary-codec-absent-unplayable', track: { reps: [{ id: 77, codec: 'flac' }] }, predicate: { codecs: [] } },

  // Determinism / ties.
  { id: 'duplicate-ids-first', track: { ...PRIMARY, reps: [{ id: 77, codec: 'flac' }, { id: 77, codec: 'wav' }] }, pref: { mode: 'representation', id: 77 }, predicate: {} },
  { id: 'same-codec-order', track: { ...PRIMARY, reps: [{ id: 10, codec: 'flac' }, { id: 11, codec: 'flac' }] }, pref: { mode: 'codec', codec: 'flac' }, predicate: {} },
];

const out: string[] = [];
function emit(obj: unknown): void {
  out.push(JSON.stringify(obj));
}

for (const c of cases) {
  const track = buildTrack(c.track);
  const canPlay = makeCanPlay(c.predicate);
  const chosen = resolveAudio(track, c.pref as never, canPlay).representation;
  const parsed = parseAudioPreference(c.pref);
  emit({
    id: c.id,
    track: c.track,
    pref: c.pref === undefined ? undefined : c.pref,
    predicate: c.predicate,
    prefParsed: parsed ?? null,
    expected: { representation: chosen ? chosen.id : null },
  });
}

process.stdout.write(out.join('\n') + '\n');
