<!--
Copyright (c) 2026, The MusicPack Development Team
SPDX-License-Identifier: BSD-3-Clause
-->

<script lang="ts">
  // Album section rail (Overview / Tracks / Edition / …). Roving-tabindex
  // tabs: arrow keys move selection and focus, per the WAI-ARIA tabs
  // pattern with manual activation.
  let {
    sections,
    active,
    onSelect,
  }: {
    sections: Array<{ id: string; label: string }>;
    active: string;
    onSelect: (id: string) => void;
  } = $props();

  function onTabKeydown(event: KeyboardEvent, index: number): void {
    let next = -1;
    if (event.key === 'ArrowRight') next = (index + 1) % sections.length;
    else if (event.key === 'ArrowLeft') next = (index - 1 + sections.length) % sections.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = sections.length - 1;
    if (next < 0) return;
    event.preventDefault();
    const target = sections[next];
    if (!target) return;
    onSelect(target.id);
    queueMicrotask(() => {
      document.getElementById(`tab-${target.id}`)?.focus();
    });
  }
</script>

<div class="section-tabs" role="tablist" aria-label="Album sections">
  {#each sections as s, i (s.id)}
    <button
      role="tab"
      id={`tab-${s.id}`}
      class="section-tab"
      class:active={active === s.id}
      aria-selected={active === s.id}
      aria-controls={`panel-${s.id}`}
      tabindex={active === s.id ? 0 : -1}
      onclick={() => onSelect(s.id)}
      onkeydown={(e) => onTabKeydown(e, i)}
    >{s.label}</button>
  {/each}
</div>
