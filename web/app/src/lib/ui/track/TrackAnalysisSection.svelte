<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page, Analysis section: loudness grid + waveform summary.
  import InfoGrid from '../InfoGrid.svelte';
  import WaveformSpark from '../WaveformSpark.svelte';
  import type { ReleaseDetail, Track } from '../../api/types';
  import type { TrackFacts } from '../track-facts';

  let {
    t,
    rel,
    facts,
    gains,
  }: {
    t: Track;
    rel: ReleaseDetail;
    facts: TrackFacts;
    gains: { album: number | null; track: number | null };
  } = $props();
</script>

<div class="album-section">
  <h2 class="smallcaps section-heading">Analysis</h2>
  {#if t.loudness || rel.loudness}
    <InfoGrid
      rows={[
        ['Track integrated', t.loudness ? `${t.loudness.lufs.toFixed(2)} LUFS` : undefined],
        ['Track true peak', t.loudness ? `${t.loudness.truePeakDb.toFixed(2)} dBTP` : undefined],
        ...(gains.track !== null
          ? ([['Track normalization gain', `${gains.track > 0 ? '+' : ''}${gains.track.toFixed(1)} dB`]] as Array<[string, string]>)
          : []),
        ['Album integrated', rel.loudness ? `${rel.loudness.albumLufs.toFixed(2)} LUFS` : undefined],
        ['Album true peak', rel.loudness ? `${rel.loudness.albumTruePeakDb.toFixed(2)} dBTP` : undefined],
        ...(gains.album !== null
          ? ([['Album normalization gain', `${gains.album > 0 ? '+' : ''}${gains.album.toFixed(1)} dB`]] as Array<[string, string]>)
          : []),
        ['Algorithm', rel.loudness?.algorithm],
      ]}
    />
    <p class="muted section-note">
      Loudness measured as one program at build time. The gain preview uses the
      player's published policy (−16 LUFS target, −1 dBTP ceiling).
    </p>
  {:else}
    <p class="muted">No loudness measurement is recorded for this track or album.</p>
  {/if}

  {#if t.waveform}
    <h3 class="smallcaps section-heading">Waveform</h3>
    <div class="track-waveform">
      <WaveformSpark track={t} height={48} />
      <p class="muted">
        {t.waveform.intervalMs} ms buckets · {t.waveform.encoding} · {t.waveform.floorDb} dB floor · {t.waveform.points} points
      </p>
    </div>
  {/if}
</div>
