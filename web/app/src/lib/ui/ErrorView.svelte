<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  let { message, detail }: { message: string; detail?: string } = $props();
</script>

<div role="alert" class="empty-state error-state">
  <p>{message}</p>
  {#if detail}
    <p class="muted error-detail">{detail}</p>
  {/if}
</div>
