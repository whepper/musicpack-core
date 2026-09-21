<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Rail summary: album loudness (BS.1770, measured as one program at
  // build time) plus a waveform peek. The normalization-gain preview uses
  // the player's own published policy constants — nothing invented.
  // Sonic analysis has no server exposure yet, so it has no row here.
  import type { ReleaseDetail, Track } from '../api/types';
  import WaveformSpark from './WaveformSpark.svelte';
  import { normalizationGainDb } from '../playback/loudness';

  let {
    rel,
    waveTrack,
    onSection,
  }: {
    rel: ReleaseDetail;
    waveTrack: Track | null;
    onSection: (id: string) => void;
  } = $props();

  const gain = $derived(
    rel.loudness
      ? normalizationGainDb('album', undefined, {
          albumLufs: rel.loudness.albumLufs,
          albumTruePeakDb: rel.loudness.albumTruePeakDb,
        })
      : null,
  );
</script>

<section class="rail-panel">
  <header>
    <h3 class="smallcaps">Analysis summary</h3>
    <button class="rail-link" aria-label="Open the full analysis section" onclick={() => onSection('analysis')}>›</button>
  </header>

  {#if rel.loudness}
    <div class="analysis-mini">
      <p class="smallcaps rail-label">Loudness</p>
      <dl class="rail-kv">
        <div><dt>Integrated</dt><dd>{rel.loudness.albumLufs.toFixed(1)} LUFS</dd></div>
        <div><dt>True peak</dt><dd>{rel.loudness.albumTruePeakDb.toFixed(1)} dBTP</dd></div>
        {#if gain !== null}
          <div><dt>Album gain</dt><dd>{gain > 0 ? '+' : ''}{gain.toFixed(1)} dB</dd></div>
        {/if}
      </dl>
    </div>
  {/if}

  {#if waveTrack?.waveform}
    <div class="analysis-mini">
      <p class="smallcaps rail-label">Waveform — {waveTrack.title}</p>
      <WaveformSpark track={waveTrack} height={44} />
    </div>
  {/if}
</section>
