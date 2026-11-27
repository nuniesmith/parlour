<script lang="ts">
  // POD8 (D35): one of this profile's playlists — its books or episodes
  // in order, with Play / Open, move up and down, remove, rename and
  // delete. A playlist is the profile's own: any other profile gets the
  // API's 404, read here as "not found". Entries the profile can't reach
  // (no grant, or an explicit show with D41's flag off) aren't shown.
  // Removing the last entry deletes the playlist, as Audiobookshelf does.
  // Playing a whole playlist in order waits for the persistent player
  // bar (UI-AUDIO-REFERENCE §4.2); each entry opens its own player.
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import EpisodeMark from '$lib/EpisodeMark.svelte';
  import ShowArt from '$lib/ShowArt.svelte';
  import {
    episodeFraction,
    episodeState,
    formatDate,
    formatDurationMs,
    posterUrl,
    type PlaylistDetail,
    type PlaylistEntry,
  } from '$lib/types';

  let playlist = $state<PlaylistDetail | null>(null);
  let missing = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let editing = $state(false);
  let name = $state('');
  let description = $state('');

  const id = $derived(page.params.id ?? '');

  async function load() {
    const r = await api<PlaylistDetail>(`/api/v1/playlists/${id}?client=web`);
    if (r.status === 404) {
      missing = true;
      playlist = null;
      return;
    }
    if (r.data) {
      playlist = r.data;
      error = null;
    } else error = r.error ?? 'Could not load this playlist.';
  }

  $effect(() => {
    void id;
    missing = false;
    playlist = null;
    editing = false;
    void load();
  });

  function startEdit() {
    if (!playlist) return;
    name = playlist.name;
    description = playlist.description ?? '';
    editing = true;
  }

  async function saveEdit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    const r = await api<PlaylistDetail>(`/api/v1/playlists/${id}`, {
      method: 'PATCH',
      body: JSON.stringify({ name, description }),
    });
    busy = false;
    if (r.data) {
      playlist = r.data;
      editing = false;
      error = null;
    } else error = r.error ?? 'Could not save that.';
  }

  async function move(index: number, by: number) {
    if (!playlist) return;
    const order = playlist.items.map((e) => e.id);
    const to = index + by;
    if (to < 0 || to >= order.length) return;
    [order[index], order[to]] = [order[to], order[index]];
    busy = true;
    const r = await api<PlaylistDetail>(`/api/v1/playlists/${id}/items`, {
      method: 'PUT',
      body: JSON.stringify({ items: order }),
    });
    busy = false;
    if (r.data) {
      playlist = r.data;
      error = null;
    } else {
      error = r.error ?? 'Could not move it.';
      if (r.status === 409) await load();
    }
  }

  async function remove(entry: PlaylistEntry) {
    if (!playlist) return;
    const last = playlist.items.length === 1;
    if (last && !confirm('This is the last one: removing it deletes the playlist. Remove it?')) {
      return;
    }
    busy = true;
    const libraryId = playlist.library_id;
    const r = await api<{ playlist_deleted: boolean }>(
      `/api/v1/playlists/${id}/items/${entry.id}`,
      { method: 'DELETE' },
    );
    busy = false;
    if (!r.data) {
      error = r.error ?? 'Could not remove it.';
      return;
    }
    if (r.data.playlist_deleted) {
      await goto(`/library/${libraryId}${playlist.library_kind === 'audiobooks' ? '?view=collections' : ''}`);
      return;
    }
    await load();
  }

  async function deletePlaylist() {
    if (!playlist || !confirm(`Delete the playlist “${playlist.name}”?`)) return;
    const back = `/library/${playlist.library_id}${playlist.library_kind === 'audiobooks' ? '?view=collections' : ''}`;
    const r = await api(`/api/v1/playlists/${id}`, { method: 'DELETE' });
    if (r.status === 204) await goto(back);
    else error = r.error ?? 'Could not delete it.';
  }

  function entryTitle(e: PlaylistEntry): string {
    return e.book?.title ?? e.episode?.title ?? '';
  }

  function entryLine(e: PlaylistEntry): string {
    if (e.book) {
      const p = e.book.progress;
      const state = p.finished
        ? 'Finished'
        : p.position_ms > 0
          ? `${formatDurationMs(Math.max(0, e.book.duration_ms - p.position_ms)) || '0:00'} left`
          : formatDurationMs(e.book.duration_ms);
      return [e.book.authors.join(', '), state].filter(Boolean).join(' · ');
    }
    if (e.episode) {
      const ep = e.episode;
      const st = episodeState(ep.progress);
      const len =
        st === 'in_progress' && ep.length_ms
          ? `${formatDurationMs(Math.max(0, ep.length_ms - ep.progress.position_ms)) || '0:00'} left`
          : formatDurationMs(ep.length_ms);
      return [ep.show_title, formatDate(ep.published_at), len].filter(Boolean).join(' · ');
    }
    return '';
  }

  function playHref(e: PlaylistEntry): string {
    return e.book ? `/book/${e.book.id}?play=1` : `/episode/${e.episode?.id}?play=1`;
  }

  const what = $derived(playlist?.library_kind === 'podcasts' ? 'episode' : 'book');
</script>

