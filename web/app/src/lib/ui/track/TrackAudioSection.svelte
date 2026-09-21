<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page, Audio section: codec facts, primary hash and the
  // representation rows.
  import HashLine from '../HashLine.svelte';
  import InfoGrid from '../InfoGrid.svelte';
  import { codecName, formatBytes, fmtTime } from '../../format';
  import type { Track } from '../../api/types';
  import type { RepRow } from '../track-facts';

  let {
    t,
    repRows,
  }: {
    t: Track;
    repRows: RepRow[];
  } = $props();
</script>

<div class="album-section">
  <h2 class="smallcaps section-heading">Audio</h2>
  <InfoGrid
    rows={[
      ['Codec', codecName(t.codec.codec)],
      ['Stream version', t.codec.streamVersion ? `SV${t.codec.streamVersion}` : undefined],
      ['Sample rate', t.codec.sampleRate ? `${t.codec.sampleRate} Hz` : undefined],
      ['Channels', t.codec.channels],
      ['Duration', t.duration ? fmtTime(t.duration) : undefined],
      ['Primary file', t.audio.size ? formatBytes(t.audio.size) : undefined],
    ]}
  />
  {#if t.audio.sha256}
    <p class="section-note">
      Audio SHA-256: <HashLine value={t.audio.sha256} label="audio SHA-256" />
    </p>
  {/if}

  <h3 class="smallcaps section-heading">Representations</h3>
  <ul class="rep-list">
    {#each repRows as row (row.id)}
      <li>
        <span class="rep-track">{row.label}</span>
        <span class="rep-facts">
          {row.sub}
          {#if row.size}· {formatBytes(row.size)}{/if}
          {#if row.sha}· <HashLine value={row.sha} label={`${row.label} SHA-256`} />{/if}
        </span>
      </li>
    {/each}
  </ul>
</div>
