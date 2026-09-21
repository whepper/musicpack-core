<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Primary navigation rail (v2 shell). Desktop ≥681 px: static sidebar
  // (icon-only ≤1024 px). Below 681 px: off-canvas drawer toggled by the
  // top bar menu button; `visibility` removes the closed drawer from the
  // tab order and the accessibility tree (same pattern as QueueDrawer).
  import { session, router } from '../bootstrap';

  let { open, onClose }: { open: boolean; onClose: () => void } = $props();

  // Subscribe to the store; a plain router.get() read would freeze the
  // active highlight on whichever page first rendered.
  const routeStore = router.route;
  const name = $derived($routeStore.name);
  const sessionState = $derived($session.state);

  function signOut(): void {
    onClose();
    void session.logout();
  }
</script>

<aside class="sidebar" class:open>
  <a class="side-brand" href="/" aria-label="MusicPack home" onclick={onClose}>Music<em>Pack</em></a>

  <nav class="side-nav" aria-label="Primary">
    <p class="side-group" aria-hidden="true">Collection</p>
    <a
      class="side-link"
      href="/albums"
      aria-current={name === 'albums' || name === 'album' ? 'page' : undefined}
      onclick={onClose}
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
        <circle cx="12" cy="12" r="8.5" /><circle cx="12" cy="12" r="2.2" />
      </svg>
      <span class="side-label">Albums</span>
    </a>
    <a
      class="side-link"
      href="/artists"
      aria-current={name === 'artists' || name === 'artist' ? 'page' : undefined}
      onclick={onClose}
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
        <circle cx="12" cy="8" r="3.4" /><path d="M5 19.5c1.3-3.4 3.9-5 7-5s5.7 1.6 7 5" />
      </svg>
      <span class="side-label">Artists</span>
    </a>
    <a
      class="side-link"
      href="/search"
      aria-current={name === 'search' ? 'page' : undefined}
      onclick={onClose}
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
        <circle cx="11" cy="11" r="6.5" /><path d="M20 20l-4.4-4.4" />
      </svg>
      <span class="side-label">Search</span>
    </a>

    <p class="side-group" aria-hidden="true">Playback</p>
    <a
      class="side-link"
      href="/queue"
      aria-current={name === 'queue' ? 'page' : undefined}
      onclick={onClose}
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
        <path d="M8 5.5v13M16 5.5v13M8 12h8" /><circle cx="6.4" cy="18" r="1.9" /><circle cx="14.4" cy="18" r="1.9" />
      </svg>
      <span class="side-label">Now Playing</span>
    </a>

    <p class="side-group" aria-hidden="true">System</p>
    <a
      class="side-link"
      href="/settings"
      aria-current={name === 'settings' ? 'page' : undefined}
      onclick={onClose}
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
        <path d="M4 7.5h9M17 7.5h3M4 16.5h3M11 16.5h9" /><circle cx="15" cy="7.5" r="2" /><circle cx="9" cy="16.5" r="2" />
      </svg>
      <span class="side-label">Settings</span>
    </a>
  </nav>

  <footer class="side-foot">
    {#if sessionState === 'offline'}
      <span class="smallcaps side-state">Offline mode</span>
    {/if}
    <button class="smallcaps" onclick={signOut}>Sign out</button>
  </footer>
</aside>
