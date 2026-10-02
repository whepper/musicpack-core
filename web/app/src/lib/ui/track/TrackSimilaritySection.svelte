<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Track page, Similar tracks section: the Slice 0 discovery surface. The
  // Player knows nothing about the embedding model — it consumes the
  // `{track, release, score, rank}` list the server returns (ADR 0017 §5.9).
  //
  // Capability absence is explicit: `unavailable` is a first-class state,
  // never an empty result that reads as "nothing is similar" (FORMAT_SPEC
  // §13, ADR 0017 §5.8).
  import type { SimilarTrack } from '../../api/types';
  import type { SimilarTracksState } from '../../state/similarity';

  let { state }: { state: SimilarTracksState } = $props();

  function artistOf(n: SimilarTrack): string {
    return n.track.artists.map((a) => a.name).join(', ') || 'Unknown artist';
  }
</script>

<div class="album-section">
  <h2 class="smallcaps section-heading">Similar tracks</h2>

  {#if state.state === 'loading' || state.state === 'idle'}
    <p class="muted">Loading similar tracks…</p>
  {:else if state.state === 'unavailable'}
    <p class="muted">
      Similarity is not available for this track. It may not have been analyzed,
      or the similarity index is not populated.
    </p>
  {:else if state.state === 'error'}
    <p class="muted">Could not load similar tracks. {state.message}</p>
  {:else if state.neighbors.length === 0}
    <p class="muted">No similar tracks were found in your collection.</p>
  {:else}
    <ol class="similar-list">
      {#each state.neighbors as n (n.track.id)}
        <li class="similar-row">
          <span class="similar-rank">{n.rank}</span>
          <div class="similar-meta">
            <a class="similar-title" href="/tracks/{n.track.id}">{n.track.title}</a>
            <span class="similar-artist">{artistOf(n)}</span>
          </div>
          <span class="similar-score" title="Within-model cosine similarity">
            {n.score.toFixed(3)}
          </span>
        </li>
      {/each}
    </ol>
    {#if state.profileId}
      <p class="muted section-note">Profile: {state.profileId}</p>
    {/if}
  {/if}
</div>
