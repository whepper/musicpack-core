<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { api, draft, draftStore } from '../bootstrap';
  import {
    validating,
    validation,
    encodeStaging,
    activeStage,
    STAGES,
    type StageId,
  } from '../authoring-state';
  import { runValidation } from '../validate';
  import type { IdentifyCandidate, IdentifyResult } from '../types';
  import { artistLine, formatDate } from '../format';
  import ConfidenceBadge from './ConfidenceBadge.svelte';
  import ReleaseForm from './ReleaseForm.svelte';
  import TrackList from './TrackList.svelte';
  import EncodePanel from './EncodePanel.svelte';
  import ArtworkManager from './ArtworkManager.svelte';
  import IdentityPanel from './IdentityPanel.svelte';
  import WorkflowNav from './WorkflowNav.svelte';
  import ValidationPanel from './ValidationPanel.svelte';
  import SonicPanel from './SonicPanel.svelte';
  import WaveformPanel from './WaveformPanel.svelte';
  import CreateDialog from './CreateDialog.svelte';

  let { onReset }: { onReset: () => void } = $props();


  let coverUrl = $state<string | null>(null);
  let candidates = $state<IdentifyCandidate[] | null>(null);

  async function loadCover(): Promise<void> {
    coverUrl = null;
    const d = draft.get();
    if (!d) return;
    const file = d.artwork.find((a) => a.path);
    if (!file?.path) return;
    try {
      const img = await api.readImage(`${d.sourceRoot}/${file.path}`);
      coverUrl = `data:${img.mime};base64,${img.dataBase64}`;
    } catch {
      coverUrl = null;
    }
  }

  // ---- autosave + window title + debounced auto-validation ----------------

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let validateTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const d = $draft;
    if (!d) return;

    // window title reflects the album being authored
    const artist = d.album.artists?.[0]?.name ?? '';
    document.title =
      `${d.album.title || 'Untitled album'}${artist ? ` — ${artist}` : ''} · MusicPack Author`;

    // autosave (debounced): an accidental quit no longer discards the draft.
    // Skipped while encode staging is active — that transformed draft points
    // at temporary encoded files which won't survive a restart.
    const staged = encodeStaging.get();
    if (!staged && saveTimer) clearTimeout(saveTimer);
    if (!staged) {
      saveTimer = setTimeout(() => {
        void api.draftSave(JSON.stringify(d)).catch(() => {
          /* persistence is best-effort; authoring continues in memory */
        });
      }, 800);
    }

    // auto-validation (debounced): keeps the verdict fresh after edits so
    // Create is gated by a current result rather than a stale one
    if (validateTimer) clearTimeout(validateTimer);
    validateTimer = setTimeout(() => {
      if (!validating.get()) void runValidation();
    }, 1200);
  });

  onMount(() => {
    return () => {
      if (saveTimer) clearTimeout(saveTimer);
      if (validateTimer) clearTimeout(validateTimer);
      document.title = 'MusicPack Author';
    };
  });

  function handleIdentify(result: IdentifyResult): void {
    if (result.kind === 'applied') {
      draftStore.setDraft(result.draft);
      candidates = null;
      void loadCover();
    } else {
      candidates = result.candidates;
    }
  }

  function handleArtworkChange(): Promise<void> {
    return loadCover();
  }

  onMount(() => {
    void loadCover();
  });

  // ---- stage switching -----------------------------------------------------
  // Stage panels stay mounted (hidden) so running tasks keep their progress
  // and panel-local edits survive a switch; only visibility changes. Focus
  // follows the stage change so keyboard users land in the new panel.

  let lastStage: StageId | null = null;

  $effect(() => {
    const stage = $activeStage;
    if (lastStage === null) {
      lastStage = stage;
      return;
    }
    if (stage === lastStage) return;
    lastStage = stage;
    document
      .getElementById(`stage-heading-${stage}`)
      ?.focus({ preventScroll: false });
  });
</script>

