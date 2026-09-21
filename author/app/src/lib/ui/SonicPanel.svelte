<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { api, draft, draftStore } from '../bootstrap';
  import { activeTask } from '../authoring-state';
  import ErrorDetails from './ErrorDetails.svelte';
  import type { ModelStatus, SonicError, SonicProgress } from '../types';

  /** The profiles exposed in the UI. Only the permissive openl3-v1 profile is
   * shipped; the list keeps future profiles selectable without a redesign.
   * Research/restricted profiles are intentionally not exposed. */
  const PROFILES: { id: string; label: string }[] = [
    { id: 'musicpack-sonic-openl3-v1', label: 'MusicPack OpenL3 v1' },
  ];
  const DEFAULT_PROFILE = PROFILES[0] as { id: string; label: string };

  let running = $state(false);
  let done = $state(0);
  let total = $state(0);
  let modelStatus = $state<ModelStatus | null>(null);
  let modelProgress = $state<{ downloaded: number; total: number } | null>(null);
  let verifying = $state(false);
  let errorDetails = $state<string | null>(null);

  function selectedProfile(): { id: string; label: string } {
    const id = $draft?.sonicAnalysis?.profile ?? DEFAULT_PROFILE.id;
    return PROFILES.find((p) => p.id === id) ?? DEFAULT_PROFILE;
  }

  function trackCount(): number {
    return $draft ? $draft.media.reduce((n, m) => n + m.tracks.length, 0) : 0;
  }

  function mb(bytes: number): string {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  async function refreshModelStatus(): Promise<void> {
    try {
      modelStatus = await api.sonicModelStatus();
    } catch {
      modelStatus = null;
    }
  }
  void refreshModelStatus();

  function errorMessage(e: unknown): string {
    const err = e as SonicError;
    switch (err?.code) {
      case 'model_missing':
      case 'offline':
        return 'The Sonic analysis model is not installed and could not be downloaded. Connect to the internet and try again.';
      case 'download_failed':
        return 'The Sonic analysis model could not be downloaded.';
      case 'checksum_mismatch':
        return 'The downloaded Sonic analysis model failed verification.';
      case 'download_cancelled':
        return 'The Sonic model download was cancelled.';
      case 'analyzer_unavailable':
        return 'The Sonic analysis engine is not installed.';
      case 'sonic_retired':
        return 'Sonic analysis is not part of the Rust authoring runtime.';
      case 'runtime_dependency_missing':
        return 'The Sonic analysis engine is missing a required runtime component.';
      case 'analysis_failed':
        return err.message ?? 'Sonic analysis failed.';
      default:
        return err?.message ?? 'Sonic analysis failed.';
    }
  }

  async function analyse(): Promise<void> {
    const d = draft.get();
    if (!d || running) return;
    running = true;
    activeTask.set('sonic');
    errorDetails = null;
    const n = trackCount();
    done = 0;
    total = n;
    modelProgress = null;
    verifying = false;
    draftStore.updateSonicAnalysis((s) => {
      s.status = 'pending';
      s.profile = selectedProfile().id;
      s.tracksTotal = n;
      s.tracksAnalysed = 0;
      s.error = undefined;
      s.warnings = undefined;
    });
    let warnings: string[] = [];
    try {
      const result = await api.sonicAnalyze(d, (p: SonicProgress) => {
        if (p.event === 'model') {
          if (p.state === 'downloading') {
            modelProgress = { downloaded: p.downloaded ?? 0, total: p.total ?? 0 };
            verifying = false;
          } else if (p.state === 'verifying') {
            verifying = true;
          } else if (p.state === 'ready') {
            modelProgress = null;
            verifying = false;
            modelStatus = { ...(modelStatus ?? { profile: '', state: 'ready', sizeBytes: 0 }), state: 'ready' };
          }
        } else if (p.event === 'track') {
          done = p.done ?? done;
          total = p.total ?? total;
          draftStore.updateSonicAnalysis((s) => {
            s.status = 'pending';
            s.tracksAnalysed = p.done ?? 0;
            s.tracksTotal = p.total ?? s.tracksTotal;
          });
          if (p.status === 'no-embedding')
            warnings.push(`disc ${p.disc} track ${p.track} has no embedding`);
        }
      });
      if (result.cancelled) {
        draftStore.updateSonicAnalysis((s) => {
          s.status = 'not_analysed';
          s.path = undefined;
          s.tracksAnalysed = undefined;
        });
      } else if (result.ok) {
        draftStore.updateSonicAnalysis((s) => {
          s.status = warnings.length > 0 ? 'ready-with-warnings' : 'ready';
          s.path = result.outputPath;
          s.tracksAnalysed = result.tracks ?? n;
          s.tracksTotal = result.tracks ?? n;
          s.warnings = warnings.length > 0 ? warnings : undefined;
          s.error = undefined;
        });
      }
    } catch (e) {
      const err = e as SonicError;
      errorDetails = err?.details ?? null;
      draftStore.updateSonicAnalysis((s) => {
        s.status = 'error';
        s.error = errorMessage(e);
      });
    } finally {
      running = false;
      activeTask.set(null);
      modelProgress = null;
      verifying = false;
    }
  }

  async function cancel(): Promise<void> {
    try {
      await api.sonicCancel();
    } catch {
      /* the run will end on its own */
    }
  }
</script>

{#if $draft}
  <section class="section">
    <h2>Sonic Analysis</h2>
    <p class="muted">
      Content-based similarity vectors, computed once at authoring time with the
      <strong>{selectedProfile().label}</strong> profile and stored in the package.
    </p>

    {#if running}
      {#if modelProgress || verifying}
        {#if verifying}
          <p><span class="chip idle">◐</span> Verifying model…</p>
        {:else}
          <p>
            <span class="chip idle">↓</span> Downloading Sonic model…
            {mb(modelProgress?.downloaded ?? 0)} / {mb(modelProgress?.total ?? 0)}
          </p>
        {/if}
        <button class="btn ghost" onclick={cancel}>Cancel</button>
      {:else}
        <p role="status" aria-live="polite"><span class="chip idle">◐</span> Analysing {done} / {total} tracks…</p>
        <button class="btn ghost" onclick={cancel}>Cancel</button>
      {/if}
    {:else}
      {@const s = $draft.sonicAnalysis}
      {#if !s}
        <p>○ Not analysed</p>
        {#if modelStatus?.state === 'missing'}
          <p class="muted">
            {selectedProfile().label} requires a {mb(modelStatus.sizeBytes || 18_742_941)} analysis model, downloaded on first use.
          </p>
        {/if}
        <button class="btn" onclick={analyse}>Analyse Sonic</button>
      {:else if s.status === 'pending'}
        <p><span class="chip idle">◐</span> Analysing…</p>
        <button class="btn ghost" onclick={cancel}>Cancel</button>
      {:else if s.status === 'ready' || s.status === 'ready-with-warnings'}
        <p>
          <span class="chip {s.status === 'ready' ? 'ok' : 'warn'}">
            {s.status === 'ready' ? '●' : '⚠'}
          </span>
          {s.status === 'ready' ? 'Ready' : 'Ready with warnings'}
        </p>
        <p class="muted">
          {s.profile} · {s.tracksAnalysed ?? 0} / {s.tracksTotal ?? 0} tracks
        </p>
        {#if s.warnings?.length}
          <ul>
            {#each s.warnings as w}
              <li class="warn">{w}</li>
            {/each}
          </ul>
        {/if}
        <button class="btn ghost" onclick={analyse}>Re-analyse</button>
      {:else if s.status === 'error'}
        <p><span class="chip warn">✕</span> Analysis failed</p>
        <ErrorDetails
          message={s.error ?? 'Sonic analysis failed.'}
          details={errorDetails}
          onRetry={analyse}
          retryLabel="Retry Sonic Analysis"
        />
      {:else}
        <p>○ Not analysed</p>
        <button class="btn" onclick={analyse}>Analyse Sonic</button>
      {/if}
    {/if}
  </section>
{/if}
