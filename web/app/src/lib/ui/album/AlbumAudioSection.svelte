<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album page, Audio section: how the album plays, codec facts and the
  // per-track representation inventory.
  import InfoGrid from '../InfoGrid.svelte';
  import { codecLabel, codecName, formatBytes, qualityLine } from '../../format';
  import type { ReleaseFacts } from '../album-facts';

  let { facts }: { facts: ReleaseFacts } = $props();

  const primary = $derived(facts.primary);
</script>

{#if primary}
  <div class="section-block">
    <h3 class="smallcaps section-heading">How this album plays</h3>
    <p class="muted">
      Default format {qualityLine({ codec: primary.codec.codec, sampleRate: primary.codec.sampleRate, channels: primary.codec.channels }) || codecLabel(primary.codec.codec)} —
      the player picks per your <a href="/settings">playback quality preference</a>.
    </p>
  </div>
  <InfoGrid
    rows={[
      ['Primary codec', codecName(primary.codec.codec)],
      ['Stream version', primary.codec.streamVersion ? `SV${primary.codec.streamVersion}` : undefined],
      ['Sample rate', primary.codec.sampleRate ? `${primary.codec.sampleRate} Hz` : undefined],
      ['Channels', primary.codec.channels],
      ['Primary audio', facts.primarySize],
      ['Alternate formats', facts.repGroups.map((g) => g.title).join(', ') || undefined],
      ['Total audio', facts.totalSize],
    ]}
  />
  {#if facts.repGroups.length > 0}
    <h3 class="smallcaps section-heading">Per-track representations</h3>
    <ul class="rep-list">
      {#each facts.allTracks as track (track.id)}
        {#if (track.representations?.length ?? 0) > 0}
          <li>
            <span class="rep-track">{track.number}. {track.title}</span>
            <span class="rep-facts">
              {#each track.representations ?? [] as rep (rep.id)}
                <span class="rep-fact">
                  {rep.label ?? codecLabel(rep.codec.codec)} · {qualityLine({ codec: rep.codec.codec, sampleRate: rep.codec.sampleRate, channels: rep.codec.channels })} · {formatBytes(rep.size)}
                </span>
              {/each}
            </span>
          </li>
        {/if}
      {/each}
    </ul>
  {:else}
    <p class="muted">This edition carries a single audio representation per track.</p>
  {/if}
{/if}
