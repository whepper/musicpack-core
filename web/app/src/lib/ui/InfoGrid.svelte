<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Key/value grid shared by the album sections (extracted from the old
  // ReleaseInfo panel). Rows with empty/undefined values are omitted.
  let { rows }: { rows: Array<[string, string | number | undefined]> } = $props();

  const visible = $derived(rows.filter(([, value]) => value !== undefined && value !== ''));
</script>

{#if visible.length > 0}
  <dl class="info-grid">
    {#each visible as [label, value] (label)}
      <div>
        <dt>{label}</dt>
        <dd>{value}</dd>
      </div>
    {/each}
  </dl>
{/if}
