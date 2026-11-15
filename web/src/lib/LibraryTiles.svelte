<script lang="ts">
  // Home's library tile row (UI8): one landscape tile per library
  // the profile can see, linking to that library's grid. The art is
  // the backdrop of the library's newest item that has one.
  import type { LibraryTile } from './types';

  let { libraries }: { libraries: LibraryTile[] } = $props();

  const KIND_LABELS: Record<string, string> = {
    movies: 'Movies',
    shows: 'TV shows',
    audiobooks: 'Audiobooks',
    ebooks: 'Books',
    music: 'Music',
    podcasts: 'Podcasts',
  };

  function countLabel(lib: LibraryTile): string {
    const n = lib.item_count;
    if (lib.kind === 'shows') return n === 1 ? '1 show' : `${n} shows`;
    if (lib.kind === 'movies') return n === 1 ? '1 film' : `${n} films`;
    return n === 1 ? '1 item' : `${n} items`;
  }
</script>

{#if libraries.length > 0}
  <section class="rail">
    <h2>Libraries</h2>
    <div class="track">
      {#each libraries as lib (lib.id)}
        <a class="tile" href={`/library/${lib.id}`} aria-label={`${lib.name}, ${countLabel(lib)}`}>
          <span class="frame">
            {#if lib.art_item_id}
              <img src={`/api/v1/items/${lib.art_item_id}/artwork/backdrop`} alt="" loading="lazy" />
            {/if}
            <span class="label">
              <span class="name">{lib.name}</span>
              <span class="meta">{KIND_LABELS[lib.kind] ?? lib.kind} · {countLabel(lib)}</span>
            </span>
          </span>
        </a>
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
  .tile {
    flex: 0 0 260px;
    scroll-snap-align: start;
    color: inherit;
    text-decoration: none;
  }
  .frame {
    position: relative;
    display: block;
    aspect-ratio: 16 / 9;
    border-radius: var(--radius);
    overflow: hidden;
    background: linear-gradient(135deg, var(--bg-card), var(--bg-raise));
    border: 1px solid var(--line);
  }
  .tile:hover .frame,
  .tile:focus-visible .frame {
    border-color: var(--accent);
  }
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    opacity: 0.55;
  }
  .label {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    display: grid;
    gap: 0.15rem;
    padding: 0.7rem 0.85rem;
    background: linear-gradient(transparent, rgb(0 0 0 / 0.7));
  }
  .name {
    font-weight: 600;
    font-size: 1.05rem;
  }
  .meta {
    font-size: 0.8rem;
    color: var(--text-dim);
  }
  @media (max-width: 640px) {
    .tile {
      flex-basis: 200px;
    }
  }
</style>
