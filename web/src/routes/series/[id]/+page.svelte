<script lang="ts">
  // A series page (Phase 10, AB7, D28): the series' books in
  // sequence order, each with its place in it. Books without a
  // sequence come last.
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import BookGrid from '$lib/BookGrid.svelte';
  import type { SeriesDetail } from '$lib/types';

  let series = $state<SeriesDetail | null>(null);
  let error = $state<string | null>(null);

  async function load(id: string) {
    series = null;
    error = null;
    const { data, error: err } = await api<SeriesDetail>(`/api/v1/series/${id}`);
    if (data) series = data;
    else error = err ?? 'Could not load that series.';
  }

  $effect(() => {
    void load(page.params.id ?? '');
  });

  /** The authors across the series' books, in order of appearance. */
  const authors = $derived.by(() => {
    const seen = new Map<string, string>();
    for (const b of series?.books ?? []) {
      for (const a of b.author_refs) if (!seen.has(a.id)) seen.set(a.id, a.name);
    }
    return [...seen].map(([id, name]) => ({ id, name }));
  });
</script>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if !series}
  <p class="dim">Loading…</p>
{:else}
  <header class="head">
    <p class="kicker dim">
      <a href={`/library/${series.library_id}?view=series`}>Series</a>
    </p>
    <h1>{series.name}</h1>
    {#if authors.length}
      <p class="by">
        by
        {#each authors as a, i (a.id)}{i > 0 ? ', ' : ''}<a
            href={`/author/${a.id}?library_id=${series.library_id}`}>{a.name}</a
          >{/each}
      </p>
    {/if}
    <p class="dim">
      {series.book_count}
      {series.book_count === 1 ? 'book' : 'books'}{series.finished_count > 0
        ? ` · ${series.finished_count} finished`
        : ''}
    </p>
  </header>
  <BookGrid books={series.books} seriesId={series.id} />
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
  .kicker a,
  .by a {
    color: inherit;
  }
  .by {
    margin: 0 0 0.3rem;
  }
</style>
