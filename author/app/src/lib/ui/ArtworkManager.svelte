<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { api, draft, draftStore } from '../bootstrap';
  import { ARTWORK_ROLES } from '../types';
  import { encodeStaging } from '../authoring-state';

  let { onChange }: { onChange?: () => void } = $props();


  let addRole = $state('front');
  let note = $state<string | null>(null);

  // Row thumbnails, keyed by the artwork entry's relative path. Read failures
  // are recorded so a bad file never triggers a fetch loop; those rows fall
  // back to showing the filename text.
  let thumbs = $state<Record<string, string>>({});
  let failed = $state<Record<string, true>>({});

  $effect(() => {
    const d = $draft;
    if (!d) return;
    for (const art of d.artwork) {
      const rel = art.path;
      if (!rel || thumbs[rel] || failed[rel]) continue;
      void api.readImage(`${d.sourceRoot}/${rel}`)
        .then((img) => {
          thumbs[rel] = `data:${img.mime};base64,${img.dataBase64}`;
        })
        .catch(() => {
          failed[rel] = true;
        });
    }
  });

  function relUnderRoot(abs: string): string | null {
    const root = (draft.get()?.sourceRoot ?? '').replace(/\/+$/, '');
    if (abs.startsWith(root + '/')) return abs.slice(root.length + 1);
    return null;
  }

  async function pickArtwork(): Promise<string | null> {
    const file = await api.pickImageFile();
    if (!file) return null;
    const rel = relUnderRoot(file);
    if (!rel) {
      note = 'The image must be inside the album directory for now.';
      return null;
    }
    note = null;
    return rel;
  }

  async function changeFront(): Promise<void> {
    const rel = await pickArtwork();
    if (!rel) return;
    draftStore.setArtwork([
      ...(draft.get()?.artwork ?? []).filter((a) => a.role !== 'front'),
      { role: 'front', path: rel },
    ]);
    onChange?.();
  }

  async function addArtwork(): Promise<void> {
    const rel = await pickArtwork();
    if (!rel) return;
    draftStore.setArtwork([...(draft.get()?.artwork ?? []), { role: addRole, path: rel }]);
    onChange?.();
  }

  function removeArtwork(index: number): void {
    const next = (draft.get()?.artwork ?? []).slice();
    next.splice(index, 1);
    draftStore.setArtwork(next);
    onChange?.();
  }

  async function addAsset(kind: 'booklet' | 'lyrics' | 'extras'): Promise<void> {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const picked = await open({ multiple: false });
    if (typeof picked !== 'string' || !picked) return;
    const rel = relUnderRoot(picked);
    if (!rel) {
      note = 'The file must be inside the album directory for now.';
      return;
    }
    note = null;
    draftStore.setAssets(kind, [...(draft.get()?.[kind] ?? []), { path: rel }]);
  }
</script>

{#if $draft}
<fieldset disabled={$encodeStaging !== null} style="border:0;padding:0;margin:0;min-inline-size:0">
{#if note}<div class="error-banner" style="margin: 8px 0">{note}</div>{/if}

<h3 class="disc-title">Artwork</h3>
<div class="artwork-row">
  <button class="btn ghost" onclick={changeFront}>Change front artwork…</button>
  <span class="meta">Replaces the front cover with an image inside the album directory.</span>
</div>
{#each $draft.artwork as art, i}
  <div class="artwork-row">
    <span class="thumb">
      {#if art.path && thumbs[art.path]}
        <img src={thumbs[art.path]} alt={`${art.role} artwork`} />
      {:else if art.path}
        {art.path.split('/').pop()}
      {:else}
        embedded
      {/if}
    </span>
    <span class="role">{art.role}</span>
    <span class="meta">
      {#if art.path}
        {art.path}
      {:else}
        embedded in {art.sourceAudio} · extracted at build
      {/if}
    </span>
    <button class="btn ghost" onclick={() => removeArtwork(i)}>Remove</button>
  </div>
{/each}
{#if $draft.artwork.length === 0}
  <p class="muted smallcaps">No artwork yet.</p>
{/if}
<div class="artwork-row">
  <select bind:value={addRole} aria-label="Artwork role">
    {#each ARTWORK_ROLES as role}
      <option value={role}>{role}</option>
    {/each}
  </select>
  <button class="btn ghost" onclick={addArtwork}>Add artwork…</button>
</div>

<h3 class="disc-title">Booklet, lyrics &amp; extras</h3>
<div class="artwork-row">
  <span class="role">Booklet</span>
  <span class="meta">{#each $draft.booklet as b}({b.path}) {/each}</span>
  <button class="btn ghost" onclick={() => addAsset('booklet')}>Add…</button>
</div>
<div class="artwork-row">
  <span class="role">Lyrics</span>
  <span class="meta">{#each $draft.lyrics as l}({l.path}) {/each}</span>
  <button class="btn ghost" onclick={() => addAsset('lyrics')}>Add…</button>
</div>
<div class="artwork-row">
  <span class="role">Extras</span>
  <span class="meta">{#each $draft.extras as x}({x.path}) {/each}</span>
  <button class="btn ghost" onclick={() => addAsset('extras')}>Add…</button>
</div>
 </fieldset>
{/if}
