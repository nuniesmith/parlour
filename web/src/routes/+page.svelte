<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import Rail from '$lib/Rail.svelte';
  import { session } from '$lib/session.svelte';
  import type { ContinueItem, HomeData, Item } from '$lib/types';

  // Rails in the active profile's order, minus the ones it hid
  // (Preferences → Home Screen). Before a profile loads, or for a
  // rail id this build doesn't know, fall back to the defaults.
  const DEFAULT_RAILS = ['continue_watching', 'next_up', 'recently_added'];
  const rails = $derived(
    (
      session.me?.active_profile?.home_rails ??
      DEFAULT_RAILS.map((id) => ({ id, visible: true }))
    )
      .filter((r) => r.visible)
      .map((r) => r.id),
  );

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
    <h2>Welcome to Parlour</h2>
    <p class="dim">
      Nothing here yet. Once an admin adds a library and scans it, your movies and shows will show
      up here.
    </p>
  {/if}
  {#each rails as id (id)}
    {#if id === 'continue_watching'}
      <Rail
        title="Continue watching"
        items={home.continue_watching as ContinueItem[]}
        {progressFor}
      />
    {:else if id === 'next_up'}
      <Rail title="Next up" items={home.next_up} />
    {:else if id === 'recently_added'}
      <Rail title="Recently added" items={home.recently_added} />
    {/if}
  {/each}
  {#each session.libraries as lib (lib.id)}
    <p><a class="browse-link" href={`/library/${lib.id}`}>Browse {lib.name} →</a></p>
  {/each}
{/if}

<style>
  .browse-link {
    color: var(--accent);
  }
</style>
