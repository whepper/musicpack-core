<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause

Waveform-backed seek control. Renders the track's precomputed envelope as
a Canvas of vertical bars; click/tap/pointer-drag seeks; keyboard arrow /
Home / End seek via a visually-hidden `<input type="range">` sibling that
also receives focus and shows a visible focus ring on the canvas (so the
visual seek control is the keyboard seek surface). Plays well with
gapless transitions: the component reacts to `track` changes and fetches
the new envelope.

Position model:
- `positionSeconds` is album-absolute (the queue can span multiple tracks).
- `startSeconds` is the album-absolute start of the current track.
- `durationSeconds` is the per-track duration.

We compute within-track position = positionSeconds - startSeconds and map
seek commits back to album-absolute via `onSeek(startSeconds + within)`.

Pointer seeking belongs to the canvas (its handlers map clicks/drag to
track fractions). The hidden `<input type=range>` is the keyboard/a11y
surface only: it is pointer-transparent (`pointer-events: none`) and
covers exactly the current track ([0, durationSeconds]) so its semantics
match what is drawn.

Falls back gracefully: a missing `track.waveform` is the parent's
responsibility; this component only renders when waveform is present.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import {
    downsampleToWidth,
    fetchWaveform,
    peakToHeight,
    type WaveformBars,
  } from '../playback/waveform';
  import type { Track } from '../api/types';

  type Props = {
    track: Track;
    startSeconds: number;
    durationSeconds: number;
    positionSeconds: number;
    onSeek: (albumAbsoluteSeconds: number) => void;
    disabled?: boolean;
    fetchImpl?: typeof fetch;
    base?: string;
    token?: () => string | undefined;
  };

  const {
    track,
    startSeconds,
    durationSeconds,
    positionSeconds,
    onSeek,
    disabled = false,
    fetchImpl = fetch,
    base = '',
    token = () => undefined,
  }: Props = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let hidden: HTMLInputElement | undefined = $state();
  let container: HTMLDivElement | undefined = $state();
  let bars: WaveformBars | null = $state(null);
  let widthCss = $state(0);
  let heightCss = $state(0);
  let fetchError: string | null = $state(null);
  let fetchSeq = 0;

  const trackId = $derived(track.id);
  const waveformUrl = $derived(track.waveform?.url ?? null);

  const withinPos = $derived(
    Math.max(0, Math.min(durationSeconds, positionSeconds - startSeconds)),
  );
  const playheadFrac = $derived(
    durationSeconds > 0 ? withinPos / durationSeconds : 0,
  );

  // Re-fetch whenever the track changes.
  $effect(() => {
    const id = trackId;
    const url = waveformUrl;
    if (!url) {
      bars = null;
      return;
    }
    const seq = ++fetchSeq;
    fetchError = null;
    bars = null;
    (async () => {
      try {
        const result = await fetchWaveform(fetchImpl, base, token, id);
        if (seq !== fetchSeq) return;
        if (result === null) {
          fetchError = 'no waveform';
          bars = null;
          return;
        }
        bars = result;
        queueRedraw();
      } catch (e) {
        if (seq !== fetchSeq) return;
        fetchError = (e as Error).message;
        bars = null;
      }
    })();
  });

  // Resize observer keeps the canvas crisp.
  onMount(() => {
    if (container == null) return;
    const ro = new ResizeObserver(() => {
      const rect = container!.getBoundingClientRect();
      widthCss = Math.max(0, Math.floor(rect.width));
      heightCss = Math.max(0, Math.floor(rect.height));
      queueRedraw();
    });
    ro.observe(container);
    return () => ro.disconnect();
  });

  // Redraw on data, size, or playhead change.
  let redrawQueued = false;
  function queueRedraw(): void {
    if (redrawQueued) return;
    redrawQueued = true;
    requestAnimationFrame(() => {
      redrawQueued = false;
      draw();
    });
  }

  function draw(): void {
    if (canvas == null || bars == null) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const targetW = Math.max(1, Math.round(widthCss * dpr));
    const targetH = Math.max(1, Math.round(heightCss * dpr));
    if (canvas.width !== targetW) canvas.width = targetW;
    if (canvas.height !== targetH) canvas.height = targetH;
    const ctx = canvas.getContext('2d');
    if (ctx == null) return;

    ctx.clearRect(0, 0, canvas.width, canvas.height);
    const W = canvas.width;
    const H = canvas.height;
    const midY = H / 2;
    const halfH = H / 2;

    const scaled = downsampleToWidth(bars, W);
    const elapsedFrac = Math.max(0, Math.min(1, playheadFrac));

    // Background bars: dim color for the un-elapsed region.
    for (let x = 0; x < W; x++) {
      const p = scaled.peak[x] ?? 0;
      const r = scaled.rms[x] ?? 0;
      const hPeak = peakToHeight(p, halfH);
      const hRms = peakToHeight(r, halfH);
      if (hPeak === 0 && hRms === 0) continue;
      const fracX = (x + 0.5) / W;
      const elapsed = fracX <= elapsedFrac;
      // Dark-theme palette: gold for the elapsed region, quiet cream for
      // what remains. Must stay legible on both --surface and --bg.
      ctx.fillStyle = elapsed ? 'rgba(217,168,86,0.95)' : 'rgba(241,238,231,0.28)';
      ctx.fillRect(x, midY - hPeak, 1, hPeak);
      if (hRms > 0) {
        ctx.fillStyle = elapsed ? 'rgba(230,195,122,0.8)' : 'rgba(241,238,231,0.14)';
        ctx.fillRect(x, midY - hRms, 1, hRms);
      }
    }

    // Playhead overlay.
    const px = Math.round(elapsedFrac * W);
    ctx.fillStyle = 'rgba(230,195,122,0.95)';
    ctx.fillRect(px, 0, 1, H);
  }

  // Redraw whenever any input to draw() changes.
  $effect(() => {
    // dependencies
    bars;
    widthCss;
    heightCss;
    playheadFrac;
    queueRedraw();
  });

  function commitFraction(fraction: number): void {
    if (durationSeconds <= 0) return;
    const f = Math.max(0, Math.min(1, fraction));
    onSeek(startSeconds + f * durationSeconds);
  }

  let pointerCaptured = false;
  function fractionFromEvent(ev: PointerEvent): number {
    if (canvas == null) return 0;
    const rect = canvas.getBoundingClientRect();
    if (rect.width <= 0) return 0;
    return Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
  }

  function onPointerDown(ev: PointerEvent): void {
    if (disabled || durationSeconds <= 0 || canvas == null) return;
    canvas.setPointerCapture(ev.pointerId);
    pointerCaptured = true;
    commitFraction(fractionFromEvent(ev));
  }
  function onPointerMove(ev: PointerEvent): void {
    if (!pointerCaptured) return;
    commitFraction(fractionFromEvent(ev));
  }
  function onPointerUp(ev: PointerEvent): void {
    if (!pointerCaptured) return;
    pointerCaptured = false;
    if (canvas != null) canvas.releasePointerCapture(ev.pointerId);
  }
