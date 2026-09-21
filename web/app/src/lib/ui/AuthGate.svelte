<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { session } from '../bootstrap';

  const sessionStore = session;
  let { children }: { children?: import('svelte').Snippet } = $props();
  let token = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    const t = token.trim();
    if (!t) return;
    busy = true;
    error = null;
    try {
      await session.authenticate(t);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not sign in.';
    } finally {
      busy = false;
      token = '';
    }
  }
</script>

{#if $sessionStore.state === 'checking'}
  <div class="spinner" role="status" aria-label="Checking session"></div>
{:else if $sessionStore.state === 'unauthenticated'}
  <div class="signin">
    <h1>Sign in</h1>
    <p class="muted">
      Enter the server token once. It is exchanged for a secure session
      cookie and is never stored in your browser.
    </p>
    <form onsubmit={submit}>
      <div class="field">
        <label for="token">Server token</label>
        <input
          id="token"
          type="password"
          autocomplete="off"
          value={token}
          oninput={(e) => (token = (e.currentTarget as HTMLInputElement).value)}
          placeholder="mpk_…"
        >
      </div>
      {#if error}<p class="muted" style="color:var(--danger)" role="alert">{error}</p>{/if}
      <button class="btn" type="submit" disabled={busy || !token}>
        {busy ? 'Signing in…' : 'Sign in'}
      </button>
    </form>
  </div>
{:else}
  {@render children?.()}
{/if}
