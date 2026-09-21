<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { player, playerModel, queue } from '../bootstrap';
  import { fmtTime } from '../format';

  const currentTrackId = $derived($playerModel.current?.track.id);
  // Reactive: queue.get() is a plain read and would freeze the highlight at
  // whatever index the list first rendered with (the stuck-highlight bug).
  const currentIndex = $derived($queue.index);
</script>

{#if $queue.items.length === 0}
  <p class="empty-state queue-empty">
    Nothing in the queue. Play an album from the shelf.
  </p>
{:else}
  <div class="queue-meta">
    <span class="smallcaps">{`${$queue.items.length} item${$queue.items.length === 1 ? '' : 's'}`}</span>
    <button class="smallcaps queue-clear" onclick={() => queue.clear()}>
      Clear queue
    </button>
  </div>
  <div class="queue-list">
    {#each $queue.items as item, i (item.track.id)}
      <div class="queue-item" aria-current={i === currentIndex ? 'true' : undefined}>
        <img class="queue-thumb" src={item.artworkUrl ?? '/placeholder.svg'} alt="" aria-hidden="true"
          onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')}>
        <div class="queue-item-body">
          <button
            class="queue-open"
            aria-current={i === currentIndex ? 'true' : undefined}
            onclick={() => {
              void player.playQueueIndex(i);
            }}
          >
            <div class="queue-tt">
              <span class="num">{i + 1}.</span>
              {' '}{item.track.title}
              {#if i === currentIndex}
                <span class="muted queue-elapsed"> · {fmtTime(Math.max(0, $playerModel.positionSeconds - $playerModel.currentTrackStartSeconds))}</span>
              {/if}
            </div>
            <div class="queue-art">{item.artist} — {item.albumTitle}{item.edition ? ` · ${item.edition}` : ''}</div>
          </button>
        </div>
        <div class="queue-ops">
          <button
            class="queue-move"
            aria-label={`Move ${item.track.title} up in the queue`}
            disabled={i === 0}
            onclick={() => queue.move(i, i - 1)}
          >▲</button>
          <button
            class="queue-move"
            aria-label={`Move ${item.track.title} down in the queue`}
            disabled={i === $queue.items.length - 1}
            onclick={() => queue.move(i, i + 1)}
          >▼</button>
          <button
            class="queue-remove"
            aria-label={`Remove ${item.track.title} from the queue`}
            onclick={() => queue.removeAt(i)}>✕</button>
        </div>
      </div>
    {/each}
  </div>
{/if}
