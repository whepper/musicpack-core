<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Rail summary: the audio representations this release actually carries
  // (primary stream + alternates), with the package-level verification
  // state shown once — per-file claims are never invented (DM-11/DM-12).
  // A card's Play button applies the matching real playback preference and
  // starts the album (the existing representation-selection policy does
  // the routing).
  import type { AudioPreference } from '../state/representation-selection';
  import type { ReleaseDetail } from '../api/types';
  import StatusChip from './StatusChip.svelte';
  import { verifyStatusChip } from './status';
  import { codecName, formatBytes, qualityLine } from '../format';

  let {
    rel,
    onPlay,
    onSection,
  }: {
    rel: ReleaseDetail;
    onPlay: (preference?: AudioPreference) => void;
    onSection: (id: string) => void;
  } = $props();

  interface Group {
    key: string;
    title: string;
    sub: string;
    codec: string;
  }

  const primary = $derived(rel.media[0]?.tracks[0]);

  const groups = $derived.by(() => {
    const out: Group[] = [];
    if (primary) {
      const isMpc = primary.codec.codec.toLowerCase().startsWith('musepack');
      out.push({
        key: 'primary',
        title: codecName(primary.codec.codec),
        sub: isMpc && primary.codec.streamVersion
          ? `SV${primary.codec.streamVersion}`
          : qualityLine({ codec: primary.codec.codec, sampleRate: primary.codec.sampleRate, channels: primary.codec.channels }),
        codec: '',
      });
    }
    const seen = new Map<string, Group>();
    for (const disc of rel.media) {
      for (const track of disc.tracks) {
        for (const rep of track.representations ?? []) {
          const key = (rep.label ?? rep.codec.codec).toLowerCase();
          if (seen.has(key)) {
            seen.get(key)!.sub = qualityLine({
              codec: rep.codec.codec,
              sampleRate: rep.codec.sampleRate,
              channels: rep.codec.channels,
            });
            continue;
          }
          const g: Group = {
            key,
            title: rep.label ?? codecName(rep.codec.codec),
            sub: qualityLine({
              codec: rep.codec.codec,
              sampleRate: rep.codec.sampleRate,
              channels: rep.codec.channels,
            }),
            codec: rep.codec.codec.toLowerCase(),
          };
          seen.set(key, g);
          out.push(g);
        }
      }
    }
    return out;
  });

  const primarySize = $derived(
    formatBytes((rel.media ?? []).reduce(
      (sum, disc) => sum + disc.tracks.reduce((s, t) => s + t.audio.size, 0),
      0,
    )),
  );
</script>

<section class="rail-panel">
  <header>
    <h3 class="smallcaps">Audio representations</h3>
    <span class="smallcaps rail-count">{groups.length}</span>
    <button class="rail-link" aria-label="Open the full audio section" onclick={() => onSection('audio')}>›</button>
  </header>

  {#each groups as g (g.key)}
    <div class="rep-card">
      <div class="rep-body">
        <p class="rep-title">
          {g.title}
          {#if g.key === 'primary' && primarySize}<span class="rep-size"> · {primarySize}</span>{/if}
        </p>
        {#if g.sub}<p class="rep-sub">{g.sub}</p>{/if}
      </div>
      {#if g.key === 'primary'}
        <button class="btn-small" aria-label="Play the album in its default format" onclick={() => onPlay()}>Play</button>
      {:else}
        <button
          class="btn-small"
          aria-label={`Prefer ${g.title} and play the album`}
          onclick={() => onPlay({ mode: 'codec', codec: g.codec })}
        >Play</button>
      {/if}
    </div>
  {/each}

  <div class="rail-verify">
    <StatusChip chip={verifyStatusChip(rel.verifyStatus)} />
  </div>
</section>
