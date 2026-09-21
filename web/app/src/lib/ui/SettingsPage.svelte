<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Settings: playback quality (the Phase-4 audio preference, exposed at
  // last) plus downloads & storage (offline lifecycle management). Every
  // control binds to an EXISTING store/manager — no new state model.
  import { audioPreference, offline } from '../bootstrap';
  import type { Writable } from '../store';

  const preferenceWritable: Writable<AudioPreference> = audioPreference.preference;
  import { LOSSLESS_CODECS } from '../state/representation-selection';
  import { codecLabel, formatBytes } from '../format';
  import type { InstalledPackage } from '../offline/types';
  import type { AudioPreference } from '../state/representation-selection';

  const pref = $derived($preferenceWritable);

  function setMode(mode: 'default' | 'lossless'): void {
    audioPreference.set({ mode });
  }
  function setCodec(codec: string): void {
    audioPreference.set({ mode: 'codec', codec });
  }

  const isCodecPref = $derived(pref.mode === 'codec');

  // ---- storage section -------------------------------------------------
  let usageBytes = $state<number | null>(null);
  let quotaBytes = $state<number | null>(null);
  let persisted = $state<boolean | null>(null);
  let packages: InstalledPackage[] = $state([]);
  // Bump to re-read the catalog after a remove.
  let storageVersion = $state(0);

  $effect(() => {
    void storageVersion;
    void offline.storageUsage().then((u) => {
      usageBytes = u.usageBytes;
      quotaBytes = u.quotaBytes;
      persisted = u.persisted;
    });
    void offline.listPackages().then((pkgs) => {
      packages = pkgs.filter((p) => p.status === 'installed');
    });
  });

  async function removePackage(releaseId: number): Promise<void> {
    await offline.remove(releaseId);
    storageVersion++;
  }

  const usageLine = $derived.by(() => {
    const used = formatBytes(usageBytes);
    if (!used) return 'Storage use unknown';
    const quota = formatBytes(quotaBytes);
    return quota ? `${used} of ${quota} used` : `${used} used`;
  });
</script>

<div class="settings-page">
  <h1>Settings</h1>

  <section aria-labelledby="quality-heading">
    <h2 id="quality-heading" class="smallcaps">Playback quality</h2>
    <p class="muted">
      Chooses which stored format plays when a track carries more than one.
      Changes apply to albums and tracks you play next — what is already
      playing keeps its source. If a preferred format is unavailable the
      player falls back automatically.
    </p>
    <div class="pref-group" role="radiogroup" aria-label="Playback quality preference">
      <label class="pref-option">
        <input
          type="radio"
          name="audio-pref"
          checked={pref.mode === 'default'}
          onchange={() => setMode('default')}
        >
        <span>
          <strong>Automatic</strong>
          <span class="muted"> — the album’s default format (Musepack when present)</span>
        </span>
      </label>
      <label class="pref-option">
        <input
          type="radio"
          name="audio-pref"
          checked={pref.mode === 'lossless'}
          onchange={() => setMode('lossless')}
        >
        <span>
          <strong>Prefer lossless</strong>
          <span class="muted"> — FLAC/WAV/AIFF when available; uses more bandwidth for music not downloaded</span>
        </span>
      </label>
      {#each LOSSLESS_CODECS as codec (codec)}
        <label class="pref-option">
          <input
            type="radio"
            name="audio-pref"
            checked={isCodecPref && (pref as { codec: string }).codec.toLowerCase() === codec}
            onchange={() => setCodec(codec)}
          >
          <span>
            <strong>{codecLabel(codec)} only</strong>
            <span class="muted"> — play {codecLabel(codec)} when the track offers it</span>
          </span>
        </label>
      {/each}
    </div>
  </section>

  <section aria-labelledby="storage-heading">
    <h2 id="storage-heading" class="smallcaps">Downloads &amp; storage</h2>
    {#if !offline.enabled}
      <p class="muted">
        This browser does not support offline downloads, so nothing has been
        stored.
      </p>
    {:else}
      <p class="muted">{usageLine}.</p>
      <p class="muted">
        {persisted === true
          ? 'Downloads are protected from automatic cleanup by your browser.'
          : persisted === false
            ? 'Your browser may clear downloads automatically when storage runs low.'
            : ''}
      </p>
      {#if packages.length === 0}
        <p class="empty-state">No downloads yet.</p>
      {:else}
        <ul class="pkg-list" role="list" aria-label="Downloaded albums">
          {#each packages as pkg (pkg.releaseId)}
            <li class="pkg-row">
              <span class="pkg-title">
                {pkg.releaseDetail?.album?.title ?? `Release ${pkg.releaseId}`}
                {#if pkg.stale}<span class="smallcaps pkg-flag">(update available)</span>{/if}
              </span>
              <span class="pkg-size muted">{formatBytes(pkg.bytes)}</span>
              <button
                class="btn ghost"
                aria-label={`Remove ${pkg.releaseDetail?.album?.title ?? 'this'} download`}
                onclick={() => void removePackage(pkg.releaseId)}>Remove</button>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </section>
</div>
