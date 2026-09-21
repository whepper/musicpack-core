<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Now Playing — the expanded player surface. Same system as the compact
  // bar and the queue drawer: artwork hero, serif display title, the same
  // format/normalization facts, the gold play control, and the shared
  // QueueList below.
  import { player, playerModel } from '../bootstrap';
  import { fmtTime, playingFormatLine } from '../format';
  import WaveformSeek from './WaveformSeek.svelte';
  import QueueList from './QueueList.svelte';
  import type { Track } from '../api/types';

  type NowPlayingItem = {
    artworkUrl?: string;
    artist: string;
    albumTitle: string;
    edition?: string;
    albumId: number;
    codec?: string;
    representationId?: number;
    source: { kind: string };
    track: Track;
  };

  const item = $derived($playerModel.current as NowPlayingItem | null);
  let drag = $state<number | null>(null);
  const pos = $derived(drag ?? $playerModel.positionSeconds);
  const trackStart = $derived($playerModel.currentTrackStartSeconds);
  const trackDur = $derived($playerModel.currentTrackDurationSeconds);
  const withinPos = $derived(Math.max(0, Math.min(trackDur, pos - trackStart)));

  const wfTrack = $derived<Track | null>(item?.track ?? null);
  const playingFormat = $derived(item ? playingFormatLine(item) : '');
  const offlinePlayback = $derived(item?.source.kind === 'local-file');
  const stateLabel = $derived(
    $playerModel.state === 'playing' ? 'Playing' : $playerModel.state === 'paused' ? 'Paused' : $playerModel.state,
  );
  const normLine = $derived.by(() => {
    if ($playerModel.normalizeMode === 'off') return 'Normalization off';
    const mode = $playerModel.normalizeMode === 'album' ? 'Album' : 'Track';
    return `Normalization ${mode}${$playerModel.normDb ? ` ${$playerModel.normDb > 0 ? '+' : ''}${$playerModel.normDb.toFixed(1)} dB` : ''}`;
  });
  const rangeFill = $derived(
    trackDur > 0 ? Math.max(0, Math.min(100, (withinPos / trackDur) * 100)) : 0,
  );
</script>

<div class="now-playing">
  {#if item}
    <div class="np-hero">
      <img
        class="np-art"
        src={item.artworkUrl ?? '/placeholder.svg'}
        alt=""
        aria-hidden="true"
        onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')}
      >
      <div class="np-body">
        <p class="eyebrow">
          {stateLabel}
          {#if offlinePlayback}<span class="np-offline">· ⤓ Offline</span>{/if}
        </p>
        <h1 class="np-title">{item.track.title}</h1>
        <p class="np-context">
          {item.artist} — <a href={`/albums/${item.albumId}`}>{item.albumTitle}</a>
          {#if item.edition} · {item.edition}{/if}
        </p>
        <p class="np-format smallcaps">
          {#if playingFormat}{playingFormat} · {/if}{normLine}
        </p>

        <div class="np-seek">
          <span class="time smallcaps">{fmtTime(withinPos)}</span>
          <div class="np-seek-control">
            {#if wfTrack && wfTrack.waveform}
              <WaveformSeek
                track={wfTrack}
                startSeconds={trackStart}
                durationSeconds={trackDur}
                positionSeconds={pos}
                onSeek={(s) => void player.seek(s)}
              />
            {:else}
              <input
                type="range"
                min="0"
                max={trackDur > 0 ? trackDur : 0}
                step="0.5"
                value={withinPos}
                aria-label="Seek position"
                style={`--range-fill:${rangeFill}%`}
                oninput={(e) => (drag = Number((e.currentTarget as HTMLInputElement).value))}
                onchange={(e) => {
                  drag = null;
                  void player.seek(trackStart + Number((e.currentTarget as HTMLInputElement).value));
                }}
              >
            {/if}
          </div>
          <span class="time smallcaps">{fmtTime(trackDur)}</span>
        </div>

        <div class="np-transport">
          <button class="btn ghost" onclick={() => void player.previous()}>Previous</button>
          <button
            class="play-toggle np-play"
            aria-label={$playerModel.state === 'playing' ? 'Pause' : 'Play'}
            onclick={() => void player.togglePlay()}
          >
            {#if $playerModel.state === 'playing'}
              <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor" aria-hidden="true"><path d="M7 5h4v14H7zM13 5h4v14h-4z"/></svg>
            {:else}
              <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor" aria-hidden="true"><path d="M8 5l11 7-11 7z"/></svg>
            {/if}
          </button>
          <button class="btn ghost" onclick={() => void player.next()}>Next</button>
        </div>
      </div>
    </div>
  {:else}
    <div class="shelf-header">
      <div class="shelf-title-wrap">
        <p class="eyebrow">Playback</p>
        <h1>Now playing</h1>
      </div>
    </div>
    <p class="empty-state">Nothing playing yet — choose an album from the shelf.</p>
  {/if}

  <div class="np-queue">
    <h2 class="smallcaps np-queue-heading">Queue</h2>
    <QueueList />
  </div>
</div>
