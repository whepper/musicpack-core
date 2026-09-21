<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Slim top bar: menu button (mobile), breadcrumb, collection search.
  // The search input is deliberately labelled differently from the shelf's
  // own search field; it deep-links to the shelf with ?q= which the shelf
  // picks up reactively.
  import { library, router, session } from '../bootstrap';

  let { menuOpen, onMenu }: { menuOpen: boolean; onMenu: () => void } = $props();

  const routeStore = router.route;
  const route = $derived($routeStore);
  const sessionState = $derived($session.state);

  let q = $state('');

  interface Crumb {
    label: string;
    href?: string;
  }

  interface AlbumCrumb {
    title: string;
    artist: string;
    artistId?: number;
  }

  let albumInfo = $state<AlbumCrumb | null>(null);
  let artistName = $state<string | null>(null);

  $effect(() => {
    if (route.name !== 'album') albumInfo = null;
    if (route.name !== 'artist') artistName = null;
    if (route.name === 'album') {
      const id = route.params.id ?? '';
      void library
        .albumDetail(id)
        .then((d) => {
          albumInfo = {
            title: d.album.title,
            artist: d.album.artists[0]?.name ?? '',
            artistId: d.album.artists[0]?.id,
          };
        })
        .catch(() => (albumInfo = null));
    } else if (route.name === 'artist') {
      const id = route.params.id ?? '';
      void library
        .artistDetail(id)
        .then((d) => (artistName = d.name))
        .catch(() => (artistName = null));
    }
  });

  const crumbs = $derived.by(() => {
    const libraryCrumb: Crumb = { label: 'Library', href: '/albums' };
    switch (route.name) {
      case 'albums':
        return [libraryCrumb];
      case 'album':
        return [
          libraryCrumb,
          albumInfo?.artist ? { label: albumInfo.artist, href: albumInfo.artistId ? `/artists/${albumInfo.artistId}` : undefined } : null,
          albumInfo ? { label: albumInfo.title } : null,
        ].filter((c): c is Crumb => c !== null);
      case 'artists':
        return [libraryCrumb, { label: 'Artists' }];
      case 'artist':
        return [libraryCrumb, { label: 'Artists', href: '/artists' }, ...(artistName ? [{ label: artistName }] : [])];
      case 'queue':
        return [{ label: 'Now Playing' }];
      case 'settings':
        return [{ label: 'Settings' }];
      default:
        return [];
    }
  });

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    const term = q.trim();
    router.replace(term ? `/search?q=${encodeURIComponent(term)}` : '/search');
  }
</script>

<header class="topbar">
  <button
    class="topbar-menu-btn"
    aria-label={menuOpen ? 'Close navigation' : 'Open navigation'}
    aria-expanded={menuOpen}
    onclick={onMenu}
  >
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
      <path d="M4 7h16M4 12h16M4 17h16" />
    </svg>
  </button>

  {#if crumbs.length > 0}
    <nav class="breadcrumb" aria-label="Breadcrumb">
      <ol>
        {#each crumbs as crumb, i (crumb.label)}
          <li>
            {#if crumb.href && i < crumbs.length - 1}
              <a href={crumb.href}>{crumb.label}</a>
            {:else}
              <span aria-current={i === crumbs.length - 1 ? 'page' : undefined}>{crumb.label}</span>
            {/if}
          </li>
        {/each}
      </ol>
    </nav>
  {/if}

  <form class="search-field topbar-search" role="search" onsubmit={submit}>
    <span class="search-glyph" aria-hidden="true">⌕</span>
    <input type="search" placeholder="Search MusicPack" aria-label="Search MusicPack" bind:value={q}>
  </form>

  {#if sessionState === 'offline'}
    <span class="offline-chip smallcaps" role="status">Offline</span>
  {/if}
</header>
