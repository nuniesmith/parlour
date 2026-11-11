<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import Rail from '$lib/Rail.svelte';
  import { session } from '$lib/session.svelte';
  import type { ContinueItem, HomeData, Item } from '$lib/types';

  let home = $state<HomeData | null>(null);
  let error = $state<string | null>(null);

  onMount(async () => {
    const { data, error: err } = await api<HomeData>('/api/v1/home');
    if (data) home = data;
    else error = err ?? 'Could not load home.';
  });

  function progressFor(item: Item): number | null {
    const c = home?.continue_watching.find((x) => x.id === item.id);
    if (!c || !c.duration_ms) return null;
    return Math.min(1, c.position_ms / c.duration_ms);
  }
</script>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if !home}
  <p class="dim">Loading…</p>
{:else}
  {#if home.continue_watching.length === 0 && home.recently_added.length === 0}
    <h1>Welcome to Parlour</h1>
    <p class="dim">
      Nothing here yet. Once an admin adds a library and scans it, your movies and shows will show
      up here.
    </p>
  {/if}
  <Rail
    title="Continue watching"
    items={home.continue_watching as ContinueItem[]}
    {progressFor}
  />
  <Rail title="Next up" items={home.next_up} />
  <Rail title="Recently added" items={home.recently_added} />
  {#each session.libraries as lib (lib.id)}
    <p><a class="browse-link" href={`/library/${lib.id}`}>Browse {lib.name} →</a></p>
  {/each}
{/if}

<style>
  .browse-link {
    color: var(--accent);
  }
</style>
