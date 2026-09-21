<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album / edition screen — the primary UI v2 reference. Hero + info
  // tiles + seven deep-linkable sections (Overview / Tracks / Edition /
  // Audio / Analysis / Metadata / Package) and the contextual right rail.
  //
  // This page OWNS: data loading, the selected edition, section routing,
  // and the queue actions. Derived release facts live in `album-facts.ts`
  // and each section renders from `ui/album/*` components; nothing here
  // invents data.
  import { library, player, playerModel, queue, router, offline, offlineStates, session, audioPreference } from '../bootstrap';
  import type { AudioPreference } from '../state/representation-selection';
  import DownloadControl from './DownloadControl.svelte';
  import Artwork from './Artwork.svelte';
  import ArtworkViewer from './ArtworkViewer.svelte';
  import EditionSelector from './EditionSelector.svelte';
  import SectionTabs from './SectionTabs.svelte';
  import InfoTile from './InfoTile.svelte';
  import ContextRail from './ContextRail.svelte';
  import ErrorView from './ErrorView.svelte';
  import { codecName, countryName, formatDate, fmtTime, yearOf } from '../format';
  import { identityChip, offlineStateChip } from './status';
  import { albumNormalizationGain, heroArtwork, releaseFacts, waveformPeekTrack } from './album-facts';
  import { tracksOfRelease } from '../state/queue';
  import AlbumOverviewSection from './album/AlbumOverviewSection.svelte';
  import AlbumTracksSection from './album/AlbumTracksSection.svelte';
  import AlbumEditionSection from './album/AlbumEditionSection.svelte';
  import AlbumAudioSection from './album/AlbumAudioSection.svelte';
  import AlbumAnalysisSection from './album/AlbumAnalysisSection.svelte';
  import AlbumMetadataSection from './album/AlbumMetadataSection.svelte';
  import AlbumPackageSection from './album/AlbumPackageSection.svelte';
  import { onDestroy, onMount } from 'svelte';

  let { albumId, releaseParam }: { albumId: string; releaseParam?: string } = $props();

  let detail = $state<Awaited<ReturnType<typeof library.albumDetail>> | null>(null);
  let rel = $state<Awaited<ReturnType<typeof library.releaseDetail>> | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let selectedId = $state(-1);
  let viewerOpen = $state(false);
  // Transient "✓ Added" confirmation for the album-level queue action; the
  // timer is cleared on destroy so no state update fires post-unmount.
  let albumAdded = $state(false);
  let addedTimer: ReturnType<typeof setTimeout> | undefined;
  onDestroy(() => {
    if (addedTimer) clearTimeout(addedTimer);
  });

  const currentTrackId = $derived($playerModel.current?.track.id);
  const routeStore = router.route;
  const sectionParam = $derived($routeStore.query.get('section') ?? 'overview');

  const SECTIONS = [
    { id: 'overview', label: 'Overview' },
    { id: 'tracks', label: 'Tracks' },
    { id: 'edition', label: 'Edition' },
    { id: 'audio', label: 'Audio' },
    { id: 'analysis', label: 'Analysis' },
    { id: 'metadata', label: 'Metadata' },
    { id: 'package', label: 'Package' },
  ];
  const section = $derived(SECTIONS.some((s) => s.id === sectionParam) ? sectionParam : 'overview');

  onMount(() => {
    void load();
  });

  async function load(): Promise<void> {
    loading = true;
    error = null;
    detail = null;
    rel = null;
    try {
      detail = await library.albumDetail(albumId);
      const releases = detail.releases;
      if (releases.length === 0) throw new Error('This album has no playable releases in the collection.');
      const paramId = releaseParam ? Number(releaseParam) : NaN;
      selectedId =
        !Number.isNaN(paramId) && releases.some((r) => r.id === paramId)
          ? paramId
          : library.selectedRelease(detail.album.id, releases);
      library.selectRelease(detail.album.id, selectedId);
      await loadRelease(selectedId);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not open this album.';
    } finally {
      loading = false;
    }
  }

  async function loadRelease(id: number): Promise<void> {
    rel = await library.releaseDetail(id);
    // D2 update check (online only): flag a committed download whose
    // hashes no longer match the server. Flagging is all the domain does;
    // replacement stays a user action on the Download control.
    if (offline.enabled && session.get().state === 'authenticated') {
      void offline.checkForUpdate(rel);
    }
  }

  function sectionHref(id: string): string {
    const base = `/albums/${albumId}?release=${selectedId}`;
    return id === 'overview' ? base : `${base}&section=${id}`;
  }

  function setSection(id: string): void {
    router.replace(sectionHref(id));
  }

  async function selectEdition(id: number): Promise<void> {
    selectedId = id;
    library.selectRelease(detail?.album.id ?? 0, id);
    const base = `/albums/${albumId}?release=${id}`;
    router.replace(section === 'overview' ? base : `${base}&section=${section}`);
    await loadRelease(id);
  }

  function playAlbum(): void {
    if (!rel || !detail) return;
    const artist = detail.album.artists[0]?.name ?? '';
    void player.playAlbum(rel, detail.album.title, artist, 0);
  }

  function playWithPreference(preference?: AudioPreference): void {
    if (!rel || !detail) return;
    if (preference) audioPreference.set(preference);
    const artist = detail.album.artists[0]?.name ?? '';
    void player.playAlbum(rel, detail.album.title, artist, 0);
  }

  function shuffleAlbum(): void {
    if (!rel || !detail) return;
    const artist = detail.album.artists[0]?.name ?? '';
    player.setShuffle(true);
    // Host owns randomness (core purity law): pick the shuffled starting
    // track here; the core shuffles only the continuation order.
    const count = tracksOfRelease(rel).length;
    void player.playAlbum(rel, detail.album.title, artist,
      count > 0 ? Math.floor(Math.random() * count) : 0);
  }

  function addToQueue(): void {
    if (!rel || !detail) return;
    const artist = detail.album.artists[0]?.name ?? '';
    queue.addAlbum(rel, detail.album.title, artist);
    albumAdded = true;
    if (addedTimer) clearTimeout(addedTimer);
    addedTimer = setTimeout(() => {
      albumAdded = false;
      addedTimer = undefined;
    }, 2500);
  }

  function playFrom(index: number): void {
    if (!rel || !detail) return;
    const artist = detail.album.artists[0]?.name ?? '';
    void player.playAlbum(rel, detail.album.title, artist, index);
  }

  // ---- derived view data (facts modules own the rules) ------------------
  const facts = $derived(releaseFacts(rel));
  const primary = $derived(facts.primary);
  const primaryTileSub = $derived(facts.primaryTileSub);
  const altGroup = $derived(facts.repGroups[0]);
  const heroArt = $derived(rel ? heroArtwork(rel.artwork) : undefined);
  const title = $derived(detail?.album.title ?? '');
  const artist = $derived(detail?.album.artists.map((a) => a.name).join(', ') ?? '');
  const leadArtist = $derived(detail?.album.artists[0]);
  const identity = $derived(rel ? identityChip(rel.identitySource, rel.identityConfidence) : null);
  const offlineChip = $derived(rel ? offlineStateChip($offlineStates.get(rel.id)?.state) : null);
  const waveTrack = $derived(waveformPeekTrack(rel, facts.allTracks, currentTrackId));
