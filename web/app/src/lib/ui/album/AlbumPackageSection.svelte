<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album page, Package section: status chips, package facts, asset
  // inventory, and the verification note.
  import HashLine from '../HashLine.svelte';
  import InfoGrid from '../InfoGrid.svelte';
  import StatusChip from '../StatusChip.svelte';
  import { packageStatusChip, verifyStatusChip, type Chip } from '../status';
  import type { ReleaseDetail } from '../../api/types';
  import type { ReleaseFacts } from '../album-facts';

  let {
    rel,
    facts,
    offlineChip,
  }: {
    rel: ReleaseDetail;
    facts: ReleaseFacts;
    offlineChip: Chip | null;
  } = $props();
</script>

<div class="status-row" style="margin-bottom:var(--space-4)">
  <StatusChip chip={packageStatusChip(rel.packageStatus)} />
  <StatusChip chip={verifyStatusChip(rel.verifyStatus)} />
  {#if offlineChip}<StatusChip chip={offlineChip} />{/if}
</div>
<InfoGrid
  rows={[
    ['Package status', rel.packageStatus],
    ['Verification', rel.verifyStatus],
    ['Structure', `${facts.discCount} ${facts.discCount === 1 ? 'disc' : 'discs'} · ${facts.trackCount} tracks`],
    ['Audio files', `${facts.trackCount} primary${facts.repCount > 0 ? ` · ${facts.repCount} alternate${facts.repCount === 1 ? '' : 's'}` : ''}`],
    ['Waveform envelopes', facts.waveCount > 0 ? `${facts.waveCount} of ${facts.trackCount} tracks` : undefined],
    ['Total audio', facts.totalSize],
    ['Artwork', `${rel.artwork.length} image${rel.artwork.length === 1 ? '' : 's'}`],
    ['Authored with', [rel.provenanceTool, rel.provenanceToolVersion].filter(Boolean).join(' ') || undefined],
  ]}
/>
<h3 class="smallcaps section-heading">Assets</h3>
<ul class="asset-list">
  {#each rel.artwork as art (art.id)}
    <li>
      <span class="id-what">Artwork{art.role ? ` · ${art.role}` : ''}</span>
      <span class="muted">{art.mimeType}</span>
      {#if art.sha256}<HashLine value={art.sha256} label="Artwork SHA-256" />{/if}
    </li>
  {/each}
  {#each rel.assets as asset (asset.id)}
    <li>
      <span class="id-what">{asset.kind}{asset.role ? ` · ${asset.role}` : ''}</span>
      <span class="muted">{asset.mimeType}</span>
      {#if asset.sha256}<HashLine value={asset.sha256} label="Asset SHA-256" />{/if}
    </li>
  {/each}
</ul>
<p class="muted section-note">
  The server verifies every referenced file's SHA-256 before this album is served; the offline
  download re-verifies hashes on this device during install.
</p>
