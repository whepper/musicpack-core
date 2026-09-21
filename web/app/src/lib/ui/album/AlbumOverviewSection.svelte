<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album page, Overview section: the track list plus a one-line "how this
  // plays" note. Pure presentation — data and callbacks come from the page.
  import TrackList from '../TrackList.svelte';
  import { codecLabel, qualityLine } from '../../format';
  import type { ReleaseDetail } from '../../api/types';
  import type { ReleaseFacts } from '../album-facts';

  let {
    rel,
    facts,
    currentTrackId,
    onPlay,
    audioHref,
  }: {
    rel: ReleaseDetail;
    facts: ReleaseFacts;
    currentTrackId: number | undefined;
    onPlay: (index: number) => void;
    audioHref: string;
  } = $props();

  const primary = $derived(facts.primary);
</script>

<h2 class="smallcaps section-heading">
  {facts.discCount} {facts.discCount === 1 ? 'medium' : 'media'} · {facts.trackCount} tracks
</h2>
<TrackList release={rel} currentTrackId={currentTrackId} onPlay={onPlay} />
{#if primary}
  <p class="muted section-note">
    Plays as {qualityLine({ codec: primary.codec.codec, sampleRate: primary.codec.sampleRate, channels: primary.codec.channels }) || codecLabel(primary.codec.codec)}
    {#if rel.loudness} · album {rel.loudness.albumLufs.toFixed(1)} LUFS{/if}
    — <a href={audioHref}>audio details</a>
  </p>
{/if}
