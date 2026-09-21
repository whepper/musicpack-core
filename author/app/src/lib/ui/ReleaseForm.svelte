<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { draft, draftStore } from '../bootstrap';
  import { encodeStaging } from '../authoring-state';
  import { MEDIUM_FORMATS, RELEASE_TYPES, SOURCE_TYPES, type Artist } from '../types';


  function genreList(): string {
    return (draft.get()?.album.genres ?? []).join(', ');
  }

  function setGenres(raw: string): void {
    draftStore.updateAlbum((a) => {
      const genres = raw
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean);
      if (genres.length > 0) a.genres = genres;
      else delete a.genres;
    });
  }

  function setArtist(index: number, patch: Partial<Artist>): void {
    draftStore.updateAlbum((a) => {
      const artist = a.artists[index];
      if (artist) Object.assign(artist, patch);
    });
  }

  function addArtist(): void {
    draftStore.updateAlbum((a) => {
      a.artists.push({ name: '' });
    });
  }

  function removeArtist(index: number): void {
    draftStore.updateAlbum((a) => {
      a.artists.splice(index, 1);
    });
  }
</script>

{#if $draft}
<fieldset disabled={$encodeStaging !== null} style="border:0;padding:0;margin:0;min-inline-size:0">
<div class="section">
  <h2>Release</h2>
  <div class="form-grid">
    <div class="field">
      <label for="f-title">Album title</label>
      <input
        id="f-title"
        type="text"
        value={$draft.album.title}
        oninput={(e) =>
          draftStore.updateAlbum((a) => (a.title = e.currentTarget.value))}
      />
    </div>
    <div class="field">
      <label for="f-type">Release type</label>
      <select
        id="f-type"
        value={$draft.album.releaseType ?? ''}
        onchange={(e) =>
          draftStore.updateAlbum((a) => {
            const v = e.currentTarget.value;
            if (v) a.releaseType = v;
            else delete a.releaseType;
          })}
      >
        <option value="">—</option>
        {#each RELEASE_TYPES as rt}
          <option value={rt}>{rt}</option>
        {/each}
      </select>
    </div>
    <div class="field">
      <label for="f-orig">Original release</label>
      <input
        id="f-orig"
        type="date"
        value={$draft.album.originalReleaseDate ?? ''}
        onchange={(e) =>
          draftStore.updateAlbum((a) => {
            const v = e.currentTarget.value;
            if (v) a.originalReleaseDate = v;
            else delete a.originalReleaseDate;
          })}
      />
    </div>
    <div class="field">
      <label for="f-genres">Genres</label>
      <input id="f-genres" type="text" value={genreList()} onchange={(e) => setGenres(e.currentTarget.value)} />
    </div>
  </div>

  <div class="field">
    <span class="smallcaps">Artists</span>
    {#each $draft.album.artists as artist, i}
      <div class="form-grid" style="margin-bottom: 8px">
        <input
          type="text"
          value={artist.name}
          aria-label="Artist name"
          placeholder="Artist name"
          oninput={(e) => setArtist(i, { name: e.currentTarget.value })}
        />
        <input
          type="text"
          value={artist.role ?? ''}
          aria-label="Artist role"
          placeholder="role (main, featuring, …)"
          oninput={(e) => setArtist(i, { role: e.currentTarget.value || undefined })}
        />
        <button class="btn ghost" onclick={() => removeArtist(i)}>Remove</button>
      </div>
    {/each}
    <button class="btn ghost" onclick={addArtist}>Add artist</button>
  </div>
</div>

<div class="section">
  <h2>This release / edition</h2>
  <div class="form-grid">
    <div class="field">
      <label for="f-rd">Release date</label>
      <input
        id="f-rd"
        type="date"
        value={$draft.release?.releaseDate ?? ''}
        onchange={(e) =>
          draftStore.updateRelease((r) => {
            const v = e.currentTarget.value;
            if (v) r.releaseDate = v;
            else delete r.releaseDate;
          })}
      />
    </div>
    <div class="field">
      <label for="f-ed">Edition</label>
      <input
        id="f-ed"
        type="text"
        value={$draft.release?.edition ?? ''}
        oninput={(e) => draftStore.updateRelease((r) => (r.edition = e.currentTarget.value || undefined))}
      />
    </div>
    <div class="field">
      <label for="f-country">Country</label>
      <input
        id="f-country"
        type="text"
        value={$draft.release?.country ?? ''}
        placeholder="ISO 3166-1 alpha-2"
        oninput={(e) => draftStore.updateRelease((r) => (r.country = e.currentTarget.value || undefined))}
      />
    </div>
    <div class="field">
      <label for="f-label">Label</label>
      <input
        id="f-label"
        type="text"
        value={$draft.release?.label ?? ''}
        oninput={(e) => draftStore.updateRelease((r) => (r.label = e.currentTarget.value || undefined))}
      />
    </div>
    <div class="field">
      <label for="f-cat">Catalogue number</label>
      <input
        id="f-cat"
        type="text"
        value={$draft.release?.catalogueNumber ?? ''}
        oninput={(e) => draftStore.updateRelease((r) => (r.catalogueNumber = e.currentTarget.value || undefined))}
      />
    </div>
    <div class="field">
      <label for="f-notes">Notes</label>
      <input
        id="f-notes"
        type="text"
        value={$draft.release?.notes ?? ''}
        oninput={(e) => draftStore.updateRelease((r) => (r.notes = e.currentTarget.value || undefined))}
      />
    </div>
  </div>
</div>

<div class="section">
  <h2>Identifiers</h2>
  <div class="form-grid">
    <div class="field">
      <label for="f-rgid">MusicBrainz release-group ID</label>
      <input
        id="f-rgid"
        type="text"
        value={$draft.identifiers?.musicbrainzReleaseGroupId ?? ''}
        oninput={(e) =>
          draftStore.updateIdentifiers((i) => (i.musicbrainzReleaseGroupId = e.currentTarget.value || undefined))}
      />
    </div>
    <div class="field">
      <label for="f-rid">MusicBrainz release ID</label>
      <input
        id="f-rid"
        type="text"
        value={$draft.identifiers?.musicbrainzReleaseId ?? ''}
        oninput={(e) =>
          draftStore.updateIdentifiers((i) => (i.musicbrainzReleaseId = e.currentTarget.value || undefined))}
      />
    </div>
    <div class="field">
      <label for="f-barcode">Barcode</label>
      <input
        id="f-barcode"
        type="text"
        value={$draft.identifiers?.barcode ?? ''}
        oninput={(e) =>
          draftStore.updateIdentifiers((i) => (i.barcode = e.currentTarget.value || undefined))}
      />
    </div>
  </div>
</div>

<div class="section">
  <h2>Source</h2>
  <p class="smallcaps">Where the audio came from — separate from the release/edition above.</p>
  <div class="form-grid">
    <div class="field">
      <label for="f-srctype">Source type</label>
      <select
        id="f-srctype"
        value={$draft.source?.type ?? ''}
        onchange={(e) =>
          draftStore.updateSource((s) => {
            const v = e.currentTarget.value;
            if (v) s.type = v;
            else delete s.type;
          })}
      >
        <option value="">—</option>
        {#each SOURCE_TYPES as st}
          <option value={st}>{st}</option>
        {/each}
      </select>
    </div>
    <div class="field">
      <label for="f-store">Store / service</label>
      <input
        id="f-store"
        type="text"
        value={$draft.source?.store ?? ''}
        oninput={(e) => draftStore.updateSource((s) => (s.store = e.currentTarget.value || undefined))}
      />
    </div>
    <div class="field">
      <label for="f-srcid">Source ID</label>
      <input
        id="f-srcid"
        type="text"
        value={$draft.source?.sourceId ?? ''}
        oninput={(e) => draftStore.updateSource((s) => (s.sourceId = e.currentTarget.value || undefined))}
      />
    </div>
  </div>
</div>

<div class="section">
  <h2>Media</h2>
  {#each $draft.media as medium, di}
    <div class="form-grid" style="margin-bottom: 12px">
      <div class="field">
        <span class="smallcaps">Disc {medium.disc} format</span>
        <select
          value={medium.format ?? ''}
          onchange={(e) =>
            draftStore.updateMedium(di, (m) => {
              const v = e.currentTarget.value;
              if (v) m.format = v;
              else delete m.format;
            })}
        >
          <option value="">—</option>
          {#each MEDIUM_FORMATS as fmt}
            <option value={fmt}>{fmt}</option>
          {/each}
        </select>
      </div>
      <div class="field">
        <span class="smallcaps">Disc {medium.disc} title</span>
        <input
          type="text"
          value={medium.title ?? ''}
          oninput={(e) => draftStore.updateMedium(di, (m) => (m.title = e.currentTarget.value || undefined))}
        />
      </div>
    </div>
  {/each}
</div>
 </fieldset>
{/if}
