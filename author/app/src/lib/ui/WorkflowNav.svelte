<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<!-- Workflow stepper: the primary navigation over authoring stages. Behaves
     like tabs (ARIA tabs pattern, arrow-key roving focus) but reads as a
     compact stage rail, with one quiet status mark per stage. -->
<script lang="ts">
  import { draft } from '../bootstrap';
  import { chipState } from '../draft-store';
  import {
    STAGES,
    activeStage,
    encodeStaging,
    validation,
    type StageId,
  } from '../authoring-state';
  import { needsEncoding } from '../format';

  type Mark = 'ok' | 'warn' | 'idle';

  const MARK_CHAR: Record<Mark, string> = { ok: '✓', warn: '!', idle: '·' };
  const MARK_TEXT: Record<Mark, string> = {
    ok: 'complete',
    warn: 'needs attention',
    idle: 'not started',
  };

  let tabEls = $state<Partial<Record<StageId, HTMLButtonElement>>>({});

  // One quiet mark per stage, derived from the same local state that feeds
  // the readiness bar — no extra sources of truth.
  const marks = $derived.by<Record<StageId, Mark>>(() => {
    const d = $draft;
    const out = {} as Record<StageId, Mark>;
    const c = d ? chipState(d) : null;
    for (const s of STAGES) {
      switch (s.id) {
        case 'identity':
        case 'release':
        case 'tracks':
        case 'artwork':
        case 'sonic':
          out[s.id] = c ? c[markKey(s.id)] : 'idle';
          break;
        case 'waveform':
          out[s.id] = c?.waveform === 'warn' ? 'warn' : (c?.waveform ?? 'idle');
          break;
        case 'encode':
          if (!d) out[s.id] = 'idle';
          else if (d.openedFrom || $encodeStaging) out[s.id] = 'ok';
          else
            out[s.id] = d.media.some((m) => m.tracks.some(needsEncoding))
              ? 'idle'
              : 'ok';
          break;
        case 'validate': {
          const v = $validation;
          out[s.id] = !v ? 'idle' : v.ok ? 'ok' : 'warn';
          break;
        }
      }
    }
    return out;
  });

  function markKey(id: StageId): 'identity' | 'metadata' | 'audio' | 'artwork' | 'sonic' {
    switch (id) {
      case 'identity': return 'identity';
      case 'release': return 'metadata';
      case 'tracks': return 'audio';
      case 'artwork': return 'artwork';
      default: return 'sonic';
    }
  }

  function select(id: StageId): void {
    activeStage.set(id);
  }

  function onKeydown(e: KeyboardEvent): void {
    const ids = STAGES.map((s) => s.id);
    const idx = ids.indexOf($activeStage);
    let next = -1;
    if (e.key === 'ArrowRight') next = (idx + 1) % ids.length;
    else if (e.key === 'ArrowLeft') next = (idx - 1 + ids.length) % ids.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = ids.length - 1;
    if (next < 0) return;
    e.preventDefault();
    const id = ids[next]!;
    activeStage.set(id);
    tabEls[id]?.focus();
  }
</script>

<nav class="workflow-nav" aria-label="Authoring stages">
  <div class="stages" role="tablist" tabindex={-1} onkeydown={onKeydown}>
    {#each STAGES as s (s.id)}
      <button
        bind:this={tabEls[s.id]}
        id={`stage-tab-${s.id}`}
        class="stage"
        class:active={$activeStage === s.id}
        role="tab"
        aria-selected={$activeStage === s.id}
        aria-controls={`stage-panel-${s.id}`}
        tabindex={$activeStage === s.id ? 0 : -1}
        aria-label={`${s.label} — ${MARK_TEXT[marks[s.id]]}`}
        onclick={() => select(s.id)}
      >
        <span class="mark {marks[s.id]}" aria-hidden="true">{MARK_CHAR[marks[s.id]]}</span>
        <span class="label">{s.label}</span>
      </button>
    {/each}
  </div>
</nav>

<style>
  .workflow-nav {
    position: sticky;
    top: 0;
    z-index: 10;
    background: var(--bg-rail);
    border-top: 1px solid var(--hairline);
    border-bottom: 1px solid var(--hairline);
    margin-bottom: var(--space-5);
  }
  .stages {
    display: flex;
    gap: var(--space-1);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .stages::-webkit-scrollbar {
    display: none;
  }
  .stage {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 9px 14px 7px;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    color: var(--text-soft);
    font-size: var(--fs-sm);
    white-space: nowrap;
    flex-shrink: 0;
  }
  .stage:hover {
    color: var(--text);
    background: var(--surface);
  }
  .stage.active {
    color: var(--text);
    font-weight: 600;
    border-bottom-color: var(--accent);
  }
  .mark {
    width: 1em;
    text-align: center;
    color: var(--text-faint);
    opacity: 0.55;
    font-size: var(--fs-xs);
  }
  .mark.ok {
    color: var(--ok);
    opacity: 1;
  }
  .mark.warn {
    color: var(--accent-strong);
    opacity: 1;
  }
  @media (max-width: 720px) {
    .stage {
      padding: 8px 10px 6px;
      font-size: var(--fs-xs);
    }
  }
</style>
