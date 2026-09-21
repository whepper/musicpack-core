<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import type { Confidence } from '../types';

  let { confidence }: { confidence: Confidence | undefined } = $props();

  const label: Record<Confidence, string> = {
    exact: 'Exact',
    confirmed: 'Confirmed',
    probable: 'Probable',
    none: 'Not identified',
  };
</script>

{#if confidence}
  <span class="conf {confidence}">{label[confidence]}</span>
{:else}
  <span class="conf none">Not identified</span>
{/if}
