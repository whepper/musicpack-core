<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { draft, draftStore } from '../bootstrap';
  import { codecLabel, fmtTime } from '../format';
  import type { Track } from '../types';
  import { encodeStaging } from '../authoring-state';
  import TrackLyrics from './TrackLyrics.svelte';


  let editing = $state<{ disc: number; track: number } | null>(null);

  function beginEdit(disc: number, track: number): void {
    editing = { disc, track };
  }

  function commitEdit(disc: number, index: number, patch: Partial<Track>): void {
    draftStore.updateTrack(disc, index, patch);
    editing = null;
  }

  function updateTrackField(disc: number, index: number, patch: Partial<Track>): void {
    draftStore.updateTrack(disc, index, patch);
  }

  /** Edits the primary artist's name without destroying additional credited
   * artists or non-main roles (composer, featured, …) filled in by
   * inspection or MusicBrainz. */
  function updateTrackArtist(disc: number, index: number, name: string): void {
    const d = draft.get();
    if (!d) return;
    const track = d.media[disc]?.tracks[index];
    if (!track) return;
    const rest = (track.artists ?? []).slice(1);
    if (!name.trim()) {
      // clearing the field keeps any secondary credits
      draftStore.updateTrack(disc, index, { artists: rest.length > 0 ? rest : undefined });
      return;
    }
    const main = { ...(track.artists?.[0] ?? { role: 'main' }), name };
    draftStore.updateTrack(disc, index, { artists: [main, ...rest] });
  }
</script>

{#if $draft}
{#each $draft.media as medium, di}
  {@const sourceRoot = $draft.sourceRoot}
  <h3 class="disc-title">
    Disc {medium.disc}
    {#if medium.format}<span class="smallcaps">{medium.format}</span>{/if}
    {#if medium.title}— {medium.title}{/if}
  </h3>
  <div class="tracklist">
    {#each medium.tracks as track, ti}
      {@const isEditing = editing?.disc === di && editing?.track === ti}
      {#if isEditing}
        <div class="track editing">
          <span class="num">{track.track}</span>
          <input
            type="text"
            aria-label="Track title"
            value={track.title}
            oninput={(e) => updateTrackField(di, ti, { title: e.currentTarget.value })}
            disabled={$encodeStaging !== null}
          />
          <input
            type="text"
            aria-label="Track artist"
            placeholder="artist"
            value={track.artists?.[0]?.name ?? ''}
            oninput={(e) => updateTrackArtist(di, ti, e.currentTarget.value)}
            disabled={$encodeStaging !== null}
          />
          <button class="btn ghost" onclick={() => commitEdit(di, ti, {})} disabled={$encodeStaging !== null}>Done</button>
          <TrackLyrics disc={di} index={ti} {sourceRoot} />
        </div>
      {:else}
        <button class="track" onclick={() => beginEdit(di, ti)} disabled={$encodeStaging !== null}>
          <span class="num">{track.track}</span>
          <span class="tt">
            {#if track.title}{track.title}{:else}<em>untitled</em>{/if}
            {#if track.artists?.[0]}
              <span class="smallcaps"> · {track.artists[0].name}</span>
            {/if}
          </span>
          <span class="dur">{fmtTime(track.duration)}</span>
          <span class="codec-tag">{codecLabel(track.codec, track.streamVersion)}</span>
          {#if track.sampleRate}
            <span class="smallcaps">
              {track.sampleRate / 1000} kHz{track.bitDepth ? ` · ${track.bitDepth}-bit` : ''}
            </span>
          {/if}
          <span class="filename">{track.audioPath}</span>
          {#if track.lyrics?.length}<span class="smallcaps" title="Has attached lyrics">♪ lyrics</span>{/if}
          {#if track.identifiers?.isrc}<span class="smallcaps">ISRC {track.identifiers.isrc}</span>{/if}
          {#if track.identifiers?.musicbrainzRecordingId}<span class="smallcaps">MB {track.identifiers.musicbrainzRecordingId.slice(0, 8)}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
{/each}
{/if}
