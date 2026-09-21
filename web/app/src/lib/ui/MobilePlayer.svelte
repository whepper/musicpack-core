<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { player, playerModel } from '../bootstrap';
  import { fmtTime, playingFormatLine } from '../format';
  import WaveformSeek from './WaveformSeek.svelte';
  import type { Track } from '../api/types';

  type NowPlayingItem = {
    artworkUrl?: string;
    artist: string;
    codec?: string;
    representationId?: number;
    source: { kind: string };
    track: Track;
  };

  const item = $derived($playerModel.current as NowPlayingItem | null);
  let drag = $state<number | null>(null);
  const pos = $derived(drag ?? $playerModel.positionSeconds);
  const dur = $derived($playerModel.durationSeconds);
  const trackStart = $derived($playerModel.currentTrackStartSeconds);
  const trackDur = $derived($playerModel.currentTrackDurationSeconds);
  // Track-relative elapsed, matching the track-scoped seek control.
  const withinPos = $derived(Math.max(0, Math.min(trackDur, pos - trackStart)));

  const wfTrack = $derived<Track | null>(item?.track ?? null);
  // Codec transparency, same facts as the desktop bar.
  const playingFormat = $derived(item ? playingFormatLine(item) : '');
  const offlinePlayback = $derived(item?.source.kind === 'local-file');
  const rangeFill = $derived(
    dur > 0 ? Math.max(0, Math.min(100, (pos / dur) * 100)) : 0,
  );
</script>

{#if item}
  <div class="mobile-player">
    {#if wfTrack && wfTrack.waveform}
      <div style="height:36px;width:100%;display:block">
        <WaveformSeek
          track={wfTrack}
          startSeconds={trackStart}
          durationSeconds={trackDur}
          positionSeconds={pos}
          onSeek={(s) => void player.seek(s)}
        />
      </div>
    {:else}
      <input
        type="range"
        min="0"
        max={dur > 0 ? dur : 0}
        step="0.5"
        value={pos}
        aria-label="Seek position"
        style={`--range-fill:${rangeFill}%`}
        oninput={(e) => (drag = Number((e.currentTarget as HTMLInputElement).value))}
        onchange={(e) => {
          drag = null;
          void player.seek(Number((e.currentTarget as HTMLInputElement).value));
        }}
      >
    {/if}
    <div class="row">
      <img class="thumb" src={item.artworkUrl ?? '/placeholder.svg'} alt="" aria-hidden="true"
        onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')}>
      <button style="min-width:0;text-align:left" onclick={() => window.dispatchEvent(new CustomEvent('musicpack:queue'))}>
        <div class="tt">{item.track.title}</div>
        <div class="art">
          {#if offlinePlayback}<span class="pb-offline">⤓ </span>{/if}
          {item.artist}{playingFormat ? ` · ${playingFormat}` : ''}
        </div>
      </button>
      <button aria-label="Previous track" onclick={() => void player.previous()}>
        <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor"><path d="M6 5h2v14H6zM20 5v14l-11-7z"/></svg>
      </button>
      <button class="play-toggle" aria-label={$playerModel.state === 'playing' ? 'Pause' : 'Play'} onclick={() => void player.togglePlay()}>
        {#if $playerModel.state === 'playing'}
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor"><path d="M7 5h4v14H7zM13 5h4v14h-4z"/></svg>
        {:else}
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor"><path d="M8 5l11 7-11 7z"/></svg>
        {/if}
      </button>
      <button aria-label="Next track" onclick={() => void player.next()}>
        <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor"><path d="M16 5h2v14h-2zM4 5l11 7-11 7z"/></svg>
      </button>
    </div>
    <div style="display:flex;justify-content:space-between;padding:0 var(--space-4) var(--space-2)">
      <span class="time smallcaps">{fmtTime(withinPos)}</span>
      <span class="time smallcaps">{fmtTime(trackDur)}</span>
    </div>
  </div>
{/if}
