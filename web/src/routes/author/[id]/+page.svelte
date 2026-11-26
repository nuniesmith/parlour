<script lang="ts">
  // An author page (Phase 10, AB7): the audiobooks this profile can
  // reach that credit them as author, by title, and the series those
  // books sit in. `?library_id=` keeps it to one library.
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import BookGrid from '$lib/BookGrid.svelte';
  import type { AuthorDetail } from '$lib/types';

  let author = $state<AuthorDetail | null>(null);
  let error = $state<string | null>(null);

  const libraryId = $derived(page.url.searchParams.get('library_id'));

  async function load(id: string, library: string | null) {
    author = null;
    error = null;
    const qs = library ? `?library_id=${encodeURIComponent(library)}` : '';
    const { data, error: err } = await api<AuthorDetail>(`/api/v1/authors/${id}${qs}`);
    if (data) author = data;
    else error = err ?? 'Could not load that author.';
  }

  $effect(() => {
    void load(page.params.id ?? '', libraryId);
  });
</script>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if !author}
  <p class="dim">Loading…</p>
{:else}
  <header class="head">
    <p class="kicker dim">
      {#if libraryId}
        <a href={`/library/${libraryId}?view=authors`}>Authors</a>
      {:else}
        Author
      {/if}
    </p>
    <h1>{author.name}</h1>
    <p class="dim">{author.book_count} {author.book_count === 1 ? 'book' : 'books'}</p>
  </header>

  {#if author.series.length}
    <section aria-labelledby="author-series">
      <h2 id="author-series">Series</h2>
      <ul class="series">
        {#each author.series as s (s.id)}
          <li>
            <a href={`/series/${s.id}`}>{s.name}</a>
            <span class="dim">· {s.book_count} {s.book_count === 1 ? 'book' : 'books'}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <section aria-labelledby="author-books">
    <h2 id="author-books">Books</h2>
    <BookGrid books={author.books} />
  </section>
{/if}

<style>
  .head {
    margin: 1rem 0 1.4rem;
  }
  .head h1 {
    margin: 0.2rem 0 0.4rem;
  }
  .kicker {
    margin: 0;
    font-size: 0.85rem;
  }
  .kicker a {
    color: inherit;
  }
  h2 {
    font-size: 1.05rem;
    margin: 1.4rem 0 0.7rem;
  }
  .series {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 1.2rem;
  }
  .series a {
    font-weight: 600;
  }
</style>
