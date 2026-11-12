<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import Poster from '$lib/Poster.svelte';
  import type { Item } from '$lib/types';

  const PAGE = 60;

  let items = $state<Item[]>([]);
  let allGenres = $state<string[]>([]);
  let allRatings = $state<string[]>([]);
  let allYears = $state<number[]>([]);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let visible = $state(PAGE);

  let kind = $state('');
  let genre = $state('');
  let rating = $state('');
  let year = $state('');
  let unwatched = $state(false);
  let sort = $state('title');

  let sentinel: HTMLDivElement | null = $state(null);

  async function load() {
    loading = true;
    const params = new URLSearchParams();
    if (kind) params.set('kind', kind);
    if (genre) params.set('genre', genre);
    if (rating) params.set('rating', rating);
    if (year) params.set('year', year);
    if (unwatched) params.set('unwatched', 'true');
    if (sort !== 'title') params.set('sort', sort);
    const qs = params.toString();
    const { data, error: err } = await api<Item[]>(
      `/api/v1/libraries/${page.params.id}/items${qs ? `?${qs}` : ''}`,
    );
    loading = false;
    if (data) {
      items = data;
      visible = PAGE;
      if (allGenres.length === 0) {
        allGenres = [...new Set(data.flatMap((i) => i.genres))].sort();
        allRatings = [...new Set(data.map((i) => i.content_rating).filter(Boolean) as string[])].sort();
        allYears = [...new Set(data.map((i) => i.year).filter(Boolean) as number[])].sort(
          (a, b) => b - a,
        );
      }
    } else {
      error = err ?? 'Could not load this library.';
    }
  }

  onMount(() => {
    load();
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) visible += PAGE;
      },
      { rootMargin: '600px' },
    );
    if (sentinel) observer.observe(sentinel);
    return () => observer.disconnect();
  });

</script>

<div class="toolbar" role="group" aria-label="Filters">
  <label>
    Type
    <select bind:value={kind} onchange={load}>
      <option value="">All</option>
      <option value="movie">Movies</option>
      <option value="series">Series</option>
    </select>
  </label>
  <label>
    Genre
    <select bind:value={genre} onchange={load}>
      <option value="">All</option>
      {#each allGenres as g (g)}<option value={g}>{g}</option>{/each}
    </select>
  </label>
  <label>
    Rating
    <select bind:value={rating} onchange={load}>
      <option value="">All</option>
      {#each allRatings as r (r)}<option value={r}>{r}</option>{/each}
    </select>
  </label>
  <label>
    Year
    <select bind:value={year} onchange={load}>
      <option value="">All</option>
      {#each allYears as y (y)}<option value={String(y)}>{y}</option>{/each}
    </select>
  </label>
  <label>
    Sort
    <select bind:value={sort} onchange={load}>
      <option value="title">Title</option>
      <option value="added">Recently added</option>
      <option value="year">Year (newest)</option>
      <option value="rating">Top rated</option>
    </select>
  </label>
  <label class="check">
    <input type="checkbox" bind:checked={unwatched} onchange={load} /> Unwatched only
  </label>
</div>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if loading}
  <p class="dim">Loading…</p>
{:else if items.length === 0}
  <p class="dim">Nothing matches. Try clearing a filter — or scan the library from Admin.</p>
{:else}
  <div class="grid">
    {#each items.slice(0, visible) as item (item.id)}
      <Poster {item} />
    {/each}
  </div>
  <div bind:this={sentinel} class="sentinel" aria-hidden="true"></div>
  {#if visible < items.length}
    <p class="dim center">Showing {Math.min(visible, items.length)} of {items.length}</p>
  {/if}
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
  .toolbar select {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.45rem 0.6rem;
  }
  .toolbar .check {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.95rem;
    color: var(--text);
    padding-bottom: 0.5rem;
  }
  .sentinel {
    height: 1px;
  }
  .center {
    text-align: center;
  }
</style>
