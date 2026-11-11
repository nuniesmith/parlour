<script lang="ts">
  import { api } from '$lib/api';
  import Poster from '$lib/Poster.svelte';
  import Rail from '$lib/Rail.svelte';
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

  function onInput() {
    if (timer) clearTimeout(timer);
    const query = q.trim();
    if (query.length < 2) {
      results = null;
      return;
    }
    timer = setTimeout(async () => {
      searching = true;
      const { data } = await api<SearchData>(`/api/v1/search?q=${encodeURIComponent(query)}`);
      searching = false;
      results = data;
    }, 300);
  }
</script>

<h1>Search</h1>
<div class="field search-box">
  <label for="q">Title or person</label>
  <input
    id="q"
    bind:value={q}
    oninput={onInput}
    placeholder="Start typing — at least 2 characters"
    autocomplete="off"
  />
</div>

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

<style>
  .search-box {
    max-width: 34rem;
  }
</style>
