<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
  import { api } from '../bootstrap';
  import type { Draft, RecentAlbum } from '../types';

  let { onOpen, onResume }: {
    onOpen: (path: string) => void;
    onResume: (draft: Draft) => void;
  } = $props();

  let over = $state(false);
  let savedDraft = $state<Draft | null>(null);
  let recents = $state<RecentAlbum[]>([]);
  let dismissedResume = $state(false);

  onMount(() => {
    void loadSession();
    // Tauri delivers drag/drop as an event with absolute file paths; a plain
    // HTML5 drop cannot reveal full paths. Guard for non-Tauri contexts.
    try {
      const unlisten = getCurrentWebviewWindow().onDragDropEvent((event) => {
        if (event.payload.type === 'over') {
          over = true;
        } else if (event.payload.type === 'drop') {
          over = false;
          const first = event.payload.paths[0];
          if (first) onOpen(first);
        } else {
          over = false;
        }
      });
      return () => {
        void unlisten.then((fn) => fn());
      };
    } catch {
      return undefined;
    }
  });

  async function loadSession(): Promise<void> {
    try {
      const raw = await api.draftLoad();
      if (raw) {
        const d = JSON.parse(raw) as Draft;
        if (d?.sourceRoot && Array.isArray(d.media)) savedDraft = d;
      }
    } catch {
      /* no resume offer when the session file is absent or unreadable */
    }
    try {
      recents = await api.recentsList();
    } catch {
      recents = [];
    }
  }

  async function choose(): Promise<void> {
    const dir = await api.pickDirectory();
    if (dir) onOpen(dir);
  }

  function resume(): void {
    if (!savedDraft) return;
    // savedDraft is a $state deep proxy; unwrap it before handing it to the
    // draft store — setDraft() uses structuredClone(), which throws on proxies.
    onResume($state.snapshot(savedDraft));
  }

  function discardSaved(): void {
    dismissedResume = true;
    void api.draftClear().catch(() => {});
  }

  function describe(path: string): string {
    return path.split('/').filter(Boolean).slice(-2).join(' / ');
  }
</script>

<section class="welcome">
  <h1>Author an album</h1>
  <p class="muted">
    Turn a tagged Musepack album — the output of <em>flac2mpc</em> — into a curated
    <span class="smallcaps">.mpack</span> release. Existing
    <span class="smallcaps">.mpack</span> packages can be opened and edited too.
  </p>

  {#if savedDraft && !dismissedResume}
    <div class="resume-card">
      <div>
        <p class="smallcaps">Unfinished draft</p>
        <p class="resume-title">
          {savedDraft.album.title || 'Untitled album'}
          {#if savedDraft.sourceRoot}
            <span class="muted smallcaps"> · {describe(savedDraft.sourceRoot)}</span>
          {/if}
        </p>
      </div>
      <div class="resume-actions">
        <button class="btn" onclick={resume}>Resume draft</button>
        <button class="btn ghost" onclick={discardSaved}>Discard</button>
      </div>
    </div>
  {/if}

  <div
    class="drop"
    class:over
    role="button"
    tabindex="0"
    aria-label="Choose an album directory"
    ondragover={(e) => e.preventDefault()}
    ondrop={(e) => e.preventDefault()}
    onkeydown={(e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        void choose();
      }
    }}
  >
    <p>Drop an album directory or a <span class="smallcaps">.mpack</span> package here, or</p>
    <button class="btn" onclick={choose}>Choose album or package…</button>
  </div>

  {#if recents.length > 0}
    <div class="recents">
      <p class="smallcaps">Recent albums</p>
      <ul>
        {#each recents.slice(0, 5) as r}
          <li>
            <button class="recent-link" onclick={() => onOpen(r.path)}>
              {r.title ?? describe(r.path)}
            </button>
            <span class="muted smallcaps">{describe(r.path)}</span>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</section>

<style>
  .resume-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    width: min(480px, 100%);
    margin: 0 auto 1rem;
    padding: 0.85rem 1.1rem;
    background: var(--surface);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-card);
  }
  .resume-title {
    margin: 0.15rem 0 0;
    font-family: var(--serif);
    font-size: 17px;
  }
  .resume-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }
  .recents {
    margin-top: 1.4rem;
    width: min(480px, 100%);
    margin-left: auto;
    margin-right: auto;
  }
  .recents ul {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0;
  }
  .recents li {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
    padding: 3px 0;
  }
  .recent-link {
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    color: var(--accent);
    cursor: pointer;
    text-align: left;
  }
  .recent-link:hover {
    text-decoration: underline;
  }
</style>
