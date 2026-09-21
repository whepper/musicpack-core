<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { api } from '../bootstrap';
  import { draftStore, draft } from '../bootstrap';
  import ErrorDetails from './ErrorDetails.svelte';
  import { activeTask } from '../authoring-state';
  import type { Draft, WaveformProgress } from '../types';

  let d = $derived($draft);
  let generating = $state(false);
  let progressDone = $state(0);
  let progressTotal = $state(0);
  let cancelled = $state(false);
  let autoStarted = $state(false);
  let errorDetails = $state<string | null>(null);

  const status = $derived(d?.waveformAnalysis?.status ?? 'not_generated');
  const tracksTotal = $derived(
    d?.media.reduce((n, m) => n + m.tracks.length, 0) ?? 0,
  );

  // Waveforms are default-on for a new draft. Do not restart automatically
  // after cancellation or failure; those remain explicit user choices.
  $effect(() => {
    if (!d || status !== 'not_generated' || tracksTotal === 0 || autoStarted) return;
    autoStarted = true;
    void generate();
  });

  async function generate() {
    if (!d || generating) return;
    generating = true;
    activeTask.set('waveform');
    cancelled = false;
    progressDone = 0;
    progressTotal = tracksTotal;
    errorDetails = null;

    // Mark pending so build-draft will reject a missing block.
    draftStore.updateWaveformAnalysis((s) => {
      s.status = 'pending';
      s.error = undefined;
      s.tracks = [];
      s.intervalMs = 100;
      s.encoding = 'peak-rms-u8';
      s.floorDb = -60;
    });

    try {
      const result = await api.waveformAnalyze(d, (p: WaveformProgress) => {
        if (p.event === 'track' && p.status === 'ok') {
          progressDone = p.done ?? progressDone + 1;
          if (p.disc !== undefined && p.track !== undefined && p.points !== undefined) {
            draftStore.updateWaveformAnalysis((s) => {
              s.tracks.push({
                disc: p.disc!,
                track: p.track!,
                points: p.points!,
                sha256: p.sha256 ?? '',
                path: p.path ?? '',
              });
              s.tracksGenerated = s.tracks.length;
              s.tracksTotal = tracksTotal;
            });
          }
        } else if (p.event === 'cancelled') {
          cancelled = true;
        }
      });
      if (cancelled || result.cancelled) {
        draftStore.updateWaveformAnalysis((s) => {
          s.status = 'not_generated';
          s.error = 'cancelled';
          s.tracks = [];
        });
      } else if (result.draft) {
        // Adopt the transformed draft (with the waveformAnalysis block from
        // the CLI) as the new draft. This is the same pattern as encode.
        draftStore.setDraft(result.draft as Draft);
      } else {
        draftStore.updateWaveformAnalysis((s) => {
          s.status = 'ready';
        });
      }
    } catch (e) {
      const err = e as { code?: string; message?: string; details?: string };
      errorDetails = err.details ?? null;
      draftStore.updateWaveformAnalysis((s) => {
        s.status = 'error';
        s.error = err.message ?? String(e);
      });
    } finally {
      generating = false;
      activeTask.set(null);
    }
  }

  async function cancel() {
    if (!generating) return;
    await api.waveformCancel();
  }

  function setEnabled(enabled: boolean) {
    if (!d) return;
    draftStore.updateWaveformAnalysis((s) => {
      s.status = enabled ? 'pending' : 'disabled';
      if (!enabled) {
        s.tracks = [];
        s.tracksGenerated = 0;
      }
    });
  }
</script>

<section class="wf-panel">
  <header>
    <h2>Waveform</h2>
    <span class="meta">
      {#if status === 'ready'}
        Generated · {d?.waveformAnalysis?.tracks?.length ?? 0} / {tracksTotal} tracks
      {:else if status === 'pending'}
        Generating {progressDone} / {progressTotal}…
      {:else if status === 'disabled'}
        Disabled · building without waveform
      {:else if status === 'error'}
        Error · {d?.waveformAnalysis?.error ?? ''}
      {:else}
        Not generated · {tracksTotal} tracks
      {/if}
    </span>
  </header>

  {#if status === 'error'}
    <ErrorDetails
      message={d?.waveformAnalysis?.error ?? 'Waveform generation failed.'}
      details={errorDetails}
      onRetry={generate}
      retryLabel="Regenerate waveforms"
    />
  {:else if status === 'not_generated'}
    <div class="row">
      <button onclick={generate} disabled={generating || tracksTotal === 0}>
        Generate waveforms
      </button>
      <span class="hint">100 ms · peak + RMS · 1.2 KB/minute</span>
    </div>
  {:else if status === 'pending'}
    <div class="row" role="status" aria-live="polite">
      <progress max={tracksTotal} value={progressDone}></progress>
      <button onclick={cancel}>Cancel</button>
    </div>
  {:else if status === 'ready'}
    <div class="row">
      <button onclick={generate}>Regenerate waveforms</button>
      <span class="hint">Updated {d?.waveformAnalysis?.tracksGenerated ?? 0} / {tracksTotal}</span>
    </div>
  {:else if status === 'disabled'}
    <div class="row">
      <button onclick={() => setEnabled(true)}>Enable waveform generation</button>
      <span class="hint">Building this package without waveform envelopes.</span>
    </div>
  {/if}
</section>

<style>
  .wf-panel { padding: var(--space-3) var(--space-4); }
  header { display: flex; align-items: baseline; gap: var(--space-3); margin-bottom: var(--space-2); }
  h2 { margin: 0; font-size: 1.05rem; }
  .meta { font-size: 0.8rem; color: var(--text-soft); }
  .row { display: flex; gap: var(--space-2); align-items: center; }
  .row progress { flex: 1; }
  .hint { color: var(--text-soft); font-size: 0.8rem; }
</style>
