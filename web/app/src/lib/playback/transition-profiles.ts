// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Web host side of the Sweet-Fade planner: derives BoundaryProfiles from
// the per-track waveform envelopes (specs/musicpack-waveform-v1.md) that
// the server already serves, and adapts them to the player-core
// planTransition port. Pure acquisition + normalization lives here; the
// POLICY is pure in player-core/src/transition.ts.
//
// Energy values are normalized to each window's own peak, so the absolute
// u8 scale (and floorDb interpretation) of the envelope is irrelevant to
// transition decisions.

import {
  planTransition,
  type BoundaryProfile,
  type TransitionPlan,
} from '../../../../player-core/src/transition';
import type { Track } from '../api/types';
import type { QueueItem } from '../state/queue';
import { fetchWaveform } from './waveform';

/** How much of each edge informs the decision. */
const WINDOW_SECONDS = 10;
/** musicpack-waveform-v1 fixes buckets at 100 ms. */
const SECONDS_PER_BUCKET = 0.1;

export interface TransitionPlannerDeps {
  fetchImpl?: typeof fetch;
  base: string;
  token: () => string | null;
}

function normalizeWindow(rms: Uint8Array, from: number, to: number): number[] {
  const slice: number[] = [];
  let max = 0;
  for (let i = from; i < to; i++) {
    const v = rms[i] ?? 0;
    slice.push(v);
    if (v > max) max = v;
  }
  if (max <= 0) return slice.map(() => 0);
  return slice.map((v) => v / max);
}

async function profileFor(
  deps: TransitionPlannerDeps,
  track: Track,
): Promise<BoundaryProfile | null> {
  if (!track.waveform) return null;
  const bars = await fetchWaveform(
    deps.fetchImpl ?? fetch,
    deps.base,
    () => deps.token() ?? undefined,
    track.id,
  ).catch(() => null);
  if (!bars || bars.points === 0) return null;
  const perSecond = 1000 / (track.waveform.intervalMs || SECONDS_PER_BUCKET * 1000);
  const windowBuckets = Math.max(1, Math.round(WINDOW_SECONDS * perSecond));
  const tailFrom = Math.max(0, bars.points - windowBuckets);
  const headTo = Math.min(bars.points, windowBuckets);
  return {
    lengthSeconds: bars.points / perSecond,
    tail: normalizeWindow(bars.rms, tailFrom, bars.points),
    head: normalizeWindow(bars.rms, 0, headTo),
  };
}

/**
 * Builds the PlayerPorts.planTransition adapter plus its prefetch hook.
 * Profiles are fetched asynchronously via prime() (call it on track events
 * for the current and next item); queries resolve synchronously from
 * whatever has arrived, degrading to the legacy fixed-length fade when
 * data is missing.
 */
export interface TransitionPlanner {
  plan: (query: {
    outgoing: import('../../../../player-core/src/types').PlaybackItem;
    incoming: import('../../../../player-core/src/types').PlaybackItem;
    maxFadeSeconds: number;
    repeatOne: boolean;
  }) => TransitionPlan;
  /** Fire-and-forget profile prefetch for one queue item. */
  prime: (item: unknown) => void;
}

export function createTransitionPlanner(deps: TransitionPlannerDeps): TransitionPlanner {
  const ready = new Map<number, BoundaryProfile | null>();
  const pending = new Set<number>();

  const prime = (item: unknown): void => {
    const track = (item as QueueItem | null)?.track;
    if (!track || pending.has(track.id) || ready.has(track.id)) return;
    pending.add(track.id);
    void profileFor(deps, track).then((p) => {
      ready.set(track.id, p);
      pending.delete(track.id);
    });
  };

  const plan = (query: Parameters<TransitionPlanner['plan']>[0]): TransitionPlan => {
    const outgoing = query.outgoing as QueueItem;
    const incoming = query.incoming as QueueItem;
    const sameRelease =
      typeof outgoing.releaseId === 'number' && outgoing.releaseId === incoming.releaseId;
    const outgoingProfile = ready.get(outgoing.trackId) ?? null;
    const incomingProfile = ready.get(incoming.trackId) ?? null;
    return planTransition(
      {
        outgoing: query.outgoing,
        incoming: query.incoming,
        maxFadeSeconds: query.maxFadeSeconds,
        repeatOne: query.repeatOne,
        sameRelease,
      },
      outgoingProfile,
      incomingProfile,
      SECONDS_PER_BUCKET,
    );
  };

  return { plan, prime };
}
