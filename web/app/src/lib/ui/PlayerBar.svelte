<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { player, playerModel } from '../bootstrap';
  import { fmtTime, playingFormatLine } from '../format';
  import WaveformSeek from './WaveformSeek.svelte';
  import type { Track } from '../api/types';

  /**
   * Structural view of the playing queue item (player-core QueueItem):
   * codec hint + representation id feed the format line, `source.kind`
   * distinguishes offline playback.
   */
  type NowPlayingItem = {
    artworkUrl?: string;
    artist: string;
    albumTitle: string;
    edition?: string;
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
  // The seek control (and these labels) describe the CURRENT TRACK, not the
  // album-spanning timeline the transport position is measured on.
  const withinPos = $derived(Math.max(0, Math.min(trackDur, pos - trackStart)));

  const normLabel = $derived(
    $playerModel.normalizeMode === 'album' ? 'Album' : $playerModel.normalizeMode === 'track' ? 'Track' : 'Off',
  );

  /// The QueueItem carries the Track verbatim from ReleaseDetail, which
  /// includes the `waveform` field when the server populates it. Forward
  /// it through to the WaveformSeek component.
  const wfTrack = $derived<Track | null>(item?.track ?? null);
  // Codec transparency: what is actually sounding (codec +, for lossless,
  // rate/channels when probed). The manifest label wins when present.
  const playingFormat = $derived(item ? playingFormatLine(item) : '');
  // Offline playback: the source kind is the real state, never inferred.
  const offlinePlayback = $derived(item?.source.kind === 'local-file');
  // Thin gold progress fill for the linear-range fallback (waveform tracks
  // render their own elapsed region).
  const rangeFill = $derived(
    dur > 0 ? Math.max(0, Math.min(100, (pos / dur) * 100)) : 0,
  );
</script>

{#if item}
  <div class="playerbar" aria-label="Now playing">
    <div class="now">
      <img class="thumb" src={item.artworkUrl ?? '/placeholder.svg'} alt="" aria-hidden="true"
        onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')}>
      <div class="now-meta">
        <div class="tt">{item.track.title}</div>
        <div class="art">{item.artist} — {item.albumTitle}{item.edition ? ` · ${item.edition}` : ''}</div>
        <div class="pb-format smallcaps">
          {#if offlinePlayback}<span class="pb-offline">⤓ Offline</span>{/if}
          {#if playingFormat}<span>{playingFormat}</span>{/if}
        </div>
      </div>
    </div>

    <div class="transport">
      <button aria-label="Previous track" onclick={() => void player.previous()}>
        <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M6 5h2v14H6zM20 5v14l-11-7z"/></svg>
      </button>
      <button class="play-toggle" aria-label={$playerModel.state === 'playing' ? 'Pause' : 'Play'} onclick={() => void player.togglePlay()}>
        {#if $playerModel.state === 'playing'}
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 5h4v14H7zM13 5h4v14h-4z"/></svg>
        {:else}
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5l11 7-11 7z"/></svg>
        {/if}
      </button>
      <button aria-label="Next track" onclick={() => void player.next()}>
        <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M16 5h2v14h-2zM4 5l11 7-11 7z"/></svg>
      </button>
      <div class="progress">
        <span class="time">{fmtTime(withinPos)}</span>
        {#if wfTrack && wfTrack.waveform}
          <WaveformSeek
            track={wfTrack}
            startSeconds={trackStart}
            durationSeconds={trackDur}
            positionSeconds={pos}
            onSeek={(s) => void player.seek(s)}
            disabled={!item}
          />
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
            disabled={!item}
          >
        {/if}
        <span class="time">{fmtTime(trackDur)}</span>
      </div>
    </div>

    <div class="right-zone">
      <button
        class="smallcaps"
        aria-label={`Repeat: ${$playerModel.repeat === 'off' ? 'off' : $playerModel.repeat === 'one' ? 'one track' : 'all'}. Click to change.`}
        title={`Repeat: ${$playerModel.repeat}`}
        style={$playerModel.repeat !== 'off' ? 'color:var(--accent)' : ''}
        onclick={() =>
          player.setRepeat(
            $playerModel.repeat === 'off' ? 'all' : $playerModel.repeat === 'all' ? 'one' : 'off',
          )
        }
      >
        {$playerModel.repeat === 'one' ? '↻1' : $playerModel.repeat === 'all' ? '↻All' : '↻'}
      </button>
      <button
        class="smallcaps"
        aria-label={`Shuffle: ${$playerModel.shuffle ? 'on' : 'off'}. Click to toggle.`}
        title="Shuffle"
        style={$playerModel.shuffle ? 'color:var(--accent)' : ''}
        onclick={() => player.setShuffle(!$playerModel.shuffle)}
      >
        ⤨
      </button>
      <button
        class="smallcaps"
        aria-label={`${$playerModel.crossfadeSeconds === 0 ? 'Crossfade off' : `Maximum Crossfade: ${$playerModel.crossfadeSeconds} s`} (Smart Fades adapt to the music). Click to change.`}
        title={`${$playerModel.crossfadeSeconds === 0 ? 'Crossfade off' : `Maximum Crossfade: ${$playerModel.crossfadeSeconds} s`} (Smart Fades adapt to the music)`}
        style={$playerModel.crossfadeSeconds > 0 ? 'color:var(--accent)' : ''}
        onclick={() =>
          player.setCrossfade(
            $playerModel.crossfadeSeconds === 0 ? 4 : $playerModel.crossfadeSeconds === 4 ? 8 : $playerModel.crossfadeSeconds === 8 ? 12 : 0,
          )
        }
      >
        {$playerModel.crossfadeSeconds > 0 ? `⤡${$playerModel.crossfadeSeconds}` : '⤡'}
      </button>
      <button
        class="smallcaps"
        aria-label={`Normalization: ${normLabel}. Click to change.`}
        title={`Loudness normalization: ${normLabel}`}
        onclick={() =>
          player.setNormalizeMode(
            $playerModel.normalizeMode === 'album' ? 'track' : $playerModel.normalizeMode === 'track' ? 'off' : 'album',
          )
        }
      >
        {normLabel}{$playerModel.normDb ? ` ${$playerModel.normDb > 0 ? '+' : ''}${$playerModel.normDb.toFixed(1)} dB` : ''}
      </button>
      <label class="muted" style="display:flex;align-items:center;gap:var(--space-2)" aria-label="Volume">
        <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M3 9v6h4l5 5V4L7 9H3zm13.6 3a4 4 0 0 0-2-3.5v7a4 4 0 0 0 2-3.5zM15 4v2.2a6 6 0 0 1 0 11.6V20a8 8 0 0 0 0-16z"/></svg>
        <input
          type="range" min="0" max="1" step="0.01"
          value={$playerModel.volume}
          aria-label="Volume"
          oninput={(e) => player.setVolume(Number((e.currentTarget as HTMLInputElement).value))}
          style={`width:90px;--range-fill:${($playerModel.volume * 100).toFixed(1)}%`}
        >
      </label>
      <button class="smallcaps" aria-label="Open the queue" onclick={() => window.dispatchEvent(new CustomEvent('musicpack:queue'))}>
        Queue
      </button>
    </div>
  </div>
{/if}
