<script lang="ts">
  // An audiobooks library's page (Phase 10, AB5): the books this
  // profile can reach, with cover, authors and where it is in each.
  import { api } from './api';
  import { formatDurationMs, posterUrl, type Book } from './types';

  let { libraryId }: { libraryId: string } = $props();

  let books = $state<Book[]>([]);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let q = $state('');
  let sort = $state('title');
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function load() {
    loading = true;
    const params = new URLSearchParams({ library_id: libraryId });
    if (q.trim()) params.set('q', q.trim());
    if (sort !== 'title') params.set('sort', sort);
    const { data, error: err } = await api<Book[]>(`/api/v1/audiobooks?${params}`);
    loading = false;
    if (data) {
      books = data;
      error = null;
    } else {
      error = err ?? 'Could not load the books.';
    }
  }

  function searchSoon() {
    clearTimeout(timer);
    timer = setTimeout(load, 250);
  }

  $effect(() => {
    void libraryId;
    load();
    return () => clearTimeout(timer);
  });

  function share(b: Book): number {
    if (b.progress.finished) return 1;
    return b.duration_ms > 0 ? Math.min(1, b.progress.position_ms / b.duration_ms) : 0;
  }
</script>

<div class="toolbar" role="group" aria-label="Find a book">
  <label>
    Search
    <input type="search" placeholder="Title, author or narrator" bind:value={q} oninput={searchSoon} />
  </label>
  <label>
    Sort
    <select bind:value={sort} onchange={load}>
      <option value="title">Title</option>
      <option value="added">Recently added</option>
      <option value="recent">Recently listened</option>
    </select>
  </label>
</div>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if loading && books.length === 0}
  <p class="dim">Loading…</p>
{:else if books.length === 0}
  <p class="dim">
    {q.trim() ? 'No book matches that search.' : 'No books yet — scan the library from Server → Library.'}
  </p>
{:else}
  <ul class="grid books">
    {#each books as b (b.id)}
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
          <span class="title">{b.title}</span>
          {#if b.authors.length}<span class="dim small">{b.authors.join(', ')}</span>{/if}
          <span class="dim small">{formatDurationMs(b.duration_ms)}</span>
        </a>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .toolbar {
    display: flex;
    gap: 1rem;
    flex-wrap: wrap;
    align-items: end;
    margin: 1rem 0 1.4rem;
  }
  .toolbar label {
    display: grid;
    gap: 0.25rem;
    font-size: 0.82rem;
    color: var(--text-dim);
  }
  .toolbar input,
  .toolbar select {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.45rem 0.6rem;
    color: var(--text);
  }
  .toolbar input {
    min-width: 16rem;
  }
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
