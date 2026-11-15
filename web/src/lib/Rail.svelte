<script lang="ts">
  import Poster from './Poster.svelte';
  import type { Item } from './types';

  let {
    title,
    items,
    href = null,
    progressFor = null,
  }: {
    title: string;
    items: Item[];
    /** When set, the heading links here (e.g. the library's grid). */
    href?: string | null;
    progressFor?: ((item: Item) => number | null) | null;
  } = $props();
</script>

{#if items.length > 0}
  <section class="rail">
    <h2>
      {#if href}<a class="more" {href}>{title} <span aria-hidden="true">›</span></a>{:else}{title}{/if}
    </h2>
    <div class="track">
      {#each items as item (item.id)}
        <div class="cell">
          <Poster {item} progress={progressFor ? progressFor(item) : null} />
        </div>
      {/each}
    </div>
  </section>
{/if}

<style>
  .rail {
    margin: 1.6rem 0;
  }
  h2 {
    font-size: 1.15rem;
    margin: 0 0 0.7rem;
  }
  .more {
    color: inherit;
    text-decoration: none;
  }
  .more:hover,
  .more:focus-visible {
    color: var(--accent);
  }
  .track {
    display: flex;
    gap: 1rem;
    overflow-x: auto;
    padding-bottom: 0.4rem;
    scroll-snap-type: x proximity;
  }
  .cell {
    flex: 0 0 150px;
    scroll-snap-align: start;
  }
  @media (max-width: 640px) {
    .cell {
      flex-basis: 112px;
    }
  }
</style>
