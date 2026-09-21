<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Rail summary: package integrity at a glance — real server status
  // values plus the client-side offline lifecycle, structure counts, and
  // authoring provenance. Deep-links into the full Package section.
  import type { ReleaseDetail } from '../api/types';
  import StatusChip from './StatusChip.svelte';
  import { offlineStateChip, packageStatusChip, verifyStatusChip } from './status';
  import { formatBytes } from '../format';

  let {
    rel,
    offlineState,
    onSection,
  }: {
    rel: ReleaseDetail;
    offlineState?: string;
    onSection: (id: string) => void;
  } = $props();

  const trackCount = $derived((rel.media ?? []).reduce((n, m) => n + m.tracks.length, 0));
  const repCount = $derived(
    (rel.media ?? []).reduce((n, m) => n + m.tracks.reduce((s, t) => s + (t.representations?.length ?? 0), 0), 0),
  );
  const waveCount = $derived((rel.media ?? []).reduce((n, m) => n + m.tracks.filter((t) => t.waveform).length, 0));
  const totalSize = $derived(
    formatBytes((rel.media ?? []).reduce(
      (sum, disc) =>
        sum +
        disc.tracks.reduce(
          (s, t) => s + t.audio.size + (t.representations ?? []).reduce((r, rep) => r + rep.size, 0),
          0,
        ),
      0,
    )),
  );
  const offline = $derived(offlineStateChip(offlineState));
  const provenance = $derived(
    [rel.provenanceTool, rel.provenanceToolVersion].filter(Boolean).join(' ') || undefined,
  );
</script>

<section class="rail-panel">
  <header>
    <h3 class="smallcaps">Package</h3>
    <button class="rail-link" aria-label="Open the full package section" onclick={() => onSection('package')}>›</button>
  </header>

  <div class="status-row">
    <StatusChip chip={verifyStatusChip(rel.verifyStatus)} />
    <StatusChip chip={packageStatusChip(rel.packageStatus)} />
    {#if offline}<StatusChip chip={offline} />{/if}
  </div>

  <dl class="rail-kv">
    <div><dt>Tracks</dt><dd>{trackCount} across {rel.media.length} {rel.media.length === 1 ? 'disc' : 'discs'}</dd></div>
    {#if totalSize}<div><dt>Audio</dt><dd>{totalSize}</dd></div>{/if}
    {#if repCount > 0}<div><dt>Alternate files</dt><dd>{repCount}</dd></div>{/if}
    {#if waveCount > 0}<div><dt>Waveforms</dt><dd>{waveCount}</dd></div>{/if}
    {#if provenance}<div><dt>Authored with</dt><dd>{provenance}</dd></div>{/if}
  </dl>
</section>
