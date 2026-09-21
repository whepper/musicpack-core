<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { api, draft, draftStore } from '../bootstrap';
  import { activeTask, setEncodeStaging } from '../authoring-state';
  import { needsEncoding } from '../format';
  import ErrorDetails from './ErrorDetails.svelte';
  import type { EncodeProgress } from '../types';

  const DEFAULT_QUALITY = '6.0';
  const QUALITIES = [
    { value: '6.0', label: 'q6 — excellent (recommended)' },
    { value: '5.0', label: 'q5 — high' },
    { value: '7.0', label: 'q7 — insane' },
    { value: '8.0', label: 'q8 — braindead' },
  ];

  let running = $state(false);
  let done = $state(0);
  let total = $state(0);
  let stage = $state<string | null>(null);
  let currentTitle = $state<string | null>(null);
  let quality = $state(DEFAULT_QUALITY);
  let error = $state<string | null>(null);
  let errorDetails = $state<string | null>(null);
  let encoded = $state(false);

  function trackCount(): number {
    return $draft ? $draft.media.reduce((n, m) => n + m.tracks.length, 0) : 0;
  }

  function needsEncode(): boolean {
    const d = $draft;
    return d ? d.media.some((m) => m.tracks.some(needsEncoding)) : false;
  }

  async function runEncode(): Promise<void> {
    const d = draft.get();
    if (!d || running) return;
    running = true;
    activeTask.set('encode');
    encoded = false;
    error = null;
    errorDetails = null;
    done = 0;
    total = trackCount();
    stage = null;
    currentTitle = null;
    try {
      const result = await api.encodeTracks(d, quality, (p: EncodeProgress) => {
        if (p.event === 'stage') {
          stage = p.stage ?? null;
          currentTitle = p.title ?? null;
          done = p.done ?? done;
          total = p.total ?? total;
        } else if (p.event === 'track') {
          done = p.done ?? done;
          total = p.total ?? total;
          currentTitle = null;
        }
      });
      if (result.cancelled) {
        setEncodeStaging(null);
        error = null;
      } else if (result.ok && result.draft) {
        draftStore.setDraft(result.draft);
        setEncodeStaging(result.outputDir ?? null);
        encoded = true;
      } else {
        error = 'Encoding ended without a result.';
      }
    } catch (e) {
      setEncodeStaging(null);
      const msg = e instanceof Error ? e.message : 'Encoding failed.';
      // encode errors arrive as one string with an appended backend-output
      // section; split it so the details expander renders it properly
      const sep = '\n\n--- backend output ---\n';
      const at = msg.indexOf(sep);
      if (at >= 0) {
        error = msg.slice(0, at);
        errorDetails = msg.slice(at + sep.length);
      } else {
        error = msg;
      }
    } finally {
      running = false;
      activeTask.set(null);
      stage = null;
      currentTitle = null;
    }
  }

  async function cancel(): Promise<void> {
    try {
      await api.encodeCancel();
    } catch {
      /* the run will end on its own */
    }
  }

  const stageLabel: Record<string, string> = {
    decoding: 'Decoding source',
    encoding: 'Encoding',
    tagging: 'Tagging',
  };
</script>

{#if $draft && (needsEncode() || encoded)}
  <section class="section">
    <h2>Encode to Musepack</h2>
    <p class="muted">
      Lossless sources are converted to Musepack SV8 at
      <strong>q{quality}</strong> before the package is built. Your source
      files are never modified.
    </p>

    {#if running}
      <p role="status" aria-live="polite">
        <span class="chip idle">◐</span>
        {stage ? stageLabel[stage] ?? stage : 'Preparing'} —
        {done} / {total} tracks{currentTitle ? ` · ${currentTitle}` : ''}
      </p>
      <button class="btn ghost" onclick={cancel}>Cancel</button>
    {:else if encoded}
      <p>
        <span class="chip ok">●</span>
        Encoded {total} track(s) at q{quality} — ready to create the package
      </p>
      <p class="muted">
        Tracks carry the tags written at encode time. Edit album and track
        metadata before encoding; editing is locked until you choose the source
        album again.
      </p>
    {:else}
      {#if error}
        <ErrorDetails message={error} details={errorDetails} />
      {/if}
      <button class="btn" onclick={runEncode} disabled={running}>
        Encode to Musepack
      </button>
      <details class="advanced">
        <summary>Quality</summary>
        <select aria-label="Musepack quality" bind:value={quality}>
          {#each QUALITIES as q}
            <option value={q.value}>{q.label}</option>
          {/each}
        </select>
      </details>
    {/if}
  </section>
{/if}
