// Copyright (c) 2026, The MusicPack Development Team
// SPDX-License-Identifier: BSD-3-Clause

// Compatibility re-export: the BS.1770 gain policy moved to the platform-
// independent player-core (M1). Existing imports keep working.
export {
  PLAYBACK_TARGET_LUFS,
  TRUE_PEAK_CAP_DB,
  normalizationGainDb,
  dbToLinear,
  combinedGain,
  type NormalizationMode,
  type TrackLoudnessLike,
  type AlbumLoudnessLike,
} from '../../../../player-core/src/gain';
