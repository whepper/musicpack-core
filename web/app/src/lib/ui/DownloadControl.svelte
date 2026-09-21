<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // The release/edition download control: one self-contained view of the
  // offline manager's lifecycle for a single release. All state comes from
  // the manager's reactive stores; every action is a one-line manager call.
  // Update/Retry/Reinstall always fetch FRESH release JSON (the library
  // cache holds the hashes captured when the page was opened).
  import { offline, library } from '../bootstrap';
  import type { Readable } from '../store';
  import type { PackageUiState } from '../offline/availability';
  import type { DownloadProgress } from '../offline/types';
  import { formatBytes } from '../format';
  import type { ReleaseDetail } from '../api/types';

  let {
    release,
    states,
    downloads,
  }: {
    release: ReleaseDetail;
    states: Readable<Map<number, PackageUiState>>;
    downloads: Readable<DownloadProgress[]>;
  } = $props();

  const stateOf = $derived($states.get(release.id));
  // Progress detail (bytes/asset counts) is available via $downloads when a
  // richer progress readout is wanted; the % ring covers v1.
  const sizeHint = $derived.by(() => {
    if (!release) return '';
    let total = 0;
    for (const disc of release.media) {
      for (const track of disc.tracks) {
        total += track.audio.size;
        for (const rep of track.representations ?? []) total += rep.size;
      }
    }
    return formatBytes(total);
  });

  function startInstall(detail: ReleaseDetail): void {
    void library.refreshRelease(detail.id).then((fresh) => offline.install(fresh));
  }

  async function update(): Promise<void> {
    // Explicit user action only (D2): re-download replaces the committed
    // record atomically and clears the stale flag via the fresh install.
    startInstall(release);
  }

  const label = $derived.by(() => {
    switch (stateOf?.state) {
      case 'downloading':
        return 'Downloading…';
      case 'installed':
        return sizeHint ? `Available offline · ${sizeHint}` : 'Available offline';
      case 'stale':
        return 'Update available';
      case 'damaged':
        return 'Needs repair';
      case 'failed':
        return 'Download failed';
      default:
        return sizeHint ? `Download · ${sizeHint}` : 'Download';
    }
  });

  function onPrimaryClick(): void {
    switch (stateOf?.state) {
      case undefined:
      case 'not-installed':
        startInstall(release);
        break;
      case 'failed':
      case 'stale':
      case 'damaged':
        startInstall(release);
        break;
      default:
        break; // downloading / installed: primary click does nothing
    }
  }
</script>

{#if !offline.enabled}
  <!-- Graceful degradation: browsers without OPFS/IDB see nothing at all. -->
{:else}
  <div class="dl-control">
    {#if stateOf?.state === 'installed'}
      <span class="dl-badge smallcaps" role="status">⤓ Installed</span>
      <button
        class="btn ghost dl-menu-btn"
        aria-label={`Remove ${release.album.title} download`}
        onclick={() => void offline.remove(release.id)}
      >Remove</button>
    {:else if stateOf?.state === 'downloading'}
      <span
        class="dl-progress"
        role="progressbar"
        aria-valuenow={stateOf.percent}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`Downloading ${release.album.title}`}
      >
        <svg viewBox="0 0 20 20" class="dl-ring" aria-hidden="true">
          <circle class="ring-track" cx="10" cy="10" r="8" />
          <circle
            class="ring-fill"
            cx="10" cy="10" r="8"
            stroke-dasharray={`${(stateOf.percent / 100) * 50.27} 50.27`}
          />
        </svg>
        <span class="smallcaps">{stateOf.percent}%</span>
      </span>
      <button
        class="btn ghost"
        aria-label={`Cancel ${release.album.title} download`}
        onclick={() => offline.cancel(release.id)}
      >Cancel</button>
    {:else if stateOf?.state === 'stale' || stateOf?.state === 'damaged'}
      <span class="dl-badge dl-attention smallcaps" role="status" aria-live="polite">
        {stateOf.state === 'stale' ? '⤓ Update available' : '⤓ Needs repair'}
      </span>
      <button
        class="btn ghost"
        aria-label={stateOf.state === 'stale'
          ? `Update ${release.album.title} download`
          : `Reinstall ${release.album.title} download`}
        onclick={update}
      >{stateOf.state === 'stale' ? 'Update' : 'Reinstall'}</button>
      <button
        class="btn ghost dl-menu-btn"
        aria-label={`Remove ${release.album.title} download`}
        onclick={() => void offline.remove(release.id)}
      >Remove</button>
    {:else if stateOf?.state === 'failed'}
      <button
        class="btn ghost"
        aria-label={`Retry ${release.album.title} download`}
        onclick={() => void library.refreshRelease(release.id).then((f) => offline.install(f))}
      >Retry</button>
      <span class="dl-error muted smallcaps" role="alert">{stateOf.reason}</span>
    {:else}
      <button
        class="btn ghost"
        aria-label={`Download ${release.album.title} for offline${sizeHint ? ` (${sizeHint})` : ''}`}
        onclick={() => startInstall(release)}
      >⤓ Download</button>
    {/if}
  </div>
{/if}

<style>
  /* Scoped additions; palette/typography come from theme.css variables so
     the control reads as part of the existing design system. */
  .dl-control {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .dl-badge {
    color: var(--accent);
  }
  .dl-attention {
    color: var(--text);
    border: 1px solid var(--accent);
    border-radius: 999px;
    padding: 2px 10px;
  }
  .dl-progress {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }
  .dl-ring {
    width: 20px;
    height: 20px;
    transform: rotate(-90deg);
  }
  .dl-ring circle {
    fill: none;
    stroke-width: 2.5;
  }
  .ring-track {
    stroke: var(--hairline);
  }
  .ring-fill {
    stroke: var(--accent);
    stroke-linecap: round;
  }
  .dl-error {
    color: var(--danger);
  }
  @media (max-width: 680px) {
    .dl-menu-btn {
      display: none; /* compact variant: Remove lives in Settings on mobile */
    }
  }
</style>