</script>

{#if loading}
  <div class="spinner" role="status" aria-label="Loading album"></div>
{:else if error}
  <ErrorView message={error} detail="It may have been removed from the library, or the server is unreachable." />
{:else if detail && rel}
  <div class="album-layout">
    <div class="album-main">
      <article>
        <header class="album-hero">
          <div class="cover">
            <button style="width:100%;padding:0;display:block" onclick={() => (viewerOpen = true)}
              aria-label={`View artwork for ${title}`}>
              <Artwork src={heroArt?.url} alt={`${title} — front cover`} label={title} />
            </button>
          </div>
          <div class="album-heading">
            <p class="eyebrow">{detail.album.releaseType ?? 'album'}</p>
            <h1>{title}</h1>
            {#if leadArtist}
              <p class="artist"><a href={`/artists/${leadArtist.id}`}>{artist}</a></p>
            {:else}
              <p class="artist">{artist}</p>
            {/if}
            {#if detail.album.genres?.length}
              <p class="genre-pills">
                {#each detail.album.genres as genre}
                  <span class="genre-pill">{genre}</span>
                {/each}
              </p>
            {/if}
            <p class="edition-line">
              {#if rel.releaseDate}{yearOf(rel.releaseDate)}{/if}
              {#if rel.label} · {rel.label}{/if}
              {#if rel.catalogueNumber} · {rel.catalogueNumber}{/if}
            </p>
            {#if rel.country || facts.mediums || rel.edition}
              <p class="edition-line">
                {#if rel.country}{countryName(rel.country)}{/if}
                {#if facts.mediums} · {facts.mediums}{/if}
                {#if rel.edition} · {rel.edition}{/if}
              </p>
            {/if}
            {#if identity || rel.sourceType}
              <p class="badge-row">
                {#if identity}<span class="badge">{#if identity.mark}<span aria-hidden="true">{identity.mark}</span>{/if}{identity.label}</span>{/if}
                {#if rel.sourceType}<span class="badge">{rel.sourceType}</span>{/if}
              </p>
            {/if}
            {#if rel.releaseDate || facts.totalDuration > 0}
              <p class="edition-line dates">
                {#if rel.releaseDate}{formatDate(rel.releaseDate)}{/if}
                {#if facts.totalDuration > 0} · {fmtTime(facts.totalDuration)}{/if}
              </p>
            {/if}
            <div class="hero-actions">
              <button class="btn" onclick={playAlbum}>▶ Play album</button>
              <button class="btn ghost" onclick={shuffleAlbum}>⤨ Shuffle</button>
              {#if rel}
                <DownloadControl release={rel} states={offline.states} downloads={offline.downloads} />
              {/if}
              <button
                class="btn ghost album-add"
                aria-label={albumAdded ? 'Album added to queue' : 'Add album to queue'}
                onclick={addToQueue}
              >{albumAdded ? '✓ Added' : '+ Add to Queue'}</button>
              <button class="btn ghost" onclick={() => (viewerOpen = true)}>View artwork</button>
            </div>
          </div>
        </header>

        <div class="info-tiles" role="group" aria-label="Audio summary">
          {#if primary}
            <InfoTile
              title={codecName(primary.codec.codec)}
              sub={primaryTileSub}
              href={sectionHref('audio')}
            />
            {#if altGroup}
              <InfoTile title={altGroup.title} sub={altGroup.sub} href={sectionHref('audio')} />
            {/if}
          {/if}
          <InfoTile
            title={`${facts.trackCount} Tracks`}
            sub={facts.discCount > 1 ? `${facts.discCount} Discs` : 'Single disc'}
            href={sectionHref('tracks')}
          />
        </div>

        <SectionTabs sections={SECTIONS} active={section} onSelect={setSection} />

        <div class="album-section" role="tabpanel" id={`panel-${section}`} aria-labelledby={`tab-${section}`}>
          {#if section === 'overview'}
            <AlbumOverviewSection {rel} {facts} currentTrackId={currentTrackId} onPlay={playFrom} audioHref={sectionHref('audio')} />
          {:else if section === 'tracks'}
            <AlbumTracksSection {rel} currentTrackId={currentTrackId} onPlay={playFrom} />
          {:else if section === 'edition'}
            <AlbumEditionSection {detail} {rel} {facts} {selectedId} onSelect={selectEdition} />
          {:else if section === 'audio'}
            <AlbumAudioSection {facts} />
          {:else if section === 'analysis'}
            <AlbumAnalysisSection {rel} {facts} currentTrackId={currentTrackId} onPlay={playFrom} />
          {:else if section === 'metadata'}
            <AlbumMetadataSection {detail} {rel} {facts} />
          {:else if section === 'package'}
            <AlbumPackageSection {rel} {facts} {offlineChip} />
          {/if}
        </div>
      </article>
    </div>

    <ContextRail
      {detail}
      {rel}
      waveTrack={waveTrack}
      offlineState={$offlineStates.get(rel.id)?.state}
      onSelectEdition={selectEdition}
      onPlay={playWithPreference}
      onSection={setSection}
    />
  </div>

  {#if viewerOpen}
    <ArtworkViewer {title} artwork={rel.artwork} onclose={() => (viewerOpen = false)} />
  {/if}
{:else}
  <ErrorView message="Album not found" detail="It may have been removed from the collection." />
{/if}
