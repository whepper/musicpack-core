<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { api, draft, draftStore } from '../bootstrap';
  import type { IdentifyCandidate, IdentifyResult } from '../types';
  import { formatDate } from '../format';
  import { encodeStaging } from '../authoring-state';
  import ConfidenceBadge from './ConfidenceBadge.svelte';

  let {
    onIdentified,
    candidates,
  }: { onIdentified: (r: IdentifyResult) => void; candidates: IdentifyCandidate[] | null } =
    $props();


  let mbid = $state('');
  let barcode = $state('');
  let searching = $state(false);
  let applying = $state(false);
  let message = $state<string | null>(null);

  // MBIDs are 36-char UUIDs; barcodes are digit-only (EAN-13/GTIN).
  function validMbid(v: string): boolean {
    return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(v);
  }

  function validBarcode(v: string): boolean {
    return /^[0-9]+$/.test(v);
  }

  async function applyMbid(): Promise<void> {
    const d = draft.get();
    if (!d || !mbid.trim()) return;
    if (!validMbid(mbid.trim())) {
      message = 'That is not a valid MusicBrainz release ID (expected a UUID).';
      return;
    }
    applying = true;
    message = null;
    try {
      onIdentified(await api.identifyDraft(d, { mbid: mbid.trim() }));
      message = 'Applied.';
    } catch (e) {
      message = e instanceof Error ? e.message : 'Lookup failed.';
    } finally {
      applying = false;
    }
  }

  async function searchBarcode(): Promise<void> {
    const d = draft.get();
    if (!d || !barcode.trim()) return;
    if (!validBarcode(barcode.trim())) {
      message = 'A barcode contains digits only (e.g. 198704979941).';
      return;
    }
    searching = true;
    message = null;
    try {
      onIdentified(await api.identifyDraft(d, { barcode: barcode.trim() }));
    } catch (e) {
      message = e instanceof Error ? e.message : 'Search failed.';
    } finally {
      searching = false;
    }
  }

  async function applyCandidate(c: IdentifyCandidate): Promise<void> {
    if (!c.releaseId) return;
    const d = draft.get();
    if (!d) return;
    applying = true;
    message = null;
    try {
      onIdentified(await api.identifyDraft(d, { mbid: c.releaseId }));
    } catch (e) {
      message = e instanceof Error ? e.message : 'Apply failed.';
    } finally {
      applying = false;
    }
  }
</script>

{#if $draft}
<p class="smallcaps">Identity confidence is shown honestly: a probable match is never presented as authoritative.</p>

<div class="form-grid">
  <div class="field">
    <label for="f-mbid">MusicBrainz release ID</label>
    <input
      id="f-mbid"
      type="text"
      placeholder="11111111-2222-3333-4444-555555555555"
      bind:value={mbid}
      disabled={$encodeStaging !== null}
      onkeydown={(e) => {
        if (e.key === 'Enter') void applyMbid();
      }}
    />
  </div>
  <div class="field">
    <label for="f-barcode">…or search by barcode</label>
    <div style="display:flex; gap:8px">
      <input
        id="f-barcode"
        type="text"
        placeholder="0198765432197"
        bind:value={barcode}
        disabled={$encodeStaging !== null}
        onkeydown={(e) => {
          if (e.key === 'Enter') void searchBarcode();
        }}
      />
    </div>
  </div>
</div>
<div class="artwork-row">
  <button class="btn ghost" onclick={applyMbid} disabled={$encodeStaging !== null || applying || !mbid.trim()}>
    {applying ? 'Applying…' : 'Apply release ID'}
  </button>
  <button class="btn ghost" onclick={searchBarcode} disabled={$encodeStaging !== null || searching || !barcode.trim()}>
    {searching ? 'Searching…' : 'Search barcode'}
  </button>
  <ConfidenceBadge confidence={$draft.identity?.confidence} />
  {#if message}<span class="meta">{message}</span>{/if}
</div>

{#if candidates}
  <h3 class="disc-title">Candidates</h3>
  {#each candidates as c}
    <div class="candidate">
      <div class="ct">
        <div class="t">{c.title ?? 'Untitled'}</div>
        <div class="m">
          {c.artist ?? ''}
          {#if c.date} · {formatDate(c.date)}{/if}
          {#if c.country} · {c.country}{/if}
          {#if c.barcode} · {c.barcode}{/if}
        </div>
      </div>
      <ConfidenceBadge confidence={c.confidence} />
      <button class="btn ghost" onclick={() => applyCandidate(c)} disabled={$encodeStaging !== null || !c.releaseId}>
        Apply
      </button>
    </div>
  {/each}
{/if}
{/if}
