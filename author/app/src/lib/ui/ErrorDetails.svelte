<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<!-- Shared task-failure presentation: the message, an expander with the
     captured backend output, and a retry action. Used by encode, Sonic and
     waveform so all long tasks fail the same way. -->
<script lang="ts">
  let {
    message,
    details = null,
    onRetry = null,
    retryLabel = 'Retry',
  }: {
    message: string;
    details?: string | null;
    onRetry?: (() => void) | null;
    retryLabel?: string;
  } = $props();
</script>

<div class="error-block" role="alert">
  <p class="error-banner" style="margin:0">{message}</p>
  {#if details}
    <details class="error-details">
      <summary class="smallcaps">Backend output</summary>
      <pre>{details}</pre>
    </details>
  {/if}
  {#if onRetry}
    <button class="btn ghost" onclick={onRetry}>{retryLabel}</button>
  {/if}
</div>

<style>
  .error-block {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.5rem;
  }
  .error-details pre {
    margin: 0.4rem 0 0;
    padding: 0.6rem 0.75rem;
    background: var(--surface);
    border: 1px solid var(--hairline);
    border-radius: var(--radius);
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.45;
    max-height: 180px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
