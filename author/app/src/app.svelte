<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { api, draft, busy, draftStore, error } from './lib/bootstrap';
  import { encodeStaging, setEncodeStaging } from './lib/authoring-state';
  import BackendBanner from './lib/ui/BackendBanner.svelte';
  import Welcome from './lib/ui/Welcome.svelte';
  import AlbumAuthoring from './lib/ui/AlbumAuthoring.svelte';
  import StatusBar from './lib/ui/StatusBar.svelte';
  import type { Draft } from './lib/types';

  async function openAlbum(path: string): Promise<void> {
    draftStore.setBusy(true);
    draftStore.setError(null);
    // leaving one album for another drops the old encode staging area
    const stale = encodeStaging.get();
    if (stale) {
      setEncodeStaging(null);
      void api.cleanupStaging(stale);
    }
    try {
      const d = await api.inspectAlbum(path);
      draftStore.setDraft(d);
      // remember successful opens for the Welcome screen
      void api.recentsAdd(path, d.album?.title).catch(() => {});
    } catch (e) {
      draftStore.setError(e instanceof Error ? e.message : 'Could not open album.');
    } finally {
      draftStore.setBusy(false);
    }
  }

  function resumeDraft(d: Draft): void {
    draftStore.setError(null);
    try {
      draftStore.setDraft(d);
    } catch (e) {
      // a resume failure must never fail silently — the Welcome screen
      // renders this message below itself
      draftStore.setError(e instanceof Error ? e.message : 'Could not resume draft.');
    }
  }

  function reset(): void {
    const stale = encodeStaging.get();
    if (stale) {
      setEncodeStaging(null);
      void api.cleanupStaging(stale);
    }
    draftStore.clear();
  }

  onMount(() => {
    // Cmd/Ctrl+O opens an album from anywhere in the app
    const onKey = (e: KeyboardEvent): void => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'o') {
        e.preventDefault();
        if (!busy.get()) void (async () => {
          const dir = await api.pickDirectory();
          if (dir) void openAlbum(dir);
        })();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
</script>

<svelte:head>
  <title>MusicPack Author</title>
</svelte:head>

<div class="topnav">
  <span class="brand">MusicPack Author</span>
</div>

<BackendBanner />

<main class="main">
  {#if $busy && !$draft}
    <div class="spinner" role="status"></div>
  {:else if $draft}
    <AlbumAuthoring onReset={reset} />
  {:else}
    <Welcome onOpen={openAlbum} onResume={resumeDraft} />
    {#if $error}
      <div class="error-banner">{$error}</div>
    {/if}
  {/if}
</main>

<StatusBar />
