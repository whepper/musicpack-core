// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// The authoring draft store: in-memory editable state for the album being
// authored. Plain TS + the minimal store primitive, so the editing logic is
// unit-testable without the Svelte runtime (web-client convention). The GUI
// never mutates a half-created .mpack directory; a package is only produced
// from a validated draft at "Create MusicPack".

import { writable, type Writable } from './store';
import { createResult, encodeStaging, invalidateValidation } from './authoring-state';
import type {
  ArtworkEntry,
  AssetEntry,
  Draft,
  Identifiers,
  Identity,
  Medium,
  ReleaseEdition,
  SonicAnalysis,
  SourceInfo,
  Track,
  WaveformAnalysis,
} from './types';

export type ChipStatus = 'ok' | 'warn' | 'idle';

export interface ChipState {
  audio: ChipStatus;
  metadata: ChipStatus;
  artwork: ChipStatus;
  identity: ChipStatus;
  sonic: ChipStatus;
  waveform: ChipStatus;
}

export interface DraftStore {
  draft: Writable<Draft | null>;
  busy: Writable<boolean>;
  error: Writable<string | null>;

  setDraft(draft: Draft): void;
  clear(): void;
  setBusy(b: boolean): void;
  setError(e: string | null): void;

  updateAlbum(fn: (album: Draft['album']) => void): void;
  updateRelease(fn: (release: ReleaseEdition) => void): void;
  updateIdentifiers(fn: (ids: Identifiers) => void): void;
  updateIdentity(fn: (id: Identity) => void): void;
  updateSource(fn: (src: SourceInfo) => void): void;
  updateMedium(discIndex: number, fn: (medium: Medium) => void): void;
  updateTrack(discIndex: number, trackIndex: number, patch: Partial<Track>): void;
  setArtwork(entries: ArtworkEntry[]): void;
  setAssets(kind: 'booklet' | 'lyrics' | 'extras', entries: AssetEntry[]): void;
  updateSonicAnalysis(fn: (s: SonicAnalysis) => void): void;
  updateWaveformAnalysis(fn: (s: WaveformAnalysis) => void): void;
}

function mutate(draft: Draft, fn: (d: Draft) => void): Draft {
  const next: Draft = structuredClone(draft);
  fn(next);
  return next;
}

export function createDraftStore(): DraftStore {
  const draft = writable<Draft | null>(null);
  const busy = writable<boolean>(false);
  const error = writable<string | null>(null);

  const withDraft = (fn: (d: Draft) => void): void => {
    const current = draft.get();
    // Encoded files already carry these tags, so edits must wait for a new
    // source load rather than allowing the manifest and audio to diverge.
    if (current && !encodeStaging.get()) {
      draft.set(mutate(current, fn));
      invalidateValidation();
    }
  };

  return {
    draft,
    busy,
    error,
    setDraft(d: Draft) {
      draft.set(structuredClone(d));
      createResult.set(null);
      invalidateValidation();
    },
    clear() {
      draft.set(null);
      error.set(null);
      createResult.set(null);
      invalidateValidation();
    },
    setBusy(b: boolean) {
      busy.set(b);
    },
    setError(e: string | null) {
      error.set(e);
    },

    updateAlbum(fn) {
      withDraft((d) => fn(d.album));
    },
    updateRelease(fn) {
      withDraft((d) => {
        if (!d.release) d.release = {};
        fn(d.release);
      });
    },
    updateIdentifiers(fn) {
      withDraft((d) => {
        if (!d.identifiers) d.identifiers = {};
        fn(d.identifiers);
      });
    },
    updateIdentity(fn) {
      withDraft((d) => {
        if (!d.identity) d.identity = {};
        fn(d.identity);
      });
    },
    updateSource(fn) {
      withDraft((d) => {
        if (!d.source) d.source = {};
        fn(d.source);
      });
    },
    updateMedium(discIndex: number, fn: (medium: Medium) => void) {
      withDraft((d) => {
        const medium = d.media[discIndex];
        if (medium) fn(medium);
      });
    },
    updateTrack(discIndex: number, trackIndex: number, patch: Partial<Track>) {
      withDraft((d) => {
        const medium = d.media[discIndex];
        const track = medium?.tracks[trackIndex];
        if (track) Object.assign(track, patch);
      });
    },
    setArtwork(entries: ArtworkEntry[]) {
      withDraft((d) => {
        d.artwork = structuredClone(entries);
      });
    },
    setAssets(kind: 'booklet' | 'lyrics' | 'extras', entries: AssetEntry[]) {
      withDraft((d) => {
        d[kind] = structuredClone(entries);
      });
    },
    updateSonicAnalysis(fn: (s: SonicAnalysis) => void) {
      // Unlike metadata/track edits, a Sonic result is an analysis document
      // path, not a source tag: it can never diverge from the encoded audio,
      // so it must persist even while encode staging exists (Sonic may
      // legitimately run after encoding).
      const current = draft.get();
      if (current) {
        draft.set(
          mutate(current, (d) => {
            if (!d.sonicAnalysis) d.sonicAnalysis = { status: 'not_analysed' };
            fn(d.sonicAnalysis);
          }),
        );
        invalidateValidation();
      }
    },
    updateWaveformAnalysis(fn: (s: WaveformAnalysis) => void) {
      // Like Sonic, waveform results are derived stage output (per-track
      // .wfm files + manifest references) that can never diverge from the
      // encoded audio; they survive encode staging. Status flips between
      // 'not_generated' / 'pending' / 'ready' / 'disabled' / 'error'; the
      // build step reads the same block.
      const current = draft.get();
      if (current) {
        draft.set(
          mutate(current, (d) => {
            if (!d.waveformAnalysis) {
              d.waveformAnalysis = {
                status: 'not_generated',
                intervalMs: 100,
                encoding: 'peak-rms-u8',
                floorDb: -60,
                tracks: [],
              };
            }
            fn(d.waveformAnalysis);
          }),
        );
        invalidateValidation();
      }
    },
  };
}

// Footer chip status derived from the draft (instant, local). The
// authoritative validation still comes from `validate-draft`.
export function chipState(d: Draft): ChipState {
  const tracks = d.media.reduce((n, m) => n + m.tracks.length, 0);
  const metadataOk = d.album.title.trim().length > 0 && d.album.artists.length > 0;
  const conf = d.identity?.confidence;
  const sonic = d.sonicAnalysis;
  const wf = d.waveformAnalysis;
  return {
    audio: tracks > 0 ? 'ok' : 'warn',
    metadata: metadataOk ? 'ok' : 'warn',
    artwork: d.artwork.length > 0 ? 'ok' : 'warn',
    identity:
      conf === 'exact' || conf === 'confirmed'
        ? 'ok'
        : conf === 'probable'
          ? 'warn'
          : 'idle',
    sonic:
      sonic?.status === 'ready' || sonic?.status === 'ready-with-warnings'
        ? 'ok'
        : sonic?.status === 'error'
          ? 'warn'
          : 'idle',
    waveform:
      wf?.status === 'ready'
        ? 'ok'
        : wf?.status === 'disabled'
          ? 'idle'
          : wf?.status === 'error'
            ? 'warn'
            : 'idle',
  };
}
