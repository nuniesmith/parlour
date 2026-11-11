<script lang="ts">
  import Poster from './Poster.svelte';
  import type { Item } from './types';

  let {
    title,
    items,
    progressFor = null,
  }: {
    title: string;
    items: Item[];
    progressFor?: ((item: Item) => number | null) | null;
  } = $props();
</script>

{#if items.length > 0}
  <section class="rail">
    <h2>{title}</h2>
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
