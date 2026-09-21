<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Technical track table (Tracks / Analysis sections): flattened album
  // order with disc, duration, and the loudness + waveform facts the API
  // actually carries. Discs beyond the first start collapsed behind an
  // accessible disclosure (the Overview list stays fully expanded).
  import type { ReleaseDetail, Track } from '../api/types';
  import { fmtTime } from '../format';
  import { currentSelection, queue } from '../bootstrap';
  import { itemForTrack } from '../state/queue';
  import WaveformSpark from './WaveformSpark.svelte';

  let {
    release,
    currentTrackId,
    onPlay,
  }: {
    release: ReleaseDetail;
    currentTrackId?: number;
    onPlay: (index: number) => void;
  } = $props();

  interface Row {
    track: Track;
    disc: number;
    index: number;
  }

  const discs = $derived([...release.media].sort((a, b) => a.disc - b.disc));
  const rows = $derived.by(() => {
    const out: Row[] = [];
    let i = 0;
    for (const disc of discs) {
      for (const track of disc.tracks) {
        out.push({ track, disc: disc.disc, index: i });
        i += 1;
      }
    }
    return out;
  });
  const multiDisc = $derived(discs.length > 1);

  // null = untouched: default policy (everything past the first disc collapsed).
  let collapsedOverride = $state<Set<number> | null>(null);
  const defaultCollapsed = $derived(new Set(discs.slice(1).map((d) => d.disc)));
  const collapsed = $derived(collapsedOverride ?? defaultCollapsed);

  function toggleDisc(disc: number): void {
    const base = new Set(collapsed);
    if (base.has(disc)) base.delete(disc);
    else base.add(disc);
    collapsedOverride = base;
  }

  function addTrack(track: Track): void {
    queue.addItem(itemForTrack(release, track, release.album.title, release.album.artists[0]?.name ?? '', currentSelection()));
  }

  function peakLabel(track: Track): string {
    return track.loudness ? track.loudness.truePeakDb.toFixed(1) : '—';
  }
  function peakUnit(track: Track): string {
    return track.loudness ? 'dBTP' : '';
  }
  function lufsLabel(track: Track): string {
    return track.loudness ? track.loudness.lufs.toFixed(1) : '—';
  }
</script>

<div class="table-scroll">
<table class="track-table" class:single-disc={!multiDisc}>
  <thead>
    <tr>
      <th scope="col" class="h-num">#</th>
      <th scope="col">Title</th>
      <th scope="col" class="h-disc">Disc</th>
      <th scope="col" class="r"><span class="lbl-full">Duration</span><span class="lbl-short">Time</span></th>
      <th scope="col" class="r">Peak</th>
      <th scope="col" class="r h-lufs">LUFS</th>
      <th scope="col" class="h-wave">Waveform</th>
      <th scope="col"><span class="sr-only">Track detail</span></th>
      <th scope="col"><span class="sr-only">Add to queue</span></th>
    </tr>
  </thead>
  <tbody>
    {#each rows as row (row.track.id)}
      {#if !collapsed.has(row.disc)}
        <tr
          class="trow"
          class:now-playing={row.track.id === currentTrackId}
          aria-current={row.track.id === currentTrackId ? 'true' : undefined}
        >
          <td class="num r">{row.track.number}</td>
          <td class="cell-title">
            <button class="row-play" onclick={() => onPlay(row.index)}>{row.track.title}</button>
          </td>
          <td class="r h-disc">{row.disc}</td>
          <td class="r">{row.track.duration ? fmtTime(row.track.duration) : '—'}</td>
          <td class="r">{peakLabel(row.track)}{#if peakUnit(row.track)}{' '}<span class="peak-unit">{peakUnit(row.track)}</span>{/if}</td>
          <td class="r h-lufs">{lufsLabel(row.track)}</td>
          <td class="cell-waveform"><WaveformSpark track={row.track} /></td>
          <td>
            <a
              class="row-detail"
              href={`/tracks/${row.track.id}`}
              aria-label={`Open ${row.track.title} track detail`}
            >…</a>
          </td>
          <td>
            <button
              class="queue-add"
              aria-label={`Add ${row.track.title} to queue`}
              onclick={() => addTrack(row.track)}
            >+</button>
          </td>
        </tr>
      {/if}
    {/each}
  </tbody>
</table>
</div>

{#if multiDisc}
  <div class="disc-toggles">
    {#each discs as disc (disc.disc)}
      {#if disc.disc !== discs[0]?.disc}
        <button
          class="disc-toggle"
          aria-expanded={!collapsed.has(disc.disc)}
          onclick={() => toggleDisc(disc.disc)}
        >
          {collapsed.has(disc.disc) ? 'Show' : 'Hide'} disc {disc.disc}
          <span aria-hidden="true">{collapsed.has(disc.disc) ? '⌄' : '⌃'}</span>
        </button>
      {/if}
    {/each}
  </div>
{/if}
