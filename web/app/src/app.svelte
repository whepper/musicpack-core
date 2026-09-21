<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { router, session } from './lib/bootstrap';
  import AuthGate from './lib/ui/AuthGate.svelte';
  import AppSidebar from './lib/ui/AppSidebar.svelte';
  import TopBar from './lib/ui/TopBar.svelte';
  import AlbumShelf from './lib/ui/AlbumShelf.svelte';
  import AlbumPage from './lib/ui/AlbumPage.svelte';
  import SearchPage from './lib/ui/SearchPage.svelte';
  import ArtistList from './lib/ui/ArtistList.svelte';
  import TrackPage from './lib/ui/TrackPage.svelte';
  import ArtistPage from './lib/ui/ArtistPage.svelte';
  import QueuePage from './lib/ui/QueuePage.svelte';
  import SettingsPage from './lib/ui/SettingsPage.svelte';
  import QueueDrawer from './lib/ui/QueueDrawer.svelte';
  import PlayerBar from './lib/ui/PlayerBar.svelte';
  import MobilePlayer from './lib/ui/MobilePlayer.svelte';
  import ErrorView from './lib/ui/ErrorView.svelte';
  import { onMount } from 'svelte';

  const route = $derived(router.route);
  let queueOpen = $state(false);
  let menuOpen = $state(false);
  let pageKey = $state(0);

  onMount(() => {
    // Remount + scroll-to-top only when the PATH changes. Query-only
    // updates (edition ?release=, album ?section=) must not remount the
    // page or yank the scroll position.
    let lastPath = location.pathname;
    const unsub = router.route.subscribe(() => {
      const path = location.pathname;
      if (path !== lastPath) {
        lastPath = path;
        pageKey++;
        menuOpen = false;
        window.scrollTo({ top: 0 });
      }
    });
    const onQueueEvent = () => (queueOpen = true);
    window.addEventListener('musicpack:queue', onQueueEvent);
    return () => {
      unsub();
      window.removeEventListener('musicpack:queue', onQueueEvent);
    };
  });

  function onSignOut(): void {
    void session.logout();
  }
  void onSignOut;
</script>

<AuthGate>
  <div class="shell">
    <AppSidebar open={menuOpen} onClose={() => (menuOpen = false)} />
    {#if menuOpen}
      <button
        class="sidebar-backdrop"
        aria-label="Close navigation"
        onclick={() => (menuOpen = false)}
      ></button>
    {/if}
    <div class="shell-column">
      <TopBar menuOpen={menuOpen} onMenu={() => (menuOpen = true)} />
      <main id="main" class="main" tabindex="-1">
        {#key pageKey}
          {#if route.get().name === 'albums'}
            <AlbumShelf />
          {:else if route.get().name === 'album'}
            <AlbumPage
              albumId={route.get().params.id ?? ''}
              releaseParam={route.get().query.get('release') ?? undefined}
            />
          {:else if route.get().name === 'track'}
            <TrackPage trackId={route.get().params.id ?? ''} />
          {:else if route.get().name === 'search'}
            <SearchPage />
          {:else if route.get().name === 'artists'}
            <ArtistList />
          {:else if route.get().name === 'artist'}
            <ArtistPage artistId={route.get().params.id ?? ''} />
          {:else if route.get().name === 'queue'}
            <QueuePage />
          {:else if route.get().name === 'settings'}
            <SettingsPage />
          {:else}
            <ErrorView message="Page not found" detail="This address does not match anything in the collection." />
          {/if}
        {/key}
      </main>
    </div>
    <PlayerBar />
    <MobilePlayer />
    <QueueDrawer open={queueOpen} onClose={() => (queueOpen = false)} />
  </div>
</AuthGate>