{#if missing}
  <h1>Not found</h1>
  <p class="dim">There is no such playlist.</p>
{:else if error && !playlist}
  <p class="error" role="alert">{error}</p>
{:else if !playlist}
  <p class="dim">Loading…</p>
{:else}
  <header class="head">
    <p class="kicker">
      <a href={`/library/${playlist.library_id}${playlist.library_kind === 'audiobooks' ? '?view=collections' : ''}`}
        >{playlist.library_name}</a
      > · Playlist
    </p>
    {#if editing}
      <form class="edit" onsubmit={saveEdit}>
        <label class="field"
          >Name <input type="text" maxlength="200" bind:value={name} aria-label="Playlist name" /></label
        >
        <label class="field"
          >Description <textarea rows="2" maxlength="4000" bind:value={description}></textarea></label
        >
        <div class="buttons">
          <button class="btn primary" type="submit" disabled={busy || !name.trim()}>Save</button>
          <button class="btn" type="button" onclick={() => (editing = false)}>Cancel</button>
        </div>
      </form>
    {:else}
      <h1>{playlist.name}</h1>
      {#if playlist.description}<p class="description">{playlist.description}</p>{/if}
      <p class="dim">
        {playlist.item_count}
        {what}{playlist.item_count === 1 ? '' : 's'}{playlist.duration_ms
          ? ` · ${formatDurationMs(playlist.duration_ms)}`
          : ''}
      </p>
      <div class="buttons">
        <button class="btn" type="button" onclick={startEdit}>Rename</button>
        <button class="btn danger" type="button" onclick={deletePlaylist}>Delete playlist</button>
      </div>
    {/if}
  </header>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if playlist.items.length === 0}
    <p class="dim">
      Nothing here yet. Use “Add to playlist” on {what === 'episode' ? 'an episode' : 'a book'} of
      {playlist.library_name}.
    </p>
  {:else}
    <ol class="entries" aria-label="In this playlist">
      {#each playlist.items as e, i (e.id)}
        <li class="entry" data-entry-id={e.id} data-title={entryTitle(e)}>
          <span class="pos dim">{i + 1}</span>
          {#if e.book}
            <span class="cover">
              {#if e.book.has_cover}
                <img src={posterUrl(e.book.id)} alt="" loading="lazy" />
              {:else}
                <span class="tile" aria-hidden="true">{e.book.title.charAt(0).toUpperCase()}</span>
              {/if}
            </span>
          {:else if e.episode}
            <span class="art-wrap">
              <ShowArt url={e.episode.artwork_url} title={e.episode.show_title} size="sm" />
              <EpisodeMark status={episodeState(e.episode.progress)} fraction={episodeFraction(e.episode)} />
            </span>
          {/if}
          <span class="main">
            <a class="title" href={e.book ? `/book/${e.book.id}` : `/episode/${e.episode?.id}`}
              >{entryTitle(e)}</a
            >
            <span class="dim small">{entryLine(e)}</span>
          </span>
          <span class="actions">
            <a class="btn small primary" href={playHref(e)}>Play</a>
            <button
              class="btn small"
              type="button"
              disabled={busy || i === 0}
              onclick={() => move(i, -1)}
              aria-label={`Move “${entryTitle(e)}” up`}>↑</button
            >
            <button
              class="btn small"
              type="button"
              disabled={busy || i === playlist.items.length - 1}
              onclick={() => move(i, 1)}
              aria-label={`Move “${entryTitle(e)}” down`}>↓</button
            >
            <button
              class="btn small"
              type="button"
              disabled={busy}
              onclick={() => remove(e)}
              aria-label={`Remove “${entryTitle(e)}”`}>Remove</button
            >
          </span>
        </li>
      {/each}
    </ol>
  {/if}
{/if}

<style>
  .head {
    margin: 1rem 0 1.2rem;
  }
  .kicker {
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-size: 0.78rem;
    color: var(--text-dim);
    margin: 0;
  }
  .kicker a {
    color: inherit;
  }
  h1 {
    margin: 0.2rem 0 0.4rem;
  }
  .description {
    max-width: 60ch;
    line-height: 1.5;
    margin: 0.2rem 0;
  }
  .buttons {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.6rem;
    flex-wrap: wrap;
  }
  .edit {
    max-width: 32rem;
  }
  .entries {
    list-style: none;
    margin: 0;
    padding: 0;
    max-width: 900px;
  }
  .entry {
    display: grid;
    grid-template-columns: 1.6rem 3.2rem 1fr auto;
    gap: 0.8rem;
    align-items: center;
    padding: 0.55rem 0.3rem;
    border-bottom: 1px solid var(--line);
  }
  .pos {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .cover {
    width: 3.2rem;
    height: 3.2rem;
    border-radius: 0.4rem;
    overflow: hidden;
    background: var(--bg-raise);
    border: 1px solid var(--line);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .tile {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--text-dim);
  }
  .art-wrap {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }
  .main {
    display: grid;
    gap: 0.15rem;
    min-width: 0;
  }
  .title {
    color: inherit;
    font-weight: 600;
  }
  .small {
    font-size: 0.85rem;
  }
  .actions {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  @media (max-width: 640px) {
    .entry {
      grid-template-columns: 1.2rem 3rem 1fr;
    }
    .actions {
      grid-column: 2 / -1;
      justify-content: flex-start;
    }
  }
</style>
