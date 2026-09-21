<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album artwork is content, not decoration: meaningful alt text, graceful
  // placeholder with a deterministically derived monogram tile when the
  // artwork is missing or fails to load.
  let { src, alt, label }: { src?: string; alt: string; label?: string } = $props();
  let failed = $state(false);

  // Deep, desaturated placeholder tones: they read as quiet album art on
  // the dark canvas and keep the cream monogram at high contrast.
  const PALETTE = [
    '#3d3630', '#413a30', '#31393a', '#39323f', '#3f3038', '#2f3d38',
    '#3a3440', '#403a2c', '#2e3a40', '#423430', '#34402f', '#3d3040',
  ];

  const hue = $derived.by(() => {
    const s = label ?? alt;
    let h = 0;
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
    return PALETTE[h % PALETTE.length] ?? '#3d3630';
  });

  const initials = $derived.by(() => {
    const s = label ?? alt;
    const words = s.split(/\s+/).filter(Boolean).slice(0, 2);
    return words.map((w) => w[0]?.toUpperCase() ?? '').join('');
  });
</script>

{#if src && !failed}
  <img {src} {alt} loading="lazy" decoding="async" onerror={() => (failed = true)}>
{:else}
  <div class="artwork-fallback" style="background:{hue}" role="img" aria-label={alt}>
    <span style="font-size:1.6em;letter-spacing:.05em">{initials}</span>
  </div>
{/if}
