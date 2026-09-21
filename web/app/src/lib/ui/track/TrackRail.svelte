<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page right rail: album facts, per-representation play cards,
  // analysis peek and the package panel. Pure presentation; playback and
  // navigation arrive as callbacks.
  import StatusChip from '../StatusChip.svelte';
  import WaveformSpark from '../WaveformSpark.svelte';
  import { formatBytes, yearOf } from '../../format';
  import {
    packageStatusChip,
    verifyStatusChip,
    type Chip,
  } from '../status';
  import type { ReleaseDetail, Track } from '../../api/types';
  import type { RepRow, SeekTimeline, TrackFacts } from '../track-facts';
  import type { AudioPreference } from '../../state/representation-selection';

  let {
    t,
    rel,
    facts,
    repRows,
    albumUrl,
    audioHref,
    analysisHref,
    editionHref,
    metadataHref,
    packageHref,
    offlineChip,
    onNav,
    onPlayRepresentation,
  }: {
    t: Track;
    rel: ReleaseDetail;
    facts: TrackFacts;
    repRows: RepRow[];
    albumUrl: string;
    audioHref: string | null;
    analysisHref: string | null;
    editionHref: string | null;
    metadataHref: string | null;
    packageHref: string | null;
    offlineChip: Chip | null;
    onNav: (href: string) => void;
    onPlayRepresentation: (preference?: AudioPreference) => void;
  } = $props();
</script>

<aside class="album-rail" aria-label="Track inspector">
  <section class="rail-panel">
    <header><h3 class="smallcaps">Album</h3>
      <button class="rail-link" onclick={() => onNav(albumUrl)}>›</button>
    </header>
    <dl class="rail-kv">
      <div><dt>Title</dt><dd>{rel.album.title}</dd></div>
      <div><dt>Edition</dt><dd>{rel.edition ?? '—'}</dd></div>
      <div><dt>Tracks</dt><dd>{facts.trackCount} across {rel.media.length} {rel.media.length === 1 ? 'disc' : 'discs'}</dd></div>
      <div><dt>Year</dt><dd>{yearOf(rel.releaseDate ?? rel.album.originalReleaseDate)}</dd></div>
    </dl>
  </section>

  <section class="rail-panel">
    <header><h3 class="smallcaps">Audio</h3>
      <button class="rail-link" onclick={() => audioHref && onNav(audioHref)}>›</button>
    </header>
    <div class="rep-card">
      <div class="rep-body">
        <p class="rep-title">{repRows[0]?.label ?? '—'}
          {#if repRows[0]?.size}<span class="rep-size"> · {formatBytes(repRows[0].size)}</span>{/if}
        </p>
        {#if repRows[0]?.sub}<p class="rep-sub">{repRows[0].sub}</p>{/if}
      </div>
      <button class="btn-small" onclick={() => onPlayRepresentation()}>Play</button>
    </div>
    {#each repRows.slice(1) as row (row.id)}
      <div class="rep-card">
        <div class="rep-body">
          <p class="rep-title">{row.label}{#if row.size}<span class="rep-size"> · {formatBytes(row.size)}</span>{/if}</p>
          {#if row.sub}<p class="rep-sub">{row.sub}</p>{/if}
        </div>
        <button
          class="btn-small"
          aria-label={`Play track in ${row.label}`}
          onclick={() => onPlayRepresentation({ mode: 'codec', codec: row.label.toLowerCase() })}
        >Play</button>
      </div>
    {/each}
  </section>

  <section class="rail-panel">
    <header><h3 class="smallcaps">Analysis</h3>
      <button class="rail-link" onclick={() => analysisHref && onNav(analysisHref)}>›</button>
    </header>
    {#if t.loudness}
      <dl class="rail-kv">
        <div><dt>Integrated</dt><dd>{t.loudness.lufs.toFixed(1)} LUFS</dd></div>
        <div><dt>True peak</dt><dd>{t.loudness.truePeakDb.toFixed(1)} dBTP</dd></div>
      </dl>
    {/if}
    {#if rel.loudness}
      <dl class="rail-kv" style="margin-top:var(--space-2)">
        <div><dt>Album</dt><dd>{rel.loudness.albumLufs.toFixed(1)} LUFS</dd></div>
        <div><dt>Album peak</dt><dd>{rel.loudness.albumTruePeakDb.toFixed(1)} dBTP</dd></div>
      </dl>
    {/if}
    {#if t.waveform}
      <div class="analysis-mini">
        <p class="smallcaps rail-label" style="margin-top:var(--space-3)">Waveform</p>
        <WaveformSpark track={t} height={44} />
      </div>
    {/if}
  </section>

  <section class="rail-panel">
    <header><h3 class="smallcaps">Package</h3>
      <button class="rail-link" onclick={() => packageHref && onNav(packageHref)}>›</button>
    </header>
    <div class="status-row" style="margin-bottom:var(--space-3)">
      <StatusChip chip={verifyStatusChip(rel.verifyStatus)} />
      <StatusChip chip={packageStatusChip(rel.packageStatus)} />
      {#if offlineChip}<StatusChip chip={offlineChip} />{/if}
    </div>
    <dl class="rail-kv">
      <div><dt>Authored with</dt><dd>{[rel.provenanceTool, rel.provenanceToolVersion].filter(Boolean).join(' ') || '—'}</dd></div>
      <div><dt>Waveforms</dt><dd>{facts.waveCount > 0 ? `${facts.waveCount} / ${facts.trackCount}` : '—'}</dd></div>
      <div><dt>Alt. files</dt><dd>{facts.repCount > 0 ? facts.repCount : '—'}</dd></div>
    </dl>
    <div class="rail-link-row">
      <button class="inline-link" onclick={() => editionHref && onNav(editionHref)}>Edition facts</button>
      <span aria-hidden="true">·</span>
      <button class="inline-link" onclick={() => metadataHref && onNav(metadataHref)}>Identifiers</button>
    </div>
  </section>
</aside>
