<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // /tracks/:id — the track-detail surface. Editorial, then technical:
  // hero (artwork + context + playback), then a single editorial inspector
  // that exposes the depth of the .mpack without becoming a developer
  // screen. Every value shown is real API or real package data.
  //
  // This page OWNS: data loading, the play/queue actions, and navigation.
  // Derived facts live in `track-facts.ts`; the sections and the right
  // rail render from `ui/track/*` components.
  import { api, library, player, playerModel, queue, audioPreference, offline, offlineStates, router } from '../bootstrap';
  import type { AudioPreference } from '../state/representation-selection';
  import type { ReleaseDetail, TrackDetail } from '../api/types';
  import Artwork from './Artwork.svelte';
  import DownloadControl from './DownloadControl.svelte';
  import ErrorView from './ErrorView.svelte';
  import InfoTile from './InfoTile.svelte';
  import StatusChip from './StatusChip.svelte';
  import { heroArtwork } from './album-facts';
  import {
    identityChip,
    offlineStateChip,
    packageStatusChip,
    verifyStatusChip,
  } from './status';
  import { codecLabel, codecName, fmtTime } from '../format';
  import { tracksOfRelease } from '../state/queue';
  import { representationRows, seekTimeline, trackFacts, trackNormalizationGains } from './track-facts';
  import TrackSeekSection from './track/TrackSeekSection.svelte';
  import TrackLyricsSection from './track/TrackLyricsSection.svelte';
  import TrackAudioSection from './track/TrackAudioSection.svelte';
  import TrackAnalysisSection from './track/TrackAnalysisSection.svelte';
  import TrackMetadataSection from './track/TrackMetadataSection.svelte';
  import TrackPackageSection from './track/TrackPackageSection.svelte';
  import TrackRail from './track/TrackRail.svelte';
  import { onMount } from 'svelte';

  let { trackId }: { trackId: string } = $props();

  let detail = $state<TrackDetail | null>(null);
  let rel = $state<ReleaseDetail | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);

  onMount(() => {
    void load();
  });

  async function load(): Promise<void> {
    loading = true;
    error = null;
    detail = null;
    rel = null;
    try {
      const d = await api.trackDetail(trackId);
      detail = d;
      // Pull the owning release for the editorial inspector (status, hashes,
      // provenance, package counts, edition facts). Refreshed copy bypasses
      // the library cache so a stale release JSON never lingers on this page.
      rel = await library.refreshRelease(d.context.releaseId);
      if (offline.enabled) void offline.checkForUpdate(rel);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not open this track.';
    } finally {
      loading = false;
    }
  }

  // ---- derived view data (facts modules own the rules) ------------------
  const t = $derived(detail?.track ?? null);
  const ctx = $derived(detail?.context);
  const facts = $derived(trackFacts(detail, rel));
  const repRows = $derived(representationRows(t));
  const heroArt = $derived(rel ? heroArtwork(rel.artwork) : undefined);
  const identity = $derived(rel ? identityChip(rel.identitySource, rel.identityConfidence) : null);
  const offlineChip = $derived(rel ? offlineStateChip($offlineStates.get(rel.id)?.state) : null);
  const gains = $derived(trackNormalizationGains(t, rel));
  const timeline = $derived(
    seekTimeline(
      {
        currentTrackId: $playerModel.current?.track.id,
        currentTrackStartSeconds: $playerModel.currentTrackStartSeconds,
        currentTrackDurationSeconds: $playerModel.currentTrackDurationSeconds,
        positionSeconds: $playerModel.positionSeconds,
      },
      t,
    ),
  );

  const albumUrl = $derived(ctx ? `/albums/${ctx.albumId}?release=${ctx.releaseId}` : '/');
  // Deep links reuse the canonical album route with a section parameter.
  const goPackage = $derived(ctx ? `${albumUrl}&section=package` : null);
  const goAudio = $derived(ctx ? `${albumUrl}&section=audio` : null);
  const goAnalysis = $derived(ctx ? `${albumUrl}&section=analysis` : null);
  const goEdition = $derived(ctx ? `${albumUrl}&section=edition` : null);
  const goMetadata = $derived(ctx ? `${albumUrl}&section=metadata` : null);
  function navSection(href: string | null): void {
    if (href) router.go(href);
  }

  // ---- actions ---------------------------------------------------------
  function play(): void {
    if (!rel || !detail) return;
    const startIndex = facts.trackIndexInRelease >= 0 ? facts.trackIndexInRelease : 0;
    const artist = facts.leadArtist?.name ?? '';
    void player.playAlbum(rel, detail.track.title, artist, startIndex);
  }
  function playRepresentation(preference?: AudioPreference): void {
    if (!rel || !detail) return;
    if (preference) audioPreference.set(preference);
    const startIndex = facts.trackIndexInRelease >= 0 ? facts.trackIndexInRelease : 0;
    const artist = facts.leadArtist?.name ?? '';
    void player.playAlbum(rel, detail.track.title, artist, startIndex);
  }
  function shuffle(): void {
    if (!rel || !detail) return;
    player.setShuffle(true);
    const count = tracksOfRelease(rel).length;
    const startIndex = count > 0 ? Math.floor(Math.random() * count) : 0;
    void player.playAlbum(rel, detail.track.title, facts.leadArtist?.name ?? '', startIndex);
  }
  function addToQueue(): void {
    if (!rel || !detail) return;
    // addAlbum already applies the existing representation-selection policy
    // and the per-track cover-art fallback, so we route through it.
    queue.addAlbum(rel, rel.album.title, facts.leadArtist?.name ?? '');
  }

  /** Seek callback: waveform seeks arrive album-absolute; the range input
   *  is track-relative. Both funnel through player.seek(album-absolute). */
  function onSeek(albumAbsolute: number, _trackRelative: number): void {
    void player.seek(albumAbsolute);
  }
