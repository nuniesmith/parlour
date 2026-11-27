<script lang="ts">
  // POD8 (D35): "Add to playlist" for a book or an episode. The menu lists
  // this profile's playlists in the item's library (a playlist lives in
  // one library, so a book only ever goes into a book playlist and an
  // episode into an episode one), marks the ones already holding it, and
  // can start a new playlist with it. Playlists are the profile's own.
  import { api } from './api';
  import type { PlaylistDetail, PlaylistSummary } from './types';

  let {
    libraryId,
    itemId = null,
    episodeId = null,
    small = false,
  }: { libraryId: string; itemId?: string | null; episodeId?: string | null; small?: boolean } =
    $props();

  let open = $state(false);
  let playlists = $state<PlaylistSummary[] | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let newName = $state('');
  let busy = $state(false);

  const target = $derived(itemId ? { item_id: itemId } : { episode_id: episodeId });

  async function load() {
    const params = new URLSearchParams({ library_id: libraryId });
    if (itemId) params.set('item_id', itemId);
    if (episodeId) params.set('episode_id', episodeId);
    const r = await api<PlaylistSummary[]>(`/api/v1/playlists?${params}`);
    if (r.data) playlists = r.data;
    else error = r.error ?? 'Could not load your playlists.';
  }

  function toggle() {
    open = !open;
    error = null;
    notice = null;
    if (open) void load();
  }

  async function addTo(p: PlaylistSummary) {
    busy = true;
    error = null;
    const r = await api<PlaylistDetail>(`/api/v1/playlists/${p.id}/items`, {
      method: 'POST',
      body: JSON.stringify(target),
    });
    busy = false;
    if (r.data) {
      notice = `Added to “${p.name}”.`;
      await load();
    } else error = r.error ?? 'Could not add it.';
  }

  async function createWith(e: SubmitEvent) {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    busy = true;
    error = null;
    const made = await api<PlaylistDetail>('/api/v1/playlists', {
      method: 'POST',
      body: JSON.stringify({ library_id: libraryId, name }),
    });
    if (!made.data) {
      busy = false;
      error = made.error ?? 'Could not make the playlist.';
      return;
    }
    newName = '';
    await addTo(made.data);
  }
</script>

<div class="add-to">
  <button
    class="btn"
    class:small
    type="button"
    aria-expanded={open}
    onclick={toggle}
    data-add-to-playlist>Add to playlist</button
  >
  {#if open}
    <div class="menu" role="group" aria-label="Your playlists">
      {#if playlists === null && !error}
        <p class="dim">Loading…</p>
      {:else if playlists}
        {#if playlists.length === 0}
          <p class="dim">You have no playlists in this library yet.</p>
        {/if}
        <ul>
          {#each playlists as p (p.id)}
            <li>
              {#if p.contains}
                <a href={`/playlist/${p.id}`} class="in">✓ In “{p.name}”</a>
              {:else}
                <button type="button" class="pick" disabled={busy} onclick={() => addTo(p)}
                  >{p.name}<span class="dim"> · {p.item_count}</span></button
                >
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      <form onsubmit={createWith}>
        <input
          type="text"
          placeholder="New playlist"
          maxlength="200"
          bind:value={newName}
          aria-label="New playlist name"
        />
        <button class="btn small" type="submit" disabled={busy || !newName.trim()}>Create</button>
      </form>
      {#if notice}<p class="ok" role="status">{notice}</p>{/if}
      {#if error}<p class="error" role="alert">{error}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .add-to {
    position: relative;
    display: inline-block;
  }
  .menu {
    position: absolute;
    z-index: 20;
    top: calc(100% + 0.3rem);
    left: 0;
    min-width: 16rem;
    background: var(--bg-card);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.6rem;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  }
  ul {
    list-style: none;
    margin: 0 0 0.5rem;
    padding: 0;
  }
  .pick,
  .in {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-radius: 0.4rem;
    padding: 0.4rem 0.5rem;
    color: inherit;
    font: inherit;
    cursor: pointer;
    text-decoration: none;
  }
  .pick:hover,
  .pick:focus-visible {
    background: var(--bg-raise);
  }
  .in {
    color: var(--accent);
  }
  form {
    display: flex;
    gap: 0.4rem;
  }
  input {
    flex: 1;
    min-width: 0;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.35rem 0.5rem;
    color: var(--text);
  }
  .ok {
    color: var(--accent);
    margin: 0.4rem 0 0;
    font-size: 0.88rem;
  }
  .menu p {
    margin: 0.3rem 0;
    font-size: 0.88rem;
  }
</style>
