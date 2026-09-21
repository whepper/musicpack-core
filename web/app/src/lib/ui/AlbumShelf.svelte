<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { library, router, session, offline, offlineStates } from '../bootstrap';
  import AlbumCard from './AlbumCard.svelte';
  import SearchBox from './SearchBox.svelte';
  import ErrorView from './ErrorView.svelte';
  import { onMount } from 'svelte';
  import type { AlbumSummary } from '../api/types';

  const shelf = library.shelf;
  const routeStore = router.route;
  const sessionState = $derived($session.state);
  let q = $state(new URLSearchParams(location.search).get('q') ?? '');
  let sort = $state('');
  let offlineOnly = $state(false);

  let sentinel: HTMLDivElement | undefined = $state();

  onMount(() => {
    void library.browse(q ? { q } : {});
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) void library.loadMore();
      },
      { rootMargin: '600px' },
    );
    if (sentinel) io.observe(sentinel);
    return () => io.disconnect();
  });

  // Top-bar searches deep-link to /albums?q=…; pick the query up
  // reactively so a search submitted while already on the shelf re-runs.
  // `lastUrlQ` is intentionally non-reactive: reading `q` here would make
  // the effect depend on its own write and reset the search box.
  let lastUrlQ: string = new URLSearchParams(location.search).get('q') ?? '';
  $effect(() => {
    const urlQ = $routeStore.query.get('q') ?? '';
    if (urlQ === lastUrlQ) return;
    lastUrlQ = urlQ;
    q = urlQ;
    void library.browse({ q, sort });
  });

  function onSearch(value: string): void {
    q = value;
    void library.browse({ q });
  }

  function onSort(value: string): void {
    sort = value;
    void library.browse({ q, sort });
  }

  // Album summaries for every installed release, rebuilt whenever the
  // offline states map changes (install/remove/audit). Sources are the
  // release snapshots stored in each committed package record.
  let installedAlbums = $state<AlbumSummary[]>([]);
  $effect(() => {
    // Track the reactive dependency explicitly.
    void $offlineStates;
    void offline.listPackages().then((pkgs) => {
      const seen = new Set<number>();
      const out: AlbumSummary[] = [];
      for (const pkg of pkgs) {
        if (pkg.status !== 'installed') continue;
        const d = pkg.releaseDetail;
        if (!d || seen.has(d.album.id)) continue;
        seen.add(d.album.id);
        out.push({
          id: d.album.id,
          title: d.album.title,
          releaseType: d.album.releaseType,
          originalReleaseDate: d.album.originalReleaseDate,
          artists: d.album.artists,
          releaseCount: 1,
          artwork: d.artwork.find((a) => a.role === 'front') ?? d.artwork[0],
        });
      }
      installedAlbums = out;
    });
  });

  const installedAlbumIds = $derived(new Set(installedAlbums.map((a) => a.id)));

  // Client-side intersection with the installed set: the server API has no
  // "offline" parameter and needs none — filtering a page of 50 is instant.
  const visibleAlbums = $derived(
    offlineOnly ? $shelf.albums.filter((a) => installedAlbumIds.has(a.id)) : $shelf.albums,
  );
</script>

<div class="shelf-header">
  <div class="shelf-title-wrap">
    <p class="eyebrow">The collection</p>
    <h1 id="shelf-title">The shelf</h1>
  </div>
  {#if sessionState === 'offline'}
    <span class="smallcaps" role="status" style="color:var(--accent)">Offline — showing downloaded music</span>
  {/if}
  <span class="shelf-count smallcaps">{`${$shelf.total} album${$shelf.total === 1 ? '' : 's'}`}</span>
</div>

<SearchBox value={q} onSearch={onSearch} onSort={onSort} sort={sort}>
  {#if offline.enabled}
    <button
      class="edition-chip"
      aria-pressed={offlineOnly}
      onclick={() => (offlineOnly = !offlineOnly)}>Available offline</button>
  {/if}
</SearchBox>

{#if sessionState === 'offline' && offline.enabled && !$shelf.loading}
  <!-- Offline boot with installed content: shelf fetches fail by design.
       Render the downloaded albums from their stored release snapshots. -->
  <div class="shelf-grid" role="list" aria-labelledby="shelf-title">
    {#each installedAlbums as album (album.id)}
      <div role="listitem">
        <AlbumCard {album} count={1} offline />
      </div>
    {/each}
  </div>
{:else if $shelf.error}
  <ErrorView message={$shelf.error} />
{:else if visibleAlbums.length === 0 && offlineOnly}
  <p class="empty-state">No downloads yet — open an album and choose “Download” to keep it available offline.</p>
{:else if $shelf.albums.length === 0 && $shelf.loading}
  <div class="spinner" role="status" aria-label="Loading the shelf"></div>
{:else if $shelf.albums.length === 0}
  <p class="empty-state">No albums in the collection yet — scan a library to begin.</p>
{:else}
  <div class="shelf-grid" bind:this={sentinel} role="list" aria-labelledby="shelf-title">
    {#each visibleAlbums as album (album.id)}
      <div role="listitem">
        <AlbumCard {album} count={1} offline={installedAlbumIds.has(album.id)} />
      </div>
    {/each}
  </div>
  {#if $shelf.loading}
    <div class="spinner" role="status" aria-label="Loading more albums"></div>
  {/if}
{/if}
