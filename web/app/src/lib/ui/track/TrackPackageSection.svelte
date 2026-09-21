<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page, Package section: status chips, package facts and the
  // deep link to the album's full Package inspector.
  import InfoGrid from '../InfoGrid.svelte';
  import StatusChip from '../StatusChip.svelte';
  import { packageStatusChip, verifyStatusChip, type Chip } from '../status';
  import type { ReleaseDetail } from '../../api/types';
  import type { TrackFacts } from '../track-facts';

  let {
    rel,
    facts,
    offlineChip,
    packageHref,
    onNav,
  }: {
    rel: ReleaseDetail;
    facts: TrackFacts;
    offlineChip: Chip | null;
    /** Deep link to the owning album's Package section (null pre-context). */
    packageHref: string | null;
    onNav: (href: string) => void;
  } = $props();
</script>

<div class="album-section">
  <h2 class="smallcaps section-heading">Package</h2>
  <p class="status-row">
    <StatusChip chip={packageStatusChip(rel.packageStatus)} />
    <StatusChip chip={verifyStatusChip(rel.verifyStatus)} />
    {#if offlineChip}<StatusChip chip={offlineChip} />{/if}
  </p>
  <InfoGrid
    rows={[
      ['Structure', `${facts.allTracks.length} ${rel.media.length === 1 ? 'disc' : 'discs'} · ${facts.trackCount} tracks`],
      ['Audio files', `${facts.trackCount} primary${facts.repCount > 0 ? ` · ${facts.repCount} alternate${facts.repCount === 1 ? '' : 's'}` : ''}`],
      ['Waveforms', facts.waveCount > 0 ? `${facts.waveCount} of ${facts.trackCount} tracks` : undefined],
      ['Authored with', [rel.provenanceTool, rel.provenanceToolVersion].filter(Boolean).join(' ') || undefined],
    ]}
  />
  <p class="muted section-note">
    The server verifies every referenced file's SHA-256 before this album is served; the
    offline download re-verifies hashes on this device during install. See the
    <button class="inline-link" onclick={() => packageHref && onNav(packageHref)}>full Package section</button>
    for the complete asset inventory.
  </p>
</div>
