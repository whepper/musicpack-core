<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Static waveform sparkline for track tables and rail summaries. Fetches
  // lazily when scrolled into view (the shared LRU cache in
  // playback/waveform.ts dedupes with the seek control), draws once, and
  // stays out of the accessibility tree beyond a text alternative.
  import {
    downsampleToWidth,
    fetchWaveform,
    peakToHeight,
    type WaveformBars,
  } from '../playback/waveform';
  import type { Track } from '../api/types';

  let { track, height = 24 }: { track: Track; height?: number } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let wrap: HTMLDivElement | undefined = $state();
  let bars: WaveformBars | null = $state(null);
  let missing = $state(false);
  let widthCss = $state(0);
  let visible = $state(false);
  let seq = 0;

  const hasWaveform = $derived(Boolean(track.waveform));

  // Lazy-activate on visibility, and keep a ResizeObserver so the canvas
  // stays crisp when the table reflows.
  $effect(() => {
    const el = wrap;
    if (!el || !hasWaveform) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) visible = true;
      },
      { rootMargin: '200px' },
    );
    io.observe(el);
    const ro = new ResizeObserver(() => {
      const rect = el.getBoundingClientRect();
      widthCss = Math.max(0, Math.floor(rect.width));
      draw();
    });
    ro.observe(el);
    return () => {
      io.disconnect();
      ro.disconnect();
    };
  });

  $effect(() => {
    if (!visible || !hasWaveform) return;
    const id = track.id;
    const s = ++seq;
    fetchWaveform(fetch, '', () => undefined, id)
      .then((b) => {
        if (s !== seq) return;
        bars = b;
        missing = b === null;
        draw();
      })
      .catch(() => {
        if (s === seq) missing = true;
      });
  });

  let redrawQueued = false;
  function draw(): void {
    if (redrawQueued || canvas == null || bars == null) return;
    redrawQueued = true;
    requestAnimationFrame(() => {
      redrawQueued = false;
      paint();
    });
  }

  function paint(): void {
    if (canvas == null || bars == null) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const targetW = Math.max(1, Math.round(widthCss * dpr));
    const targetH = Math.max(1, Math.round(height * dpr));
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
    for (let x = 0; x < W; x++) {
      const hPeak = peakToHeight(scaled.peak[x] ?? 0, halfH);
      const hRms = peakToHeight(scaled.rms[x] ?? 0, halfH);
      if (hPeak === 0 && hRms === 0) continue;
      ctx.fillStyle = 'rgba(241,238,231,0.32)';
      ctx.fillRect(x, midY - hPeak, 1, hPeak);
      if (hRms > 0) {
        ctx.fillStyle = 'rgba(241,238,231,0.16)';
        ctx.fillRect(x, midY - hRms, 1, hRms);
      }
    }
  }
</script>

<div class="waveform-spark" bind:this={wrap} style="height:{height}px">
  {#if hasWaveform}
    <canvas bind:this={canvas} aria-hidden="true"></canvas>
    <span class="sr-only">Waveform available</span>
  {:else}
    <span class="spark-empty" aria-hidden="true">—</span>
    <span class="sr-only">No waveform</span>
  {/if}
  {#if missing}<span class="sr-only">(unavailable)</span>{/if}
</div>
