<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Rail summary: edition identity, facts, and MusicBrainz links. Every
  // value comes straight from the release/album API objects.
  import type { AlbumDetail, ReleaseDetail } from '../api/types';
  import Artwork from './Artwork.svelte';
  import HashLine from './HashLine.svelte';
  import { countryName, mediumLabel, yearOf } from '../format';

  let {
    detail,
    rel,
    onSelect,
    onSection,
  }: {
    detail: AlbumDetail;
    rel: ReleaseDetail;
    onSelect: (releaseId: number) => void;
    onSection: (id: string) => void;
  } = $props();

  const formats = $derived(
    rel.media.map((m) => mediumLabel(m.format)).filter((v, i, a) => a.indexOf(v) === i).join(', '),
  );
</script>

<section class="rail-panel">
  <header>
    <h3 class="smallcaps">Edition</h3>
    <button class="rail-link" aria-label="Open the full edition section" onclick={() => onSection('edition')}>›</button>
  </header>

  {#if detail.releases.length > 1}
    <div class="edition-thumbs">
      {#each detail.releases as r (r.id)}
        <button
          class="edition-thumb"
          class:selected={r.id === rel.id}
          aria-pressed={r.id === rel.id}
          aria-label={`Edition: ${r.edition ?? `${mediumLabel(r.media[0])} ${yearOf(r.releaseDate)}`}`}
          title={r.edition ?? `Release ${r.id}`}
          onclick={() => onSelect(r.id)}
        >
          <Artwork src={r.artwork?.url} alt="" label={detail.album.title} />
        </button>
      {/each}
    </div>
  {/if}

  <dl class="rail-kv">
    {#if rel.releaseDate}<div><dt>Release</dt><dd>{yearOf(rel.releaseDate)}</dd></div>{/if}
    {#if rel.country}<div><dt>Country</dt><dd>{countryName(rel.country)}</dd></div>{/if}
    {#if rel.label}<div><dt>Label</dt><dd>{rel.label}</dd></div>{/if}
    {#if rel.catalogueNumber}<div><dt>Catalog No.</dt><dd>{rel.catalogueNumber}</dd></div>{/if}
    {#if formats}<div><dt>Format</dt><dd>{formats}</dd></div>{/if}
    {#if rel.barcode}<div><dt>Barcode</dt><dd>{rel.barcode}</dd></div>{/if}
    {#if detail.album.mbid}
      <div><dt>Release group</dt><dd><HashLine value={detail.album.mbid} label="MusicBrainz release group ID" /></dd></div>
    {/if}
    {#if rel.mbid}
      <div>
        <dt>MusicBrainz</dt>
        <dd>
          <a class="rail-ext" href={`https://musicbrainz.org/release/${rel.mbid}`} target="_blank" rel="noopener noreferrer"
            aria-label="Open this release on MusicBrainz (opens a new tab)">Release ↗</a>
        </dd>
      </div>
    {/if}
  </dl>
</section>
