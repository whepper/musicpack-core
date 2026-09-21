<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { library } from '../bootstrap';
  import AlbumCard from './AlbumCard.svelte';
  import ErrorView from './ErrorView.svelte';
  import { onMount } from 'svelte';
  import type { AlbumSummary } from '../api/types';

  let { artistId }: { artistId: string } = $props();

  let detail = $state<Awaited<ReturnType<typeof library.artistDetail>> | null>(null);
  let error = $state<string | null>(null);

  onMount(() => {
    library
      .artistDetail(artistId)
      .then((d) => (detail = d))
      .catch((e) => (error = e instanceof Error ? e.message : 'Could not load artist.'));
  });

  // Artist-detail albums carry the same front-artwork reference the shelf
  // emits (earliest visible release's front asset), so cards show real
  // covers; Artwork's monogram tile remains the fallback for artless
  // albums.
  const cards = $derived.by(() => {
    if (!detail) return [] as Array<AlbumSummary>;
    const d = detail;
    return detail.albums.map((album) => ({
      id: album.id,
      title: album.title,
      releaseType: album.releaseType,
      originalReleaseDate: album.originalReleaseDate,
      artists: [{ id: d.id, name: d.name }],
      releaseCount: 0,
      artwork: album.artwork,
    }));
  });
</script>

{#if error}
  <ErrorView message={error} />
{:else if !detail}
  <div class="spinner" role="status" aria-label="Loading artist"></div>
{:else}
  <div class="shelf-header">
    <div class="shelf-title-wrap">
      <p class="eyebrow">Artist</p>
      <h1>{detail.name}</h1>
    </div>
  </div>
  <h2 class="smallcaps" style="margin:var(--space-4) 0">Albums</h2>
  <div class="shelf-grid">
    {#each cards as album (album.id)}
      <div>
        <AlbumCard {album} count={1} />
      </div>
    {/each}
  </div>
{/if}
