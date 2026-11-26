<script lang="ts">
  // An audiobooks library's page (Phase 10, AB5): the books this
  // profile can reach, with cover, authors and where it is in each.
  // AB7 adds two more views of the same library, picked with
  // `?view=`: its series (each a page of books in sequence order) and
  // its authors (each a page of their books).
  import { page } from '$app/state';
  import { api } from './api';
  import BookGrid from './BookGrid.svelte';
  import { posterUrl, type Author, type Book, type Series } from './types';

  let { libraryId }: { libraryId: string } = $props();

  type View = 'books' | 'series' | 'authors';
  const VIEWS: { id: View; label: string }[] = [
    { id: 'books', label: 'Books' },
    { id: 'series', label: 'Series' },
    { id: 'authors', label: 'Authors' },
  ];
  const view = $derived<View>(
    (VIEWS.find((v) => v.id === page.url.searchParams.get('view'))?.id ?? 'books') as View,
  );

  let books = $state<Book[]>([]);
  let series = $state<Series[]>([]);
  let authors = $state<Author[]>([]);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let q = $state('');
  let sort = $state('title');
  let timer: ReturnType<typeof setTimeout> | undefined;

  const shown = $derived(
    view === 'books' ? books.length : view === 'series' ? series.length : authors.length,
  );

  async function load() {
    loading = true;
    const params = new URLSearchParams({ library_id: libraryId });
    if (q.trim()) params.set('q', q.trim());
    let failed: string | null = null;
    if (view === 'series') {
      const r = await api<Series[]>(`/api/v1/series?${params}`);
      if (r.data) series = r.data;
      else failed = r.error ?? 'Could not load the series.';
    } else if (view === 'authors') {
      const r = await api<Author[]>(`/api/v1/authors?${params}`);
      if (r.data) authors = r.data;
      else failed = r.error ?? 'Could not load the authors.';
    } else {
      if (sort !== 'title') params.set('sort', sort);
      const r = await api<Book[]>(`/api/v1/audiobooks?${params}`);
      if (r.data) books = r.data;
      else failed = r.error ?? 'Could not load the books.';
    }
    loading = false;
    error = failed;
  }

  function searchSoon() {
    clearTimeout(timer);
    timer = setTimeout(load, 250);
  }

  $effect(() => {
    void libraryId;
    void view;
    load();
    return () => clearTimeout(timer);
  });

  function initials(name: string): string {
    return name
      .split(/\s+/)
      .filter(Boolean)
      .map((w) => w.charAt(0).toUpperCase())
      .slice(0, 2)
      .join('');
  }

  function plural(n: number, one: string): string {
    return `${n} ${one}${n === 1 ? '' : 's'}`;
  }

  const placeholder = $derived(
    view === 'series' ? 'Series name' : view === 'authors' ? 'Author name' : 'Title, author, narrator or series',
  );
  const empty = $derived(
    q.trim()
      ? `No ${view === 'books' ? 'book' : view === 'series' ? 'series' : 'author'} matches that search.`
      : view === 'books'
        ? 'No books yet — scan the library from Server → Library.'
        : view === 'series'
          ? 'No series yet. Series come from a book’s Author/Series/Title folder or its tags.'
          : 'No authors yet.',
  );
</script>

<nav class="views" aria-label="Browse this library by">
  {#each VIEWS as v (v.id)}
    <a
      href={v.id === 'books' ? '?' : `?view=${v.id}`}
      aria-current={view === v.id ? 'page' : undefined}
      data-sveltekit-replacestate
      data-sveltekit-noscroll>{v.label}</a
    >
  {/each}
</nav>

<div class="toolbar" role="group" aria-label="Find in this library">
  <label>
    Search
    <input type="search" {placeholder} bind:value={q} oninput={searchSoon} />
  </label>
  {#if view === 'books'}
    <label>
      Sort
      <select bind:value={sort} onchange={load}>
        <option value="title">Title</option>
        <option value="added">Recently added</option>
        <option value="recent">Recently listened</option>
      </select>
    </label>
  {/if}
</div>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if loading && shown === 0}
  <p class="dim">Loading…</p>
{:else if shown === 0}
  <p class="dim">{empty}</p>
{:else if view === 'books'}
  <BookGrid {books} />
{:else if view === 'series'}
  <ul class="grid tiles">
    {#each series as s (s.id)}
      <li>
        <a class="tile-link" href={`/series/${s.id}`}>
          <span class="cover">
            {#if s.cover_book_id}
              <img src={posterUrl(s.cover_book_id)} alt="" loading="lazy" />
            {:else}
              <span class="letter" aria-hidden="true">{s.name.charAt(0).toUpperCase()}</span>
            {/if}
            <span class="count">{s.book_count}</span>
          </span>
          <span class="name">{s.name}</span>
          <span class="dim small">
            {plural(s.book_count, 'book')}{s.finished_count > 0
              ? ` · ${s.finished_count} finished`
              : ''}
          </span>
        </a>
      </li>
    {/each}
  </ul>
{:else}
  <ul class="grid tiles">
    {#each authors as a (a.id)}
      <li>
        <a class="tile-link" href={`/author/${a.id}?library_id=${libraryId}`}>
          <span class="cover round">
            <span class="letter" aria-hidden="true">{initials(a.name)}</span>
          </span>
          <span class="name">{a.name}</span>
          <span class="dim small">{plural(a.book_count, 'book')}</span>
        </a>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .views {
    display: flex;
    gap: 0.4rem;
    margin-top: 1rem;
  }
  .views a {
    padding: 0.35rem 0.85rem;
    border-radius: 999px;
    border: 1px solid var(--line);
    color: var(--text-dim);
    text-decoration: none;
    font-size: 0.9rem;
  }
  .views a[aria-current='page'] {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-ink);
  }
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
  .tiles {
    list-style: none;
    padding: 0;
    margin: 0;
  }
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
  .count {
    position: absolute;
    top: 0.4rem;
    right: 0.4rem;
    background: rgba(0, 0, 0, 0.7);
    color: #fff;
    font-size: 0.75rem;
    padding: 0.1rem 0.45rem;
    border-radius: 0.3rem;
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
