<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album page, Metadata section: credits, identifiers, source facts, notes.
  import HashLine from '../HashLine.svelte';
  import InfoGrid from '../InfoGrid.svelte';
  import type { AlbumDetail, ReleaseDetail } from '../../api/types';
  import type { ReleaseFacts } from '../album-facts';

  let {
    detail,
    rel,
    facts,
  }: {
    detail: AlbumDetail;
    rel: ReleaseDetail;
    facts: ReleaseFacts;
  } = $props();
</script>

<h3 class="smallcaps section-heading">Credits</h3>
<ul class="id-list">
  {#each detail.album.artists as a (a.id + a.name)}
    <li><span class="id-what">{a.name}</span>{#if a.role}<span class="muted"> — {a.role}</span>{/if}</li>
  {/each}
</ul>
<h3 class="smallcaps section-heading">Identifiers</h3>
<ul class="id-list">
  {#if detail.album.mbid}
    <li><span class="smallcaps id-key">Release group</span><HashLine value={detail.album.mbid} label="MusicBrainz release group ID" /></li>
  {/if}
  {#if rel.mbid}
    <li><span class="smallcaps id-key">Release</span><HashLine value={rel.mbid} label="MusicBrainz release ID" /></li>
  {/if}
  {#each facts.trackIsrcs as entry (entry.title + entry.isrc)}
    <li><span class="smallcaps id-key">ISRC · {entry.title}</span><span class="mono">{entry.isrc}</span></li>
  {/each}
</ul>
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
{#if rel.notes}
  <h3 class="smallcaps section-heading">Notes</h3>
  <p class="muted">{rel.notes}</p>
{/if}
