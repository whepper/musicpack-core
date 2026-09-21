<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album page, Tracks section: the technical track table.
  import TrackTable from '../TrackTable.svelte';
  import type { ReleaseDetail } from '../../api/types';

  let {
    rel,
    currentTrackId,
    onPlay,
  }: {
    rel: ReleaseDetail;
    currentTrackId: number | undefined;
    onPlay: (index: number) => void;
  } = $props();
</script>

<TrackTable release={rel} currentTrackId={currentTrackId} onPlay={onPlay} />
