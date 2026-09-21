<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album page, Analysis section: album loudness facts plus the loudness &
  // waveform track table.
  import InfoGrid from '../InfoGrid.svelte';
  import TrackTable from '../TrackTable.svelte';
  import { normalizationGainDb } from '../../playback/loudness';
  import type { ReleaseDetail } from '../../api/types';
  import type { ReleaseFacts } from '../album-facts';

  let {
    rel,
    facts,
    currentTrackId,
    onPlay,
  }: {
    rel: ReleaseDetail;
    facts: ReleaseFacts;
    currentTrackId: number | undefined;
    onPlay: (index: number) => void;
  } = $props();

  const albumGain = $derived(
    rel?.loudness
      ? normalizationGainDb('album', undefined, {
          albumLufs: rel.loudness.albumLufs,
          albumTruePeakDb: rel.loudness.albumTruePeakDb,
        })
      : null,
  );
</script>

{#if rel.loudness}
  <div class="section-block">
    <h3 class="smallcaps section-heading">Album loudness</h3>
    <InfoGrid
      rows={[
        ['BS.1770 loudness', `${rel.loudness.albumLufs.toFixed(1)} LUFS (true peak ${rel.loudness.albumTruePeakDb.toFixed(1)} dBTP)`],
        ['Algorithm', rel.loudness.algorithm],
        ...(albumGain !== null
          ? ([['Album normalization gain', `${albumGain > 0 ? '+' : ''}${albumGain.toFixed(1)} dB`]] as Array<[string, string]>)
          : []),
      ]}
    />
    <p class="muted">
      Measured as one continuous program at build time. The gain preview assumes album-mode
      normalization toward the player's −16 LUFS target.
    </p>
  </div>
{:else}
  <p class="muted">This edition carries no album loudness measurement.</p>
{/if}
<h3 class="smallcaps section-heading">Track loudness &amp; waveforms</h3>
<TrackTable release={rel} currentTrackId={currentTrackId} onPlay={onPlay} />
