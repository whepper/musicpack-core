<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page, Lyrics section (R3.4): owns this page's lyrics
  // controller instance — refs in (track detail), bytes through the API
  // client's asset path, parsing through the wasm core, position from
  // the player model (§9-§10 of docs/musicpack-lyrics-v1.md). Rendering
  // lives in ui/lyrics/LyricsPanel.svelte; this wrapper is lifecycle +
  // wiring only.
  import { api, playerModel } from '../../bootstrap';
  import { createWasmLyricsCore } from '../../playback/lyrics-core';
  import { LyricsController, withinTrackMs } from '../../playback/lyrics';
  import type { Track } from '../../api/types';
  import LyricsPanel from '../lyrics/LyricsPanel.svelte';

  let { t }: { t: Track } = $props();

  // One controller per mounted section; disposed with the page. Lyrics
  // are auxiliary content — construction can never throw.
  const lyrics = new LyricsController({
    fetchAsset: (ref) => api.assetBytes(ref.url),
    core: createWasmLyricsCore(),
  });

  $effect(() => {
    lyrics.setTrack(t.id, t.lyrics);
    return () => lyrics.dispose();
  });

  const view = $derived($lyrics);

  /** Position feed (§9): push the player model's within-track position
   *  (ms, floored) only while THIS track is the playing one. Derived per
   *  tick from the same coalesced feed the waveform seek consumes. */
  $effect(() => {
    const ms = withinTrackMs(
      {
        currentTrackId: $playerModel.current?.track.id,
        currentTrackStartSeconds: $playerModel.currentTrackStartSeconds,
        currentTrackDurationSeconds: $playerModel.currentTrackDurationSeconds,
        positionSeconds: $playerModel.positionSeconds,
      },
      t.id,
    );
    if (ms !== null) lyrics.setPositionMs(ms);
  });
</script>

<div class="album-section lyrics-section">
  <h2 class="smallcaps section-heading">
    Lyrics{#if view.lang}&nbsp;· {view.lang}{/if}
  </h2>
  <LyricsPanel {view} onRetry={() => lyrics.retry()} />
</div>
