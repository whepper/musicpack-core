<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album page, Edition section: the edition selector plus the edition
  // fact grid. Pure presentation over API payloads.
  import EditionSelector from '../EditionSelector.svelte';
  import InfoGrid from '../InfoGrid.svelte';
  import { countryName, formatDate, mediumLabel } from '../../format';
  import type { AlbumDetail, ReleaseDetail } from '../../api/types';
  import type { ReleaseFacts } from '../album-facts';

  let {
    detail,
    rel,
    facts,
    selectedId,
    onSelect,
  }: {
    detail: AlbumDetail;
    rel: ReleaseDetail;
    facts: ReleaseFacts;
    selectedId: number;
    onSelect: (id: number) => void;
  } = $props();
</script>

<EditionSelector releases={detail.releases} selectedId={selectedId} onSelect={onSelect} />
<InfoGrid
  rows={[
    ['Edition', rel.edition],
    ['Release date', formatDate(rel.releaseDate)],
    ['Original release', formatDate(rel.album.originalReleaseDate)],
    ['Country', countryName(rel.country)],
    ['Label', rel.label],
    ['Catalogue number', rel.catalogueNumber],
    ['Barcode', rel.barcode],
    ['Medium', rel.media.map((m) => `Disc ${m.disc}${m.format ? ` (${mediumLabel(m.format)})` : ''}`).join(', ')],
    ['MusicBrainz release group', detail.album.mbid],
    ['MusicBrainz release', rel.mbid],
    ['Editions in collection', detail.releases.length],
  ]}
/>
