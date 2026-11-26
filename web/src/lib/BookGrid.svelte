<script lang="ts">
  // A grid of books (AB5's shelf, shared since AB7 by the library's
  // book list, the series page and the author page): cover, title,
  // authors, length, and where this profile is in each. On a series
  // page `seriesId` adds each book's place in that series.
  import { formatDurationMs, posterUrl, sequenceLabel, type Book } from './types';

  let { books, seriesId = null }: { books: Book[]; seriesId?: string | null } = $props();

  function share(b: Book): number {
    if (b.progress.finished) return 1;
    return b.duration_ms > 0 ? Math.min(1, b.progress.position_ms / b.duration_ms) : 0;
  }
</script>

<ul class="grid books">
  {#each books as b (b.id)}
    {@const seq = seriesId ? sequenceLabel(b, seriesId) : null}
    <li>
      <a class="book" href={`/book/${b.id}`}>
        <span class="cover">
          {#if b.has_cover}
            <img src={posterUrl(b.id)} alt="" loading="lazy" />
          {:else}
            <span class="tile" aria-hidden="true">{b.title.charAt(0).toUpperCase()}</span>
          {/if}
          {#if b.progress.finished}
            <span class="badge">Finished</span>
          {/if}
          {#if share(b) > 0 && !b.progress.finished}
            <span class="progress" aria-hidden="true"><span style={`width: ${Math.round(share(b) * 100)}%`}></span></span>
          {/if}
          {#if !b.available}
            <span class="badge missing">Unavailable</span>
          {/if}
        </span>
        {#if seq}<span class="seq small">{seq}</span>{/if}
        <span class="title">{b.title}</span>
        {#if b.authors.length}<span class="dim small">{b.authors.join(', ')}</span>{/if}
        <span class="dim small">{formatDurationMs(b.duration_ms)}</span>
      </a>
    </li>
  {/each}
</ul>

<style>
  .books {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .book {
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
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .tile {
    display: grid;
    place-items: center;
    height: 100%;
    font-size: 2.4rem;
    color: var(--text-dim);
  }
  .book:hover .cover,
  .book:focus-visible .cover {
    outline: 2px solid var(--accent);
  }
  .title {
    font-weight: 600;
    line-height: 1.25;
    margin-top: 0.3rem;
  }
  .seq {
    margin-top: 0.3rem;
    color: var(--accent);
  }
  .seq + .title {
    margin-top: 0;
  }
  .small {
    font-size: 0.82rem;
  }
  .badge {
    position: absolute;
    top: 0.4rem;
    left: 0.4rem;
    background: var(--accent);
    color: var(--accent-ink);
    font-size: 0.72rem;
    padding: 0.1rem 0.4rem;
    border-radius: 0.3rem;
  }
  .badge.missing {
    background: var(--danger);
    top: auto;
    bottom: 0.6rem;
  }
  .progress {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 4px;
    background: rgba(0, 0, 0, 0.5);
  }
  .progress span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
</style>
