<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import type { Snippet } from 'svelte';

  // Shared collection-search header. The sort group is optional: callers
  // without sorting (e.g. the artist list) render just the search pill,
  // so no dead "All / Recently added" chips appear.
  let {
    value,
    onSearch,
    onSort,
    sort,
    children,
  }: {
    value: string;
    onSearch: (q: string) => void;
    onSort?: (sort: string) => void;
    sort?: string;
    children?: Snippet;
  } = $props();
</script>

<div class="search-row">
  <label class="search-field">
    <span class="search-glyph" aria-hidden="true">⌕</span>
    <input
      type="search"
      placeholder="Search title or artist"
      value={value}
      aria-label="Search the collection"
      oninput={(e) => onSearch((e.currentTarget as HTMLInputElement).value)}
    >
  </label>
  {#if onSort || children}
    <div class="sort" role="group" aria-label="Filter and sort the shelf">
      {@render children?.()}
      {#if onSort}
        <button
          class="edition-chip"
          aria-pressed={sort === ''}
          onclick={() => onSort('')}>All</button>
        <button
          class="edition-chip"
          aria-pressed={sort === 'recent'}
          onclick={() => onSort('recent')}>Recently added</button>
      {/if}
    </div>
  {/if}
</div>