{#if $draft}
  <div class="page">
    <div class="content">
      <header class="author-header">
    <div class="cover">
      {#if coverUrl}
        <img src={coverUrl} alt="Front artwork" />
      {:else}
        <div class="cover-empty">
          {#if $draft.artwork.length > 0}
            embedded · extracted at build
          {:else}
            no artwork
          {/if}
        </div>
      {/if}
    </div>
    <div>
      <h1>{$draft.album.title || 'Untitled album'}</h1>
      <div class="artist">{artistLine($draft.album.artists)}</div>
      <div class="edition">
        {#if $draft.release?.edition}{$draft.release.edition}{/if}
        {#if $draft.album.originalReleaseDate}{#if $draft.release?.edition}{' · '}{/if}original {formatDate($draft.album.originalReleaseDate)}{/if}
        {#if $draft.release?.releaseDate} · released {formatDate($draft.release.releaseDate)}{/if}
        {#if $draft.release?.country} · {$draft.release.country}{/if}
        {#if $draft.album.releaseType} · {$draft.album.releaseType}{/if}
      </div>
      <div class="edition meta-line">
        <ConfidenceBadge confidence={$draft.identity?.confidence} />
        <span class="source-root">{$draft.openedFrom ? 'package · ' : ''}{$draft.sourceRoot}</span>
      </div>
    </div>
    <div class="header-actions">
      <button class="btn ghost" onclick={onReset}>← Choose a different album</button>
    </div>
  </header>

  <WorkflowNav />

  {#each STAGES as s (s.id)}
    <div
      class="stage-panel"
      id={`stage-panel-${s.id}`}
      role="tabpanel"
      aria-labelledby={`stage-tab-${s.id}`}
      hidden={$activeStage !== s.id}
    >
      {#if s.id === 'identity'}
        <section class="section">
          <h2 tabindex="-1" id="stage-heading-identity">Identity</h2>
          <IdentityPanel onIdentified={handleIdentify} candidates={candidates} />
        </section>
      {:else if s.id === 'release'}
        <div class="stage-body" id="stage-heading-release" tabindex="-1">
          <ReleaseForm />
        </div>
      {:else if s.id === 'tracks'}
        <section class="section">
          <h2 tabindex="-1" id="stage-heading-tracks">Tracks</h2>
          <TrackList />
        </section>
      {:else if s.id === 'artwork'}
        <section class="section">
          <h2 tabindex="-1" id="stage-heading-artwork">Artwork &amp; assets</h2>
          <ArtworkManager onChange={handleArtworkChange} />
        </section>
      {:else if s.id === 'encode'}
        <div class="stage-body" id="stage-heading-encode" tabindex="-1">
          {#if $draft.openedFrom}
            <section class="section">
              <h2>Audio</h2>
              <p class="muted">
                Tracks are already encoded (Musepack SV8) inside the opened package —
                no encode step is needed. Saving writes your edits back into the
                package without re-encoding.
              </p>
            </section>
          {:else}
            <EncodePanel />
          {/if}
        </div>
      {:else if s.id === 'sonic'}
        <div class="stage-body" id="stage-heading-sonic" tabindex="-1">
          <SonicPanel />
        </div>
      {:else if s.id === 'waveform'}
        <div class="stage-body" id="stage-heading-waveform" tabindex="-1">
          <WaveformPanel />
        </div>
      {:else if s.id === 'validate'}
        <section class="section">
          <h2 tabindex="-1" id="stage-heading-validate">Validation</h2>
          <p class="muted smallcaps">Runs automatically after edits; the button forces a fresh check.</p>
          <button class="btn ghost" onclick={runValidation} disabled={$validating}>
            {$validating ? 'Validating…' : 'Validate now'}
          </button>
          {#if $validation}
            <ValidationPanel result={$validation} />
          {:else}
            <p class="muted">Not validated yet.</p>
          {/if}
        </section>
      {/if}
    </div>
  {/each}
    </div>
  </div>

  <CreateDialog />
{/if}

<style>
  .page {
    display: block;
  }
</style>
