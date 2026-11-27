<script lang="ts">
  // POD6: a podcasts library's page — its shows, by sort title, as the
  // active profile can reach them (POD5's `GET /api/v1/podcasts`: the
  // profile's grant, and D41's explicit flag, so a hidden show isn't
  // listed, counted or found by the filter). Each card: the show's
  // artwork (Parlour's cached copy, else an initial tile), title,
  // author, and this profile's finished / in-progress counts.
  // Current / All follows UI-AUDIO-REFERENCE §4.4: "current" shows are
  // those with an episode this profile hasn't finished.
  // POD8 (D35): the Playlists row above the shows — this profile's own
  // playlists of episodes, as round buttons (§2.2 / §4.4).
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import PlaylistsRow from '$lib/PlaylistsRow.svelte';
  import ShowArt from '$lib/ShowArt.svelte';
  import type { ListenerShow } from '$lib/types';

  let { libraryId }: { libraryId: string } = $props();

  const VIEW_KEY = 'parlour.podcasts.view';

  let shows = $state<ListenerShow[] | null>(null);
  let error = $state<string | null>(null);
  let q = $state('');
  let view = $state<'all' | 'current'>(
    (typeof localStorage !== 'undefined' && localStorage.getItem(VIEW_KEY) === 'current'
      ? 'current'
      : 'all') as 'all' | 'current',
  );
  let seq = 0;

  async function load() {
    const mine = ++seq;
    const params = new URLSearchParams({ library_id: libraryId });
    const query = q.trim();
    if (query) params.set('q', query);
    const { data, error: err } = await api<ListenerShow[]>(`/api/v1/podcasts?${params}`);
    if (mine !== seq) return;
    if (data) {
      shows = data;
      error = null;
    } else {
      error = err ?? 'Could not load the shows.';
    }
  }

  $effect(() => {
    void libraryId;
    shows = null;
    void load();
  });

  let debounce: ReturnType<typeof setTimeout> | null = null;
  function onSearch() {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(() => void load(), 250);
  }

  function setView(v: 'all' | 'current') {
    view = v;
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      // Storage blocked: the choice just won't persist.
    }
  }

  const visible = $derived(
    (shows ?? []).filter((s) => view === 'all' || s.finished_count < s.episode_count),
  );

  function countLine(s: ListenerShow): string {
    const parts = [s.episode_count === 1 ? '1 episode' : `${s.episode_count} episodes`];
    if (s.in_progress_count > 0) parts.push(`${s.in_progress_count} in progress`);
    if (s.finished_count > 0) parts.push(`${s.finished_count} played`);
    return parts.join(' · ');
  }
</script>

<PlaylistsRow {libraryId} kind="podcasts" round />

<h2 class="shows-head">Shows</h2>
<div class="toolbar">
  <label class="search">
    <span class="sr-only">Filter shows</span>
    <input
      type="search"
      placeholder="Filter by title or author"
      bind:value={q}
      oninput={onSearch}
      aria-label="Filter shows"
    />
  </label>
  <div class="seg" role="group" aria-label="Which shows">
    <button
      type="button"
      class:on={view === 'current'}
      aria-pressed={view === 'current'}
      onclick={() => setView('current')}>Current</button
    >
    <button
      type="button"
      class:on={view === 'all'}
      aria-pressed={view === 'all'}
      onclick={() => setView('all')}>All</button
    >
  </div>
</div>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if shows === null}
  <p class="dim">Loading…</p>
{:else if shows.length === 0}
  <p class="dim">
    {#if q.trim()}
      No show matches “{q.trim()}”.
    {:else}
      There are no shows in this library yet.
      {#if session.me?.user.is_admin}Add them on <a href="/admin/podcasts">Server → Podcasts</a>.{/if}
    {/if}
  </p>
{:else if visible.length === 0}
  <p class="dim">You have played every episode of every show here. Choose All to see them.</p>
{:else}
  <ul class="grid" aria-label="Shows">
    {#each visible as s (s.id)}
      <li>
        <a class="card" href={`/podcast/${s.id}`} data-show-id={s.id}>
          <ShowArt url={s.artwork_url} title={s.title} />
          <span class="title">{s.title}</span>
          {#if s.author}<span class="author">{s.author}</span>{/if}
          <span class="counts dim">{countLine(s)}</span>
        </a>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .shows-head {
    font-size: 1.15rem;
    margin: 0.4rem 0;
  }
  .toolbar {
    display: flex;
    flex-wrap: wrap;
    gap: 0.8rem;
    align-items: center;
    margin: 0.4rem 0 1.2rem;
  }
  .search input {
    min-width: 16rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.45rem 0.6rem;
    color: var(--text);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: 999px;
    overflow: hidden;
  }
  .seg button {
    background: transparent;
    border: 0;
    color: var(--text-dim);
    padding: 0.35rem 0.9rem;
    font: inherit;
    cursor: pointer;
  }
  .seg button.on {
    background: var(--accent);
    color: var(--accent-ink);
  }
  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 1.2rem 1rem;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    color: inherit;
    text-decoration: none;
  }
  .card:hover .title,
  .card:focus-visible .title {
    color: var(--accent);
  }
  .title {
    font-weight: 600;
    margin-top: 0.35rem;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .author {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .counts {
    font-size: 0.8rem;
  }
</style>
