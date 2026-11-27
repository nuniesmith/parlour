<script lang="ts">
  // POD8 (D35): a library's collection of books, which every profile with
  // access to the library sees (with its own progress on each book). Only
  // admins change a collection (as Audiobookshelf's update permission):
  // they get rename, delete, and an editable list to reorder and remove
  // books; everyone else sees the books in the collection's order. A
  // profile without access gets the API's 404, read as "not found".
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import BookGrid from '$lib/BookGrid.svelte';
  import { session } from '$lib/session.svelte';
  import { formatDurationMs, type Book, type CollectionDetail } from '$lib/types';

  let collection = $state<CollectionDetail | null>(null);
  let missing = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let editing = $state(false);
  let arranging = $state(false);
  let name = $state('');
  let description = $state('');

  const id = $derived(page.params.id ?? '');
  const isAdmin = $derived(!!session.me?.user.is_admin);

  async function load() {
    const r = await api<CollectionDetail>(`/api/v1/collections/${id}`);
    if (r.status === 404) {
      missing = true;
      collection = null;
      return;
    }
    if (r.data) {
      collection = r.data;
      error = null;
    } else error = r.error ?? 'Could not load this collection.';
  }

  $effect(() => {
    void id;
    missing = false;
    collection = null;
    editing = false;
    arranging = false;
    void load();
  });

  function startEdit() {
    if (!collection) return;
    name = collection.name;
    description = collection.description ?? '';
    editing = true;
  }

  async function saveEdit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    const r = await api(`/api/v1/admin/collections/${id}`, {
      method: 'PATCH',
      body: JSON.stringify({ name, description }),
    });
    busy = false;
    if (r.status === 200) {
      editing = false;
      await load();
    } else error = r.error ?? 'Could not save that.';
  }

  async function move(index: number, by: number) {
    if (!collection) return;
    const order = collection.books.map((b) => b.id);
    const to = index + by;
    if (to < 0 || to >= order.length) return;
    [order[index], order[to]] = [order[to], order[index]];
    busy = true;
    const r = await api(`/api/v1/admin/collections/${id}/items`, {
      method: 'PUT',
      body: JSON.stringify({ items: order }),
    });
    busy = false;
    if (r.status !== 200) error = r.error ?? 'Could not move it.';
    await load();
  }

  async function remove(b: Book) {
    busy = true;
    const r = await api(`/api/v1/admin/collections/${id}/items/${b.id}`, { method: 'DELETE' });
    busy = false;
    if (r.status !== 204) error = r.error ?? 'Could not remove it.';
    await load();
  }

  async function deleteCollection() {
    if (!collection) return;
    if (!confirm(`Delete the collection “${collection.name}” for everyone?`)) return;
    const back = `/library/${collection.library_id}?view=collections`;
    const r = await api(`/api/v1/admin/collections/${id}`, { method: 'DELETE' });
    if (r.status === 204) await goto(back);
    else error = r.error ?? 'Could not delete it.';
  }
</script>

{#if missing}
  <h1>Not found</h1>
  <p class="dim">There is no such collection.</p>
{:else if error && !collection}
  <p class="error" role="alert">{error}</p>
{:else if !collection}
  <p class="dim">Loading…</p>
{:else}
  <header class="head">
    <p class="kicker">
      <a href={`/library/${collection.library_id}?view=collections`}>{collection.library_name}</a> ·
      Collection
    </p>
    {#if editing}
      <form class="edit" onsubmit={saveEdit}>
        <label class="field"
          >Name <input type="text" maxlength="200" bind:value={name} aria-label="Collection name" /></label
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
      <h1>{collection.name}</h1>
      {#if collection.description}<p class="description">{collection.description}</p>{/if}
      <p class="dim">
        {collection.book_count} book{collection.book_count === 1 ? '' : 's'} · shared with everyone who
        can open {collection.library_name}
      </p>
      {#if isAdmin}
        <div class="buttons">
          <button class="btn" type="button" onclick={startEdit}>Rename</button>
          <button
            class="btn"
            type="button"
            aria-pressed={arranging}
            onclick={() => (arranging = !arranging)}>{arranging ? 'Done arranging' : 'Arrange books'}</button
          >
          <button class="btn danger" type="button" onclick={deleteCollection}>Delete collection</button>
        </div>
      {/if}
    {/if}
  </header>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if collection.books.length === 0}
    <p class="dim">
      No books in it yet.{isAdmin ? ' Use “Add to collection” on a book of this library.' : ''}
    </p>
  {:else if arranging && isAdmin}
    <ol class="entries" aria-label="Books in this collection">
      {#each collection.books as b, i (b.id)}
        <li class="entry" data-title={b.title}>
          <span class="pos dim">{i + 1}</span>
          <span class="main">
            <a class="title" href={`/book/${b.id}`}>{b.title}</a>
            <span class="dim small"
              >{[b.authors.join(', '), formatDurationMs(b.duration_ms)].filter(Boolean).join(' · ')}</span
            >
          </span>
          <span class="actions">
            <button
              class="btn small"
              type="button"
              disabled={busy || i === 0}
              onclick={() => move(i, -1)}
              aria-label={`Move “${b.title}” up`}>↑</button
            >
            <button
              class="btn small"
              type="button"
              disabled={busy || i === collection.books.length - 1}
              onclick={() => move(i, 1)}
              aria-label={`Move “${b.title}” down`}>↓</button
            >
            <button
              class="btn small"
              type="button"
              disabled={busy}
              onclick={() => remove(b)}
              aria-label={`Remove “${b.title}”`}>Remove</button
            >
          </span>
        </li>
      {/each}
    </ol>
  {:else}
    <BookGrid books={collection.books} />
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
    max-width: 800px;
  }
  .entry {
    display: grid;
    grid-template-columns: 1.6rem 1fr auto;
    gap: 0.8rem;
    align-items: center;
    padding: 0.55rem 0.3rem;
    border-bottom: 1px solid var(--line);
  }
  .pos {
    text-align: right;
  }
  .main {
    display: grid;
    gap: 0.15rem;
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
  }
</style>
