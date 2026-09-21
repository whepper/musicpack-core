<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Truncated monospace hash/id with click-to-copy. The full value is in
  // the title tooltip and the clipboard; nothing is invented or shortened
  // in what gets copied.
  let { value, label = 'hash' }: { value: string; label?: string } = $props();

  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => () => clearTimeout(timer));

  const short = $derived(
    value.length > 18 ? `${value.slice(0, 8)}…${value.slice(-6)}` : value,
  );

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      copied = true;
      clearTimeout(timer);
      timer = setTimeout(() => (copied = false), 1500);
    } catch {
      // Clipboard unavailable (permissions/insecure context): the tooltip
      // still carries the full value.
    }
  }
</script>

<span class="hash-wrap">
  <button class="hash" title={value} aria-label={`Copy ${label} to clipboard`} onclick={copy}
    >{short}</button>
  <span class="hash-copied" role="status">{copied ? 'Copied' : ''}</span>
</span>
