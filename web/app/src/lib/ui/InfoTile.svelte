<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Hero stat tile (codec / track-count summaries). Renders as a link when
  // `href` is given — tiles deep-link into their album section.
  let {
    title,
    sub,
    href,
  }: {
    title: string;
    sub?: string;
    href?: string;
  } = $props();
</script>

{#if href}
  <a class="info-tile" {href}>
    <span class="tile-title">{title}</span>
    {#if sub}<span class="tile-sub">{sub}</span>{/if}
  </a>
{:else}
  <div class="info-tile">
    <span class="tile-title">{title}</span>
    {#if sub}<span class="tile-sub">{sub}</span>{/if}
  </div>
{/if}
