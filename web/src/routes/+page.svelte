<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import AudioShelf from '$lib/AudioShelf.svelte';
  import LibraryTiles from '$lib/LibraryTiles.svelte';
  import Rail from '$lib/Rail.svelte';
  import { session } from '$lib/session.svelte';
  import {
    AUDIO_RAILS,
    isAudioRail,
    type AudioShelves,
    type ContinueItem,
    type HomeData,
    type Item,
  } from '$lib/types';

  // Rails in the active profile's order, minus the ones it hid
  // (Preferences → Home Screen). Before a profile loads, or for a
  // rail id this build doesn't know, fall back to the defaults.
  // AB8: the audiobook shelves (D29) are rails too, filled from
  // /api/v1/audiobooks/home across the profile's audiobooks libraries.
  const DEFAULT_RAILS = [
    'libraries',
    'continue_watching',
    'next_up',
    'recently_added',
    'latest',
    ...AUDIO_RAILS,
  ];
  const rails = $derived(
    (
      session.me?.active_profile?.home_rails ??
      DEFAULT_RAILS.map((id) => ({ id, visible: true }))
    )
      .filter((r) => r.visible)
      .map((r) => r.id),
  );

  let home = $state<HomeData | null>(null);
  let shelves = $state<AudioShelves | null>(null);
  let error = $state<string | null>(null);

  async function loadShelves() {
    const { data } = await api<AudioShelves>('/api/v1/audiobooks/home');
    if (data) shelves = data;
  }

  onMount(async () => {
    const shelvesLoad = rails.some(isAudioRail) ? loadShelves() : Promise.resolve();
    const { data, error: err } = await api<HomeData>('/api/v1/home');
    if (data) home = data;
    else error = err ?? 'Could not load home.';
    await shelvesLoad;
  });

  function shelfHas(id: string): boolean {
    if (!shelves) return false;
    switch (id) {
      case 'continue_listening':
        return shelves.continue_listening.length > 0;
      case 'continue_series':
        return shelves.continue_series.length > 0;
      case 'recently_added_books':
        return shelves.recently_added.length > 0;
      case 'recent_series':
        return shelves.recent_series.length > 0;
      case 'discover_books':
        return shelves.discover.length > 0;
      case 'listen_again':
        return shelves.listen_again.length > 0;
      case 'newest_authors':
        return shelves.newest_authors.length > 0;
      default:
        return false;
    }
  }

  // UI11: never a blank Home. An empty catalog gets the welcome;
  // a catalog whose every visible rail is empty (or hidden in
  // Preferences → Home Screen) says so instead of showing nothing.
  const catalogEmpty = $derived(
    !!home && home.continue_watching.length === 0 && home.recently_added.length === 0,
  );
  const anyRailShown = $derived(
    !!home &&
      rails.some((id) => {
        if (id === 'libraries') return home!.libraries.length > 0;
        if (id === 'continue_watching') return home!.continue_watching.length > 0;
        if (id === 'next_up') return home!.next_up.length > 0;
        if (id === 'recently_added') return home!.recently_added.length > 0;
        if (id === 'latest') return home!.latest.length > 0;
        return shelfHas(id);
      }),
  );

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
  {#if catalogEmpty}
    <h2>Welcome to Parlour</h2>
    <p class="dim">
      Nothing here yet. Once an admin adds a library and scans it, your movies and shows will show
      up here.
    </p>
    {#if session.me?.user.is_admin}
      <p><a class="btn primary" href="/admin/library">Add a library</a></p>
    {/if}
  {:else if !anyRailShown}
    <h2>Nothing to show on Home</h2>
    <p class="dim">
      Every Home rail that has something in it is turned off for this profile. Choose which rails
      to show in <a class="link" href="/settings/home">Preferences → Home Screen</a>.
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
    {:else if id === 'libraries'}
      <LibraryTiles libraries={home.libraries} />
    {:else if id === 'latest'}
      {#each home.latest as rail (rail.library_id)}
        <Rail
          title={`Latest in ${rail.library_name}`}
          href={`/library/${rail.library_id}`}
          items={rail.items}
        />
      {/each}
    {:else if shelves && isAudioRail(id)}
      <AudioShelf {id} {shelves} onchanged={loadShelves} />
    {/if}
  {/each}
{/if}

<style>
  .link {
    color: var(--accent);
  }
</style>
