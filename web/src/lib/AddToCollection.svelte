<script lang="ts">
  // POD8 (D35): an admin's "Add to collection" for a book. Collections
  // belong to the library and every profile with access sees them; only
  // admins change them (as Audiobookshelf's update permission), so this
  // control is shown to admins only and its routes are 404 to anyone else.
  import { api } from './api';
  import type { CollectionSummary } from './types';

  let { libraryId, itemId }: { libraryId: string; itemId: string } = $props();

  let open = $state(false);
  let collections = $state<CollectionSummary[] | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let newName = $state('');
  let busy = $state(false);

  async function load() {
    const params = new URLSearchParams({ library_id: libraryId, item_id: itemId });
    const r = await api<CollectionSummary[]>(`/api/v1/collections?${params}`);
    if (r.data) collections = r.data;
    else error = r.error ?? 'Could not load the collections.';
  }

  function toggle() {
    open = !open;
    error = null;
    notice = null;
    if (open) void load();
  }

  async function addTo(id: string, name: string) {
    busy = true;
    error = null;
    const r = await api(`/api/v1/admin/collections/${id}/items`, {
      method: 'POST',
      body: JSON.stringify({ item_id: itemId }),
    });
    busy = false;
    if (r.status === 201) {
      notice = `Added to “${name}”.`;
      await load();
    } else error = r.error ?? 'Could not add it.';
  }

  async function createWith(e: SubmitEvent) {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    busy = true;
    error = null;
    const made = await api<{ id: string }>('/api/v1/admin/collections', {
      method: 'POST',
      body: JSON.stringify({ library_id: libraryId, name }),
    });
    if (!made.data) {
      busy = false;
      error = made.error ?? 'Could not make the collection.';
      return;
    }
    newName = '';
    await addTo(made.data.id, name);
  }
</script>

<div class="add-to">
  <button class="btn" type="button" aria-expanded={open} onclick={toggle} data-add-to-collection
    >Add to collection</button
  >
  {#if open}
    <div class="menu" role="group" aria-label="Collections">
      <p class="dim note">Collections are shared: everyone with access to this library sees them.</p>
      {#if collections === null && !error}
        <p class="dim">Loading…</p>
      {:else if collections}
        {#if collections.length === 0}
          <p class="dim">No collections in this library yet.</p>
        {/if}
        <ul>
          {#each collections as c (c.id)}
            <li>
              {#if c.contains}
                <a href={`/collection/${c.id}`} class="in">✓ In “{c.name}”</a>
              {:else}
                <button type="button" class="pick" disabled={busy} onclick={() => addTo(c.id, c.name)}
                  >{c.name}<span class="dim"> · {c.book_count}</span></button
                >
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      <form onsubmit={createWith}>
        <input
          type="text"
          placeholder="New collection"
          maxlength="200"
          bind:value={newName}
          aria-label="New collection name"
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
    min-width: 17rem;
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
  .note {
    font-size: 0.8rem !important;
  }
</style>
