<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Per-track lyrics editor (R3.5): attach / replace / remove lyric
  // documents on one draft track, with immediate validation feedback.
  // The draft carries `{path, lang?}` per track (sourceRoot-relative);
  // content hashes are computed at build, never authored. Package-level
  // `Draft.lyrics` (root assets) are managed separately in ArtworkManager
  // and never touched here.
  //
  // The track's refs render from the draft store (ArtworkManager
  // precedent), so store updates re-render without prop plumbing; `disc`
  // and `index` locate the track.
  import { api, draft, draftStore } from '../bootstrap';
  import { encodeStaging } from '../authoring-state';
  import type { LyricsProbeResult, TrackLyricRef } from '../types';

  let {
    disc,
    index,
    sourceRoot,
  }: {
    disc: number;
    index: number;
    sourceRoot: string;
  } = $props();

  let note = $state<string | null>(null);
  // Probe results keyed by ref path (survive lang edits, which replace
  // the ref objects but keep the path).
  let probes = $state<Record<string, LyricsProbeResult>>({});

  const refs = $derived($draft?.media[disc]?.tracks[index]?.lyrics ?? []);
  const disabled = $derived($encodeStaging !== null);

  function relUnderRoot(abs: string): string | null {
    const root = sourceRoot.replace(/\/+$/, '');
    if (abs.startsWith(root + '/')) return abs.slice(root.length + 1);
    return null;
  }

  async function probeRef(ref: TrackLyricRef): Promise<void> {
    try {
      probes[ref.path] = await api.lyricsProbe(`${sourceRoot}/${ref.path}`);
    } catch (e) {
      probes[ref.path] = {
        ok: false,
        error: { message: e instanceof Error ? e.message : 'Probe failed' },
      };
    }
  }

  // Probe-on-display: refs arriving from inspect (or autosave) get their
  // validation state without user action.
  $effect(() => {
    for (const ref of refs) {
      if (!probes[ref.path]) void probeRef(ref);
    }
  });

  function setRefs(next: TrackLyricRef[]): void {
    draftStore.updateTrack(disc, index, next.length > 0 ? { lyrics: next } : { lyrics: undefined });
  }

  function probeSummary(path: string): string | null {
    const p = probes[path];
    if (!p) return null;
    if (!p.ok) return `invalid — ${p.error?.message ?? 'invalid lyrics'}`;
    const kind = p.synced ? 'synced' : 'plain';
    return `${kind} · ${p.lines ?? 0} lines`;
  }

  async function pickRef(): Promise<{ rel: string; abs: string } | null> {
    const picked = await api.pickLyricsFile();
    if (!picked) return null;
    const rel = relUnderRoot(picked);
    if (!rel) {
      note = 'The lyric file must be inside the album directory for now.';
      return null;
    }
    note = null;
    return { rel, abs: picked };
  }

  async function addLyrics(): Promise<void> {
    const picked = await pickRef();
    if (!picked) return;
    const probe = await api.lyricsProbe(picked.abs).catch((e: unknown) => ({
      ok: false as const,
      error: { message: e instanceof Error ? e.message : 'Probe failed' },
    }));
    probes[picked.rel] = probe;
    if (!probe.ok) {
      note = probe.error?.message ?? 'Invalid lyrics file.';
      return;
    }
    if (refs.some((r) => r.path === picked.rel)) {
      note = 'This lyric file is already attached to this track.';
      return;
    }
    setRefs([...refs, { path: picked.rel }]);
  }

  async function replaceLyrics(at: number): Promise<void> {
    const picked = await pickRef();
    if (!picked) return;
    const probe = await api.lyricsProbe(picked.abs).catch((e: unknown) => ({
      ok: false as const,
      error: { message: e instanceof Error ? e.message : 'Probe failed' },
    }));
    probes[picked.rel] = probe;
    if (!probe.ok) {
      note = probe.error?.message ?? 'Invalid lyrics file.';
      return;
    }
    const next = refs.slice();
    next[at] = { path: picked.rel, lang: next[at]?.lang };
    setRefs(next);
  }

  function removeLyrics(at: number): void {
    note = null;
    setRefs(refs.filter((_, i) => i !== at));
  }

  function setLang(at: number, lang: string): void {
    const next = refs.slice();
    const ref = next[at];
    if (!ref) return;
    const trimmed = lang.trim();
    next[at] = trimmed ? { path: ref.path, lang: trimmed } : { path: ref.path };
    setRefs(next);
  }
</script>

<style>
  .track-lyrics {
    margin-top: var(--space-2);
    padding-top: var(--space-2);
    border-top: 1px solid var(--hairline);
  }
  .lyrics-refs {
    list-style: none;
    margin: 0 0 var(--space-2);
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .lyrics-refs li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .lyrics-refs .filename {
    color: var(--text-faint);
    font-size: var(--fs-xxs);
    font-family: var(--mono);
  }
  .lyrics-refs input[type='text'] {
    width: 10em;
  }
</style>

<div class="track-lyrics">
  {#if note}<div class="error-banner" role="alert">{note}</div>{/if}
  {#if refs.length === 0}
    <span class="muted smallcaps">No lyrics.</span>
    <button class="btn ghost" onclick={addLyrics} disabled={disabled}>Add lyrics…</button>
  {:else}
    <ol class="lyrics-refs">
      {#each refs as ref, i (ref.path)}
        <li>
          <span class="filename" title={ref.path}>{ref.path.split('/').pop()}</span>
          {#if ref.lang}<span class="conf">{ref.lang}</span>{/if}
          {#if probeSummary(ref.path)}
            <span class="smallcaps">{probeSummary(ref.path)}</span>
          {/if}
          <input
            type="text"
            aria-label="Lyric language for {ref.path}"
            placeholder="lang (optional)"
            value={ref.lang ?? ''}
            oninput={(e) => setLang(i, e.currentTarget.value)}
            disabled={disabled}
          />
          <button class="btn ghost" onclick={() => replaceLyrics(i)} disabled={disabled}>Replace…</button>
          <button class="btn ghost" onclick={() => removeLyrics(i)} disabled={disabled}>Remove</button>
        </li>
      {/each}
    </ol>
    <button class="btn ghost" onclick={addLyrics} disabled={disabled}>Add another…</button>
  {/if}
</div>
