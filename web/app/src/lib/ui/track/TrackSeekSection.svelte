<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page seek control: the waveform seek when the track carries an
  // envelope, else a plain labelled range input. Pure presentation over a
  // seek timeline + one callback.
  import WaveformSeek from '../WaveformSeek.svelte';
  import { fmtTime } from '../../format';
  import type { Track } from '../../api/types';
  import type { SeekTimeline } from '../track-facts';

  let {
    t,
    timeline,
    positionSeconds,
    onSeek,
  }: {
    t: Track;
    timeline: SeekTimeline;
    /** Live player position (album-absolute), for the waveform seek. */
    positionSeconds: number;
    onSeek: (albumAbsoluteSeconds: number, trackRelativeSeconds: number) => void;
  } = $props();
</script>

<section class="track-seek">
  <span class="time smallcaps">{fmtTime(timeline.withinPos)}</span>
  <div class="track-seek-control">
    {#if t.waveform}
      <WaveformSeek
        track={t}
        startSeconds={timeline.trackStart}
        durationSeconds={timeline.trackDur}
        positionSeconds={positionSeconds}
        onSeek={(s) => onSeek(s, s - timeline.trackStart)}
      />
    {:else if t.duration}
      <input
        type="range"
        min="0"
        max={t.duration}
        step="0.5"
        value={timeline.withinPos}
        aria-label="Seek position"
        style={`--range-fill:${timeline.rangeFill}%`}
        oninput={(event) => {
          if (!timeline.isCurrent) return;
          const v = Number((event.currentTarget as HTMLInputElement).value);
          onSeek(timeline.trackStart + v, v);
        }}
        onchange={(event) => {
          if (!timeline.isCurrent) return;
          const v = Number((event.currentTarget as HTMLInputElement).value);
          onSeek(timeline.trackStart + v, v);
        }}
      >
    {/if}
  </div>
  <span class="time smallcaps">{t.duration ? fmtTime(t.duration) : '—'}</span>
</section>
