<script lang="ts">
  // POD8 (D35): this profile's own playlists in one library, with a form
  // for a new (empty) one. Shown in an audiobooks library's Collections
  // view, beside its shared collections (UI-AUDIO-REFERENCE §4.2), and as
  // the podcasts library's Playlists row (§2.2 / §4.4), where round
  // buttons sit in a row. Overcast's row starts with built-in filtered
  // views; those come with the podcast Home task, before these.
  import { api } from './api';
  import ListTile from './ListTile.svelte';
  import { formatDurationMs, type PlaylistDetail, type PlaylistSummary } from './types';

  let {
    libraryId,
    round = false,
    kind,
  }: { libraryId: string; round?: boolean; kind: 'audiobooks' | 'podcasts' } = $props();

  let playlists = $state<PlaylistSummary[] | null>(null);
  let error = $state<string | null>(null);
  let adding = $state(false);
  let newName = $state('');
  let busy = $state(false);

  async function load() {
    const r = await api<PlaylistSummary[]>(
      `/api/v1/playlists?library_id=${encodeURIComponent(libraryId)}`,
    );
    if (r.data) {
      playlists = r.data;
      error = null;
    } else error = r.error ?? 'Could not load your playlists.';
  }

  $effect(() => {
    void libraryId;
    playlists = null;
    void load();
  });

  async function create(e: SubmitEvent) {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    busy = true;
    const r = await api<PlaylistDetail>('/api/v1/playlists', {
      method: 'POST',
      body: JSON.stringify({ library_id: libraryId, name }),
    });
    busy = false;
    if (r.data) {
      newName = '';
      adding = false;
      await load();
    } else error = r.error ?? 'Could not make the playlist.';
  }

  function sub(p: PlaylistSummary): string {
    const what = kind === 'podcasts' ? 'episode' : 'book';
    const n = `${p.item_count} ${what}${p.item_count === 1 ? '' : 's'}`;
    const len = formatDurationMs(p.duration_ms);
    return len ? `${n} · ${len}` : n;
  }
</script>

<section class="playlists" aria-label="Your playlists">
  <div class="head">
    <h2>{round ? 'Playlists' : 'Your playlists'}</h2>
    {#if !adding}
      <button class="btn small" type="button" onclick={() => (adding = true)} data-new-playlist
        >New playlist</button
      >
    {/if}
  </div>
  {#if adding}
    <form onsubmit={create} class="new">
      <input
        type="text"
        placeholder="Playlist name"
        maxlength="200"
        bind:value={newName}
        aria-label="New playlist name"
      />
      <button class="btn small primary" type="submit" disabled={busy || !newName.trim()}>Create</button>
      <button class="btn small" type="button" onclick={() => (adding = false)}>Cancel</button>
    </form>
  {/if}
  {#if error}
    <p class="error" role="alert">{error}</p>
  {:else if playlists === null}
    <p class="dim">Loading…</p>
  {:else if playlists.length === 0}
    <p class="dim">
      No playlists yet. Make one here, or use “Add to playlist” on
      {kind === 'podcasts' ? 'an episode' : 'a book'}. Your playlists are yours alone.
    </p>
  {:else}
    <ul class:row={round} class:grid={!round} class="tiles">
      {#each playlists as p (p.id)}
        <li>
          <ListTile href={`/playlist/${p.id}`} name={p.name} sub={sub(p)} covers={p.covers} {round} />
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .playlists {
    margin: 1rem 0 1.6rem;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 0.8rem;
  }
  h2 {
    font-size: 1.15rem;
    margin: 0.4rem 0;
  }
  .new {
    display: flex;
    gap: 0.4rem;
    margin: 0.4rem 0 0.8rem;
    max-width: 28rem;
  }
  .new input {
    flex: 1;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.35rem 0.5rem;
    color: var(--text);
  }
  .tiles {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .row {
    display: flex;
    gap: 1rem;
    overflow-x: auto;
    padding-bottom: 0.4rem;
  }
  .row li {
    flex: 0 0 7.5rem;
    text-align: center;
  }
</style>
