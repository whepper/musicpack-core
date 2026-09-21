<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { draft } from '../bootstrap';
  import { chipState } from '../draft-store';
  import { activeTask, createOpen, validating, validation } from '../authoring-state';
  import { runValidation } from '../validate';


  function openCreate(): void {
    if (validation.get()?.ok) createOpen.set(true);
  }

  const TASK_LABELS: Record<string, string> = {
    encode: 'Encoding…',
    sonic: 'Sonic analysing…',
    waveform: 'Generating waveforms…',
  };
</script>

{#if $draft}
  {@const c = chipState($draft)}
  <footer class="statusbar">
    <div class="chips">
      {#if $activeTask}
        <span class="chip idle" role="status" aria-live="polite">
          ◐ {TASK_LABELS[$activeTask] ?? 'Working…'}
        </span>
      {:else}
        <span class="chip {c.audio}">Audio {c.audio === 'ok' ? '✓' : '?'}</span>
        <span class="chip {c.metadata}">Metadata {c.metadata === 'ok' ? '✓' : '?'}</span>
        <span class="chip {c.artwork}">Artwork {c.artwork === 'ok' ? '✓' : '?'}</span>
        <span class="chip idle">Loudness · at build</span>
        <span class="chip {c.identity}">
          Identity {c.identity === 'ok' ? '✓' : c.identity === 'warn' ? '≈' : '?'}
        </span>
        <span class="chip {c.sonic}">
          Sonic {c.sonic === 'ok' ? '✓' : c.sonic === 'warn' ? '≈' : '· not analysed'}
        </span>
        <span class="chip {c.waveform}">
          Waveform {c.waveform === 'ok' ? '✓' : c.waveform === 'warn' ? '⚠' : '· not generated'}
        </span>
      {/if}
    </div>
    <div class="actions">
      <button class="btn ghost" onclick={runValidation} disabled={$validating}>
        {$validating ? 'Validating…' : 'Validate'}
      </button>
      <button class="btn" onclick={openCreate} disabled={!$validation?.ok}>
        Create MusicPack
      </button>
    </div>
  </footer>
{/if}
