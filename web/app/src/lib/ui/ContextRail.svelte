<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // The contextual right rail: glanceable summaries that deep-link into
  // their full album sections. Rendered next to the album main column on
  // wide viewports; stacks below it on narrow ones.
  import type { AlbumDetail, ReleaseDetail, Track } from '../api/types';
  import type { AudioPreference } from '../state/representation-selection';
  import AnalysisPanel from './AnalysisPanel.svelte';
  import EditionPanel from './EditionPanel.svelte';
  import PackagePanel from './PackagePanel.svelte';
  import RepresentationsPanel from './RepresentationsPanel.svelte';

  let {
    detail,
    rel,
    waveTrack,
    offlineState,
    onSelectEdition,
    onPlay,
    onSection,
  }: {
    detail: AlbumDetail;
    rel: ReleaseDetail;
    waveTrack: Track | null;
    offlineState?: string;
    onSelectEdition: (releaseId: number) => void;
    onPlay: (preference?: AudioPreference) => void;
    onSection: (id: string) => void;
  } = $props();
</script>

<aside class="album-rail" aria-label="Release details">
  <EditionPanel {detail} {rel} onSelect={onSelectEdition} {onSection} />
  <RepresentationsPanel {rel} {onPlay} {onSection} />
  <AnalysisPanel {rel} {waveTrack} {onSection} />
  <PackagePanel {rel} {offlineState} {onSection} />
</aside>
