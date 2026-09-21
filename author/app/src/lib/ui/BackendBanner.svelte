<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../bootstrap';
  import type { BackendInfo } from '../types';

  let status: 'checking' | 'ok' | 'error' = $state('checking');
  let info: BackendInfo | null = $state(null);
  let message = $state('');

  async function check(): Promise<void> {
    status = 'checking';
    try {
      info = await api.backendInfo();
      status = 'ok';
    } catch (e) {
      message = e instanceof Error ? e.message : 'Authoring backend is unavailable.';
      status = 'error';
    }
  }

  onMount(() => {
    void check();
    // The backend can appear later (CLI built while the app was open) or a
    // previous check may have raced startup; re-check when the window gets
    // focus so recovery is one click away at most.
    const onFocus = (): void => {
      if (status === 'error') void check();
    };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  });
</script>

{#if status === 'checking'}
  <div class="banner checking">Checking authoring backend…</div>
{:else if status === 'error'}
  <div class="banner error" role="alert">
    <strong>Backend unavailable</strong> — {message}
    <button class="btn ghost" onclick={check}>Retry</button>
  </div>
{:else if info}
  <div class="banner ok">
    <span class="dot" aria-hidden="true"></span>
    musicpack {info.musicpackVersion} · {info.location} backend · author API {info.authorApi}
  </div>
{/if}

<style>
  .banner {
    font-size: 12px;
    line-height: 1.4;
    padding: 6px var(--space-5);
    border-bottom: 1px solid var(--hairline);
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .banner.checking {
    color: var(--text-soft);
  }
  .banner.error {
    background: rgba(224, 122, 106, 0.08);
    color: var(--danger);
  }
  .banner.ok {
    color: var(--text-soft);
  }
  .banner.error .btn {
    margin-left: auto;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ok);
    display: inline-block;
  }
</style>
