<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { api, router } from '../bootstrap';
  import ErrorView from './ErrorView.svelte';
  import SearchBox from './SearchBox.svelte';
  import { onMount } from 'svelte';

  let artists = $state<Array<{ id: number; name: string; albumCount: number }>>([]);
  let total = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let q = $state('');
  let timer: ReturnType<typeof setTimeout> | undefined;

  onMount(() => {
    void load(0);
    return () => clearTimeout(timer);
  });

  async function load(offset: number, query = q): Promise<void> {
    try {
      const page = await api.artists({ limit: 200, offset, q: query });
      artists = offset === 0 ? page.artists : [...artists, ...page.artists];
      total = page.total;
      error = null;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not load artists.';
    } finally {
      loading = false;
    }
  }

  function onSearch(value: string): void {
    q = value;
    clearTimeout(timer);
    timer = setTimeout(() => void load(0, value), 180);
  }
</script>

<div class="shelf-header">
  <div class="shelf-title-wrap">
    <p class="eyebrow">The collection</p>
    <h1 id="artist-title">Artists</h1>
  </div>
  <span class="shelf-count smallcaps">{`${total} artist${total === 1 ? '' : 's'}`}</span>
</div>

<SearchBox value={q} onSearch={onSearch} />

{#if error}
  <ErrorView message={error} />
{:else if loading && artists.length === 0}
  <div class="spinner" role="status" aria-label="Loading artists"></div>
{:else if artists.length === 0}
  <p class="empty-state">No artists in the collection yet.</p>
{:else}
  <ul class="artist-rows" role="list" aria-labelledby="artist-title">
    {#each artists as artist (artist.id)}
      <li>
        <a class="artist-row" href={`/artists/${artist.id}`}>
          <span class="artist-name">{artist.name}</span>
          <span class="smallcaps">{artist.albumCount} album{artist.albumCount === 1 ? '' : 's'}</span>
        </a>
      </li>
    {/each}
  </ul>
{/if}