</script>

{#if loading}
  <div class="spinner" role="status" aria-label="Loading track"></div>
{:else if error}
  <ErrorView message={error} detail="It may have been removed from the collection, or the server is unreachable." />
{:else if detail && t && ctx && rel}
  <div class="track-layout">
    <div class="track-main">
      <article>
        <header class="album-hero">
          <div class="cover">
            <Artwork src={heroArt?.url} alt={`${t.title} — front cover`} label={t.title} />
          </div>
          <div class="album-heading">
            <p class="eyebrow">
              <a href={albumUrl}>{rel.album.title}</a>
              {#if rel.edition} · {rel.edition}{/if}
            </p>
            <h1>{t.title}</h1>
            <p class="artist">
              {#if facts.leadArtist}<a href={`/artists/${facts.leadArtist.id}`}>{facts.leadArtist.name}</a>{:else}Unknown artist{/if}
            </p>
            <p class="edition-line">
              {#if facts.positionLine}
                Disc {facts.positionLine.disc} of {facts.positionLine.of}{#if facts.positionLine.title} — {facts.positionLine.title}{/if}
                · Track {facts.positionLine.pos} of {facts.positionLine.of2}
              {/if}
            </p>
            <p class="edition-line">
              {#if t.duration}{fmtTime(t.duration)} · {/if}
              {facts.primaryFormat || codecLabel(t.codec.codec)}
            </p>
            {#if identity || rel.sourceType}
              <p class="badge-row">
                {#if identity}<span class="badge">{#if identity.mark}<span aria-hidden="true">{identity.mark}</span>{/if}{identity.label}</span>{/if}
                {#if rel.sourceType}<span class="badge">{rel.sourceType}</span>{/if}
              </p>
            {/if}
            <div class="hero-actions">
              <button class="btn" onclick={play}>▶ Play track</button>
              <button class="btn ghost" onclick={shuffle}>⤨ Shuffle album</button>
              {#if repRows.length > 1}
                <button
                  class="btn ghost"
                  aria-label="Play this track with the alternate representation selected"
                  onclick={() => {
                    const alt = repRows.find((r) => r.id !== 'primary');
                    if (!alt) return;
                    playRepresentation({ mode: 'codec', codec: alt.label.toLowerCase() });
                  }}
                >Play alternate</button>
              {/if}
              <button
                class="btn ghost album-add"
                aria-label="Add track to queue"
                onclick={addToQueue}
              >+ Add to Queue</button>
              {#if rel}
                <DownloadControl release={rel} states={offline.states} downloads={offline.downloads} />
              {/if}
            </div>
            <p class="status-row">
              <StatusChip chip={packageStatusChip(rel.packageStatus)} />
              <StatusChip chip={verifyStatusChip(rel.verifyStatus)} />
              {#if offlineChip}<StatusChip chip={offlineChip} />{/if}
            </p>
          </div>
        </header>

        <TrackSeekSection {t} {timeline} positionSeconds={$playerModel.positionSeconds} onSeek={onSeek} />

        {#if t.lyrics && t.lyrics.length > 0}
          <TrackLyricsSection {t} />
        {/if}

        <div class="info-tiles" role="group" aria-label="Track facts">
          <InfoTile
            title={codecName(t.codec.codec)}
            sub={facts.isMpc && t.codec.streamVersion ? `SV${t.codec.streamVersion}` : (facts.primaryFormat || '—')}
            href={goAudio ?? undefined}
          />
          <InfoTile
            title={rel.loudness ? `${rel.loudness.albumLufs.toFixed(1)} LUFS` : '—'}
            sub={rel.loudness ? 'Album loudness' : 'No album loudness'}
            href={goAnalysis ?? undefined}
          />
          <InfoTile
            title={t.loudness ? `${t.loudness.lufs.toFixed(1)} LUFS` : '—'}
            sub={t.loudness ? `${t.loudness.truePeakDb.toFixed(1)} dBTP` : 'No track loudness'}
            href={goAnalysis ?? undefined}
          />
        </div>

        <TrackAudioSection {t} {repRows} />
        <TrackAnalysisSection {t} {rel} {facts} {gains} />
        <TrackMetadataSection {t} {rel} {facts} />
        <TrackPackageSection {rel} {facts} {offlineChip} packageHref={goPackage} onNav={navSection} />
      </article>
    </div>

    <TrackRail
      {t}
      {rel}
      {facts}
      {repRows}
      {albumUrl}
      audioHref={goAudio}
      analysisHref={goAnalysis}
      editionHref={goEdition}
      metadataHref={goMetadata}
      packageHref={goPackage}
      {offlineChip}
      onNav={navSection}
      onPlayRepresentation={playRepresentation}
    />
  </div>
{:else}
  <ErrorView message="Track not found" detail="It may have been removed from the collection." />
{/if}
