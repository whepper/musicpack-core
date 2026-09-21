<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  import type { ValidationResult } from '../types';

  let { result }: { result: ValidationResult } = $props();
</script>

<div class="validation">
  <div>
    <h3>Errors</h3>
    {#if result.errors.length === 0}
      <p class="muted">None — this draft is ready to build.</p>
    {:else}
      <ul>
        {#each result.errors as err}
          <li class="err">{err}</li>
        {/each}
      </ul>
    {/if}
  </div>
  <div>
    <h3>Warnings</h3>
    {#if result.warnings.length === 0}
      <p class="muted">None.</p>
    {:else}
      <ul>
        {#each result.warnings as warn}
          <li class="warn">{warn}</li>
        {/each}
      </ul>
    {/if}
  </div>
</div>