</script>

<div
  bind:this={container}
  class="waveform"
  class:disabled
  data-track-id={trackId}
>
  {#if fetchError}
    <span class="wf-err" aria-hidden="true">{fetchError}</span>
  {/if}
  <canvas
    bind:this={canvas}
    aria-hidden="true"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerUp}
  ></canvas>
  <!--
    Visually-hidden but focusable <input type="range"> sibling. Pointer
    transparent: the canvas owns click/drag seeking; this input is the
    keyboard surface (arrow / Home / End / PageUp / PageDown work natively)
    and shows the focus ring on the canvas via .waveform:focus-within.
    Its scale is the CURRENT TRACK, matching what is drawn — mapping it to
    the album would send clicks/keys to the wrong place once the queue
    holds more than one track.
  -->
  <input
    bind:this={hidden}
    type="range"
    class="wf-hidden"
    aria-label="Seek position"
    min="0"
    max={durationSeconds > 0 ? durationSeconds : 0}
    step="0.5"
    value={withinPos}
    disabled={disabled || durationSeconds <= 0}
    oninput={(e) => onSeek(startSeconds + Number((e.currentTarget as HTMLInputElement).value))}
  />
</div>

<style>
  .waveform {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 32px;
    display: block;
    touch-action: none;
  }
  .waveform canvas {
    display: block;
    width: 100%;
    height: 100%;
    cursor: pointer;
  }
  .waveform.disabled canvas { cursor: not-allowed; opacity: 0.5; }
  .wf-err {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.75em;
    color: var(--text-faint);
    pointer-events: none;
  }
  .wf-hidden {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    opacity: 0;
    margin: 0;
    /* Keyboard/a11y surface only; pointer events must reach the canvas. */
    pointer-events: none;
  }
  /* Visible focus ring around the canvas while the hidden range has focus. */
  .waveform:focus-within {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
</style>
