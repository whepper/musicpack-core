// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// BS.1770 playback normalization (client policy — .mpack values are never
// modified). gain = playback target - measured loudness, constrained so the
// output true peak never exceeds TRUE_PEAK_CAP_DB.

export type NormalizationMode = 'off' | 'album' | 'track';

/** Initial client playback target (documented playback policy). */
export const PLAYBACK_TARGET_LUFS = -16;
/** Ceiling applied to the normalized true peak (headroom for downstream). */
export const TRUE_PEAK_CAP_DB = -1;

export interface TrackLoudnessLike {
  lufs: number;
  truePeakDb: number;
}

export interface AlbumLoudnessLike {
  albumLufs: number;
  albumTruePeakDb: number;
}

/** Normalization gain in dB for a track under the given mode. */
export function normalizationGainDb(
  mode: NormalizationMode,
  track?: TrackLoudnessLike,
  album?: AlbumLoudnessLike,
): number {
  if (mode === 'off') return 0;
  const measured = mode === 'album' ? album?.albumLufs : track?.lufs;
  const peak = mode === 'album' ? album?.albumTruePeakDb : track?.truePeakDb;
  if (measured === undefined) return 0;
  let gain = PLAYBACK_TARGET_LUFS - measured;
  if (peak !== undefined) {
    const maxGain = TRUE_PEAK_CAP_DB - peak;
    if (gain > maxGain) gain = maxGain;
  }
  return gain;
}

export function dbToLinear(db: number): number {
  return Math.pow(10, db / 20);
}

/** Combined linear gain = user volume (0..1) x normalization gain. */
export function combinedGain(userVolume: number, normDb: number): number {
  return userVolume * dbToLinear(normDb);
}
