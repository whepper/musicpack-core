<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Lyrics panel (R3.4) — pure presentation of the lyrics controller's
  // view (docs/musicpack-lyrics-v1.md §10/§12). All timing, document
  // structure and selection rules live in the controller/core; this
  // component owns only presentation: the scroll container, the
  // auto-follow suspension and the resume-follow affordance.
  import type { LyricsView } from '../../playback/lyrics';

  let { view, onRetry }: { view: LyricsView; onRetry?: () => void } = $props();

  // ---- follow state (presentation-local) --------------------------------
  let viewport = $state<HTMLElement | null>(null);
  /** Auto-follow is ON unless the user scrolled manually (§12). */
  let following = $state(true);

  function userScrollIntent(): void {
    if (view.state === 'synced') following = false;
  }

  /** A track/document change re-arms auto-follow (§12); seeking does not
   *  touch the flag ("follow resumes if it was on"). */
  $effect(() => {
    void view.lines;
    void view.state;
    following = true;
  });

  /** Scroll the active line into view when following (§12): only when the
   *  active line changes, honoring prefers-reduced-motion. */
  $effect(() => {
    if (!following || view.state !== 'synced' || view.activeIndex < 0) return;
    const box = viewport;
    if (!box) return;
    const el = box.querySelector<HTMLElement>('.lyric-line.active');
    if (!el) return;
    const reduced =
      typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    el.scrollIntoView({ behavior: reduced ? 'auto' : 'smooth', block: 'center' });
  });
</script>

<div class="lyrics-panel" class:synced={view.state === 'synced'}>
  {#if view.state === 'loading'}
    <div class="lyrics-loading" role="status" aria-label="Loading lyrics">
      <span class="spinner" aria-hidden="true"></span>
    </div>
  {:else if view.state === 'error'}
    <p class="lyrics-note" role="status">
      Lyrics could not be loaded{view.message ? ` — ${view.message}` : ''}.
      {#if onRetry}
        <button class="btn ghost lyrics-retry" onclick={onRetry}>Retry</button>
      {/if}
    </p>
  {:else if view.lines.length === 0}
    <p class="lyrics-note" role="status">This lyric document contains no lines.</p>
  {:else}
    <div
      class="lyrics-scroll"
      class:follows={view.state === 'synced' && following}
      bind:this={viewport}
      tabindex="0"
      role="region"
      aria-label="Lyrics"
      onwheel={userScrollIntent}
      ontouchstart={userScrollIntent}
      onpointerdown={userScrollIntent}
      onkeydown={userScrollIntent}
    >      <ol class="lyric-lines">
        {#each view.lines as line, i (i)}
          <li
            class="lyric-line"
            class:active={view.activeIndex === i}
            class:past={view.activeIndex >= 0 && i < view.activeIndex}
            aria-current={view.state === 'synced' && view.activeIndex === i ? 'step' : undefined}
          >{#if line.text.length > 0}{line.text}{:else}&nbsp;{/if}</li>
        {/each}
      </ol>
    </div>
    {#if view.state === 'synced' && !following}
      <button
        class="btn ghost lyrics-resume"
        onclick={() => (following = true)}
      >↓ Return to current line</button>
    {/if}
  {/if}
  {#if view.lang}
    <p class="lyrics-lang smallcaps">{view.lang}</p>
  {/if}
</div>
