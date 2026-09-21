<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page, Metadata section: release facts, identifiers, credits and
  // source provenance.
  import InfoGrid from '../InfoGrid.svelte';
  import { countryName } from '../../format';
  import type { ReleaseDetail, Track } from '../../api/types';
  import type { TrackFacts } from '../track-facts';

  let {
    t,
    rel,
    facts,
  }: {
    t: Track;
    rel: ReleaseDetail;
    facts: TrackFacts;
  } = $props();
</script>

<div class="album-section">
  <h2 class="smallcaps section-heading">Metadata</h2>
  <InfoGrid
    rows={[
      ['Album', rel.album.title],
      ['Edition', rel.edition],
      ['Release date', rel.releaseDate ? `${rel.releaseDate.slice(0, 4)}` : undefined],
      ['Country', countryName(rel.country)],
      ['Label', rel.label],
      ['Catalogue number', rel.catalogueNumber],
      ['Release group', rel.album.mbid],
      ['Release', rel.mbid],
    ]}
  />
  {#if facts.identifiers.length > 0}
    <InfoGrid rows={facts.identifiers} />
  {/if}
  {#if t.artists.length > 1}
    <h3 class="smallcaps section-heading">Credits</h3>
    <ul class="id-list">
      {#each t.artists as a (a.id + a.name)}
        <li><span class="id-what">{a.name}</span>{#if a.role}<span class="muted"> — {a.role}</span>{/if}</li>
      {/each}
    </ul>
  {/if}
  <h3 class="smallcaps section-heading">Source</h3>
  <InfoGrid
    rows={[
      ['Source type', rel.sourceType],
      ['Source store', rel.sourceStore],
      ['Source id', rel.sourceId],
      ['Identity source', rel.identitySource],
      ['Identity confidence', rel.identityConfidence],
    ]}
  />
</div>
