<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import type { Chip } from './status';

  let { chip }: { chip: Chip } = $props();
</script>

<span class="status-chip {chip.tone}">{#if chip.mark}<span aria-hidden="true">{chip.mark}</span>{/if}{chip.label}</span>
