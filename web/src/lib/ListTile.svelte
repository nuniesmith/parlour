<script lang="ts">
  // POD8: a playlist's or a collection's tile — up to four covers in a
  // square (Parlour's own addresses), else the name's initial; the name
  // and a count line under it. `round` gives the podcast Home's round
  // playlist buttons (UI-AUDIO-REFERENCE §2.2 / §4.4).
  let {
    href,
    name,
    sub,
    covers,
    round = false,
  }: { href: string; name: string; sub: string; covers: string[]; round?: boolean } = $props();
</script>

<a class="tile-link" {href} data-list-tile={name}>
  <span class="cover" class:round class:quad={covers.length >= 4}>
    {#if covers.length >= 4}
      {#each covers.slice(0, 4) as c (c)}<img src={c} alt="" loading="lazy" />{/each}
    {:else if covers.length > 0}
      <img src={covers[0]} alt="" loading="lazy" />
    {:else}
      <span class="letter" aria-hidden="true">{name.charAt(0).toUpperCase()}</span>
    {/if}
  </span>
  <span class="name">{name}</span>
  <span class="dim small">{sub}</span>
</a>

<style>
  .tile-link {
    display: grid;
    gap: 0.2rem;
    color: inherit;
    text-decoration: none;
  }
  .cover {
    position: relative;
    display: block;
    aspect-ratio: 1;
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg-raise);
    border: 1px solid var(--line);
  }
  .cover.round {
    border-radius: 50%;
  }
  .cover.quad {
    display: grid;
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr 1fr;
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .letter {
    display: grid;
    place-items: center;
    height: 100%;
    font-size: 2.2rem;
    color: var(--text-dim);
  }
  .tile-link:hover .cover,
  .tile-link:focus-visible .cover {
    outline: 2px solid var(--accent);
  }
  .name {
    font-weight: 600;
    line-height: 1.25;
    margin-top: 0.3rem;
  }
  .small {
    font-size: 0.82rem;
  }
</style>
