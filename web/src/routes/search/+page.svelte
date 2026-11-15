<script lang="ts">
  import { untrack } from 'svelte';
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import Poster from '$lib/Poster.svelte';
  import Rail from '$lib/Rail.svelte';
  import {
    clearSearches,
    committable,
    forgetSearch,
    loadRecent,
    recent,
    recordSearch,
  } from '$lib/recentSearches.svelte';
  import { session } from '$lib/session.svelte';
  import type { Item } from '$lib/types';

  interface PersonHit {
    id: string;
    name: string;
    items: Item[];
  }
  interface SearchData {
    items: Item[];
    people: PersonHit[];
  }

  let q = $state('');
  let results = $state<SearchData | null>(null);
  let searching = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;

  function run(delay: number) {
    if (timer) clearTimeout(timer);
    const query = q.trim();
    if (query.length < 2) {
      results = null;
      return;
    }
    timer = setTimeout(async () => {
      searching = true;
      const { data } = await api<SearchData>(`/api/v1/search?q=${encodeURIComponent(query)}`);
      // A newer keystroke or sidebar search may have superseded this one.
      if (query !== q.trim()) return;
      searching = false;
      results = data;
    }, delay);
  }

  function onInput() {
    run(300);
  }

  // UI9: Enter commits the search — run it now and remember it.
  function onSubmit(event: SubmitEvent) {
    event.preventDefault();
    if (!committable(q)) return;
    run(0);
    void recordSearch(q);
  }

  // Opening a result commits the search that found it.
  function onResultClick(event: MouseEvent) {
    if (event.target instanceof Element && event.target.closest('a')) void recordSearch(q);
  }

  function rerun(query: string) {
    q = query;
    run(0);
    void recordSearch(query);
  }

  const profileId = $derived(session.me?.active_profile?.id ?? null);
  const recentItems = $derived(recent.profileId === profileId ? recent.items : []);
  const showRecent = $derived(!committable(q) && recentItems.length > 0);

  // The list is per profile; refetch when the page opens or the
  // active profile becomes known / changes.
  $effect(() => {
    if (profileId) void loadRecent();
  });

  onMount(() => {
    // Arriving with nothing to search for: start typing right away.
    if (page.url.searchParams.get('q') === null) document.getElementById('q')?.focus();
  });

  // `/search?q=…` (the sidebar search box) seeds the field and runs at once.
  $effect(() => {
    const fromUrl = page.url.searchParams.get('q');
    if (fromUrl === null) return;
    untrack(() => {
      q = fromUrl;
      run(0);
    });
  });
</script>

<form class="field search-box" role="search" onsubmit={onSubmit}>
  <label for="q">Title or person</label>
  <input
    id="q"
    type="search"
    bind:value={q}
    oninput={onInput}
    placeholder="Start typing — at least 2 characters"
    autocomplete="off"
    aria-keyshortcuts="/"
  />
</form>

{#if showRecent}
  <section class="recent" aria-labelledby="recent-heading">
    <div class="recent-head">
      <h2 id="recent-heading">Recent searches</h2>
      <button type="button" class="link" onclick={() => void clearSearches()}>Clear all</button>
    </div>
    <ul>
      {#each recentItems as r (r.id)}
        <li>
          <button type="button" class="again" onclick={() => rerun(r.query)}>{r.query}</button>
          <button
            type="button"
            class="forget"
            aria-label={`Remove "${r.query}" from recent searches`}
            title="Remove"
            onclick={() => void forgetSearch(r.id)}>×</button
          >
        </li>
      {/each}
    </ul>
  </section>
{/if}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="results" onclick={onResultClick}>
  {#if searching}
    <p class="dim">Searching…</p>
  {:else if results}
    {#if results.items.length === 0 && results.people.length === 0}
      <p class="dim">No matches for "{q}".</p>
    {/if}
    {#if results.items.length}
      <div class="grid">
        {#each results.items as item (item.id)}
          <Poster {item} />
        {/each}
      </div>
    {/if}
    {#each results.people as person (person.id)}
      <Rail title={person.name} items={person.items} />
    {/each}
  {/if}
</div>

<style>
  .search-box {
    max-width: 34rem;
  }
  .recent {
    max-width: 34rem;
    margin-bottom: 1.6rem;
  }
  .recent-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
  }
  .recent h2 {
    margin: 0 0 0.4rem;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--text-dim);
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  .recent ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .recent li {
    display: flex;
    align-items: center;
    border-top: 1px solid var(--line);
  }
  .again {
    flex: 1;
    min-width: 0;
    min-height: 2.5rem;
    text-align: left;
    background: none;
    border: 0;
    padding: 0.4rem 0.6rem;
    cursor: pointer;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .again:hover {
    background: var(--bg-card);
  }
  .forget {
    flex: none;
    width: 2.5rem;
    height: 2.5rem;
    background: none;
    border: 0;
    border-radius: 50%;
    cursor: pointer;
    color: var(--text-dim);
    font-size: 1.2rem;
  }
  .forget:hover {
    color: var(--text);
    background: var(--bg-card);
  }
  .link {
    background: none;
    border: 0;
    padding: 0.2rem 0;
    cursor: pointer;
    color: var(--text-dim);
    font-size: 0.85rem;
  }
  .link:hover {
    color: var(--text);
  }
</style>
