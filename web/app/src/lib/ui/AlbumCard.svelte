<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import type { AlbumSummary } from '../api/types';
  import Artwork from './Artwork.svelte';
  import { collectorLine, yearOf } from '../format';

  let { album, count, offline = false }: { album: AlbumSummary; count: number; offline?: boolean } = $props();
  const artist = $derived(album.artists[0]?.name ?? '');
  const title = $derived(album.title);
</script>

<a class="album-card" href={`/albums/${album.id}`}>
  <div class="cover">
    <Artwork src={album.artwork?.url} alt={`${title} by ${artist}`} label={title} />
    {#if offline}
      <span class="offline-badge" title="Available offline" aria-label={`${title} is available offline`}>⤷</span>
    {/if}
  </div>
  <div class="title">{title}</div>
  <div class="artist">{artist}</div>
  {#if count > 0}
    <div class="collector">
      {collectorLine({ year: yearOf(album.originalReleaseDate), releaseCount: album.releaseCount })}
    </div>
  {/if}
  {#if album.genres?.length}
    <div class="genre-pills">
      {#each album.genres.slice(0, 3) as genre}
        <span class="genre-pill">{genre}</span>
      {/each}
      {#if album.genres.length > 3}
        <span class="genre-pill">+{album.genres.length - 3}</span>
      {/if}
    </div>
  {/if}
</a>
