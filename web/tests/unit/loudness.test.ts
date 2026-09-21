import { describe, it, expect } from 'vitest';
import {
  normalizationGainDb,
  dbToLinear,
  combinedGain,
  PLAYBACK_TARGET_LUFS,
  TRUE_PEAK_CAP_DB,
} from '../../app/src/lib/playback/loudness';

const track = { lufs: -7.19, truePeakDb: -4.19 };
const album = { albumLufs: -7.28, albumTruePeakDb: -4.19 };

describe('loudness normalization', () => {
  it('off mode applies no gain', () => {
    expect(normalizationGainDb('off', track, album)).toBe(0);
  });

  it('album mode uses the canonical album LUFS', () => {
    expect(normalizationGainDb('album', track, album)).toBeCloseTo(
      PLAYBACK_TARGET_LUFS - album.albumLufs,
      6,
    );
  });

  it('track mode uses the track LUFS', () => {
    expect(normalizationGainDb('track', track, album)).toBeCloseTo(
      PLAYBACK_TARGET_LUFS - track.lufs,
      6,
    );
  });

  it('returns 0 when the measured value is missing', () => {
    expect(normalizationGainDb('album', track)).toBe(0);
    expect(normalizationGainDb('track', undefined, album)).toBe(0);
  });

  it('true peak caps the gain so output never exceeds the ceiling', () => {
    // A loud-peak track would clip; the gain must be reduced so the output
    // true peak lands at TRUE_PEAK_CAP_DB (-1 dBTP).
    const capped = normalizationGainDb('album', track, {
      albumLufs: -30,
      albumTruePeakDb: -3,
    });
    expect(capped).toBeCloseTo(TRUE_PEAK_CAP_DB - -3, 6);
    expect(capped).toBeLessThan(PLAYBACK_TARGET_LUFS - -30);
  });

  it('linear gain converts from dB', () => {
    expect(dbToLinear(0)).toBe(1);
    expect(dbToLinear(6.0206)).toBeCloseTo(2, 3);
    expect(dbToLinear(-6.0206)).toBeCloseTo(0.5, 3);
  });

  it('combined gain multiplies user volume and normalization in the linear domain', () => {
    const norm = normalizationGainDb('album', track, album);
    expect(combinedGain(0.5, norm)).toBeCloseTo(0.5 * dbToLinear(norm), 6);
    // Volume 0 mutes regardless of normalization.
    expect(combinedGain(0, 10)).toBe(0);
  });
});
