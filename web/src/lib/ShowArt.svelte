<script lang="ts">
  // POD6: a show's artwork — Parlour's cached copy (never the
  // publisher's address, POD4 reading 11) — or, when there is none or
  // it can't be loaded, a plain initial tile, as the book pages do.
  let {
    url,
    title,
    size = 'md',
  }: { url: string | null; title: string; size?: 'sm' | 'md' | 'lg' } = $props();

  let broken = $state<string | null>(null);
  const initial = $derived(
    ([...title].find((c) => /[\p{L}\p{N}]/u.test(c)) ?? '?').toUpperCase(),
  );
</script>

<span class="art {size}">
  {#if url && broken !== url}
    <img src={url} alt="" loading="lazy" onerror={() => (broken = url)} />
  {:else}
    <span class="tile" aria-hidden="true">{initial}</span>
  {/if}
</span>

<style>
  .art {
    display: block;
    aspect-ratio: 1;
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg-raise);
    border: 1px solid var(--line);
  }
  .art.sm {
    border-radius: 0.4rem;
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .tile {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--text-dim);
    font-size: 2rem;
  }
  .sm .tile {
    font-size: 1.1rem;
  }
  .lg .tile {
    font-size: 3rem;
  }
</style>
