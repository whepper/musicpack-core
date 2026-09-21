<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Collection search: server-side album + artist search (the API has no
  // track search), grouped results, debounced and deep-linkable via
  // /search?q=… . The URL is the source of truth; typing replaces the
  // query so back-navigation skips intermediate terms.
  import { api, router } from '../bootstrap';
  import type { AlbumSummary, ArtistSummary } from '../api/types';
  import AlbumCard from './AlbumCard.svelte';
  import ErrorView from './ErrorView.svelte';
  import { onDestroy, onMount } from 'svelte';

  const routeStore = router.route;

  let input = $state(new URLSearchParams(location.search).get('q') ?? '');
  let albums = $state<AlbumSummary[] | null>(null);
  let artists = $state<ArtistSummary[] | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let field: HTMLInputElement | undefined = $state();
  let timer: ReturnType<typeof setTimeout> | undefined;
  onDestroy(() => clearTimeout(timer));

  onMount(() => {
    const initial = new URLSearchParams(location.search).get('q') ?? '';
    if (initial) void run(initial);
    // The search page's purpose is search: claim focus on arrival without
    // yanking the scroll position.
    field?.focus({ preventScroll: true });
  });

  // URL → state: a top-bar submit or a shared /search?q= link re-runs here.
  // `lastUrlQ` carries only the URL-confirmed term (never the mid-typing
  // `input`, which is not a dependency of this effect): reading `input`
  // here would make the effect depend on its own write (the shelf hit
  // this). It must still be `$state` — `term` derives from it, and
  // `$derived` over a plain `let` never recomputes.
  let lastUrlQ = $state<string>(new URLSearchParams(location.search).get('q') ?? '');
  $effect(() => {
    const urlQ = $routeStore.query.get('q') ?? '';
    if (urlQ === lastUrlQ) return;
    lastUrlQ = urlQ;
    input = urlQ;
    void run(urlQ);
  });

  async function run(q: string): Promise<void> {
    const term = q.trim();
    loading = true;
    error = null;
    if (!term) {
      albums = null;
      artists = null;
      loading = false;
      return;
    }
    try {
      const [albumPage, artistPage] = await Promise.all([
        api.albums({ q: term, limit: 50 }),
        api.artists({ q: term, limit: 50 }),
      ]);
      albums = albumPage.albums;
      artists = artistPage.artists;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Search failed.';
      albums = null;
      artists = null;
    } finally {
      loading = false;
    }
  }

  function onInput(event: Event): void {
    const value = (event.currentTarget as HTMLInputElement).value;
    input = value;
    clearTimeout(timer);
    timer = setTimeout(() => {
      const term = value.trim();
      router.replace(term ? `/search?q=${encodeURIComponent(term)}` : '/search');
    }, 180);
  }

  const hasResults = $derived(
    (albums !== null && albums.length > 0) || (artists !== null && artists.length > 0),
  );
  const term = $derived(lastUrlQ.trim());
</script>

<div class="search-page">
  <div class="shelf-header">
    <div class="shelf-title-wrap">
      <p class="eyebrow">The collection</p>
      <h1>Search</h1>
    </div>
  </div>

  <form class="search-field search-page-field" role="search" onsubmit={(e) => e.preventDefault()}>
    <span class="search-glyph" aria-hidden="true">⌕</span>
    <input
      bind:this={field}
      type="search"
      placeholder="Search albums and artists"
      aria-label="Search the collection"
      value={input}
      oninput={onInput}
    >
  </form>

  {#if loading}
    <div class="spinner" role="status" aria-label="Searching"></div>
  {:else if error}
    <ErrorView message={error} detail="The server may be unreachable — try again." />
  {:else if !term}
    <p class="empty-state">Type to search your collection by album or artist name.</p>
  {:else if !hasResults}
    <p class="empty-state">Nothing in the collection matches “{term}”.</p>
  {:else}
    {#if albums && albums.length > 0}
      <section aria-labelledby="search-albums-heading">
        <h2 id="search-albums-heading" class="smallcaps section-heading">
          {`${albums.length} album${albums.length === 1 ? '' : 's'}`}
        </h2>
        <div class="shelf-grid" role="list" aria-label="Album search results">
          {#each albums as album (album.id)}
            <div role="listitem">
              <AlbumCard {album} count={1} />
            </div>
          {/each}
        </div>
      </section>
    {/if}
    {#if artists && artists.length > 0}
      <section aria-labelledby="search-artists-heading">
        <h2 id="search-artists-heading" class="smallcaps section-heading">
          {`${artists.length} artist${artists.length === 1 ? '' : 's'}`}
        </h2>
        <ul class="artist-rows">
          {#each artists as artist (artist.id)}
            <li>
              <a class="artist-row" href={`/artists/${artist.id}`}>
                <span class="artist-name">{artist.name}</span>
                <span class="smallcaps">{artist.albumCount} album{artist.albumCount === 1 ? '' : 's'}</span>
              </a>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/if}
</div>
