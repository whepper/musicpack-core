<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Full-screen artwork viewer (album page modal): Escape and backdrop
  // click close; the close button is a real, labelled control. Escape is
  // matched on KeyboardEvent.code, which is layout-independent and stable
  // for this key.
  import type { ArtworkRef } from '../api/types';

  let { title, artwork, onclose }: { title: string; artwork: ArtworkRef[]; onclose: () => void } =
    $props();

  let panel: HTMLDivElement | undefined = $state();

  // Modal a11y: move focus into the dialog when it opens so the keyboard
  // reaches its handler. (The pre-R2 inline viewer never focused itself,
  // so its Escape branch was unreachable in practice — R2 a11y baseline
  // fixes that; see docs/r2-migration-map.md D8.)
  $effect(() => {
    panel?.focus();
  });
</script>

<div
  bind:this={panel}
  class="viewer"
  role="dialog"
  aria-modal="true"
  aria-label="Artwork viewer"
  tabindex="-1"
  style="position:fixed;inset:0;background:rgba(8,9,11,0.94);z-index:40;display:flex;align-items:center;justify-content:center;flex-direction:column;gap:var(--space-4);padding:var(--space-5)"
  onclick={(event) => {
    if (event.target === event.currentTarget) onclose();
  }}
  onkeydown={(event) => {
    if (event.code === 'Escape') onclose();
  }}
>
  <button
    style="position:absolute;top:var(--space-4);right:var(--space-5);color:var(--text);font-size:var(--fs-xl)"
    aria-label="Close artwork viewer"
    onclick={onclose}>✕</button>
  {#each artwork as art, i (art.id)}
    <div style="max-width:min(720px, 90vw);text-align:center">
      <img src={art.url} alt={`${title} — ${art.role ?? 'artwork'} ${i + 1}`} style="max-width:100%;max-height:70vh;object-fit:contain;border-radius:4px">
      <p class="smallcaps" style="color:var(--text)">{art.role ?? 'artwork'}</p>
    </div>
  {/each}
</div>
