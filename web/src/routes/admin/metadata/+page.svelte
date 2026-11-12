<script lang="ts">
  // Server → Metadata: items the matcher was not sure about,
  // with its candidates and a manual search.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { Candidate, ReviewItem } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let review = $state<ReviewItem[]>([]);
  let searchKind = $state('movie');
  let searchQ = $state('');
  let searchFor = $state<ReviewItem | null>(null);
  let searchResults = $state<(Candidate & { confidence: number })[]>([]);

  async function loadReview() {
    const { data, error: err } = await api<ReviewItem[]>('/api/v1/admin/metadata/review-queue');
    review = data ?? [];
    if (err) error = err;
  }

  onMount(loadReview);

  async function applyCandidate(itemId: string, candidate: Candidate) {
    const { status, error: err } = await api(
      `/api/v1/admin/items/${itemId}/metadata/apply`,
      {
        method: 'POST',
        body: JSON.stringify({ provider_id: candidate.provider_id, provider: candidate.provider }),
      },
    );
    if (status === 200) {
      notice = 'Match applied.';
      searchFor = null;
      searchResults = [];
      await loadReview();
    } else error = err ?? 'Could not apply that match.';
  }

  async function manualSearch(event: SubmitEvent) {
    event.preventDefault();
    if (!searchFor) return;
    const { data, error: err } = await api<(Candidate & { confidence: number })[]>(
      `/api/v1/admin/metadata/search?kind=${searchKind}&q=${encodeURIComponent(searchQ)}`,
    );
    if (data) searchResults = data;
    else error = err ?? 'Search failed.';
  }
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" role="status">{notice}</p>{/if}

<h2>Needs review{#if review.length} ({review.length}){/if}</h2>
{#if review.length === 0}
  <p class="dim">Nothing waiting. The matcher only parks items here when it isn't sure.</p>
{/if}
{#each review as item (item.id)}
  <div class="review-item">
    <h3>{item.title}{#if item.year} ({item.year}){/if} <span class="chip">{item.kind}</span></h3>
    <div class="candidates">
      {#each item.candidates as cand (cand.provider_id)}
        <div class="candidate">
          {#if cand.poster_url}<img src={cand.poster_url} alt="" loading="lazy" />{/if}
          <div>
            <strong>{cand.title}</strong>
            <span class="dim">
              {cand.year ?? ''} · {Math.round(cand.confidence * 100)}% match
            </span>
            {#if cand.overview}<p class="dim overview">{cand.overview}</p>{/if}
            <button class="btn small primary" onclick={() => applyCandidate(item.id, cand)}>
              This is the one
            </button>
          </div>
        </div>
      {/each}
    </div>
    <form
      class="row"
      onsubmit={(e) => {
        searchFor = item;
        searchKind = item.kind === 'series' ? 'show' : 'movie';
        searchQ = item.title;
        manualSearch(e);
      }}
    >
      <span class="dim">Not listed?</span>
      <button class="btn small" type="submit">Search TMDB manually…</button>
    </form>
    {#if searchFor?.id === item.id && searchResults.length}
      <div class="candidates">
        {#each searchResults as cand (cand.provider_id)}
          <div class="candidate">
            {#if cand.poster_url}<img src={cand.poster_url} alt="" loading="lazy" />{/if}
            <div>
              <strong>{cand.title}</strong>
              <span class="dim">{cand.year ?? ''}</span>
              {#if cand.overview}<p class="dim overview">{cand.overview}</p>{/if}
              <button class="btn small primary" onclick={() => applyCandidate(item.id, cand)}>
                This is the one
              </button>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/each}
{#if searchFor}
  <form class="row search-manual" onsubmit={manualSearch}>
    <input bind:value={searchQ} aria-label="Search query" />
    <select bind:value={searchKind} aria-label="Kind">
      <option value="movie">Movie</option>
      <option value="show">Show</option>
    </select>
    <button class="btn" type="submit">Search for {searchFor.title}</button>
    <button class="btn" type="button" onclick={() => (searchFor = null)}>Close</button>
  </form>
{/if}

<style>
  .review-item {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 1rem 1.1rem;
    margin-bottom: 1rem;
    background: var(--bg-raise);
  }
  .candidates {
    display: grid;
    gap: 0.8rem;
  }
  .candidate {
    display: flex;
    gap: 0.9rem;
    align-items: flex-start;
  }
  .candidate img {
    width: 54px;
    border-radius: 0.4rem;
  }
  .overview {
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    margin: 0.3rem 0 0.5rem;
    max-width: 70ch;
  }
  .search-manual {
    position: sticky;
    bottom: 0.8rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.7rem;
  }
</style>
