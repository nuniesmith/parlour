<script lang="ts">
  // Server → Library: library definitions, their folders, and
  // the scan / metadata-match actions.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { loadSession } from '$lib/session.svelte';
  import type { LibraryDetail } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let loaded = $state(false);
  let libraries = $state<LibraryDetail[]>([]);
  let libForm = $state({ id: '', name: '', kind: 'movies', paths: '' });

  async function loadLibraries() {
    const { data, error: err } = await api<LibraryDetail[]>('/api/v1/admin/libraries');
    if (err) error = err;
    const base = data ?? [];
    // The list endpoint has no paths; fetch each detail to fill them in.
    libraries = await Promise.all(
      base.map(async (lib) => {
        const detail = await api<LibraryDetail>(`/api/v1/admin/libraries/${lib.id}`);
        return detail.data ?? { ...lib, paths: [] };
      }),
    );
    loaded = true;
  }

  onMount(loadLibraries);

  function editLibrary(lib: LibraryDetail) {
    libForm = { id: lib.id, name: lib.name, kind: lib.kind, paths: lib.paths.join('\n') };
  }

  async function saveLibrary(event: SubmitEvent) {
    event.preventDefault();
    error = null;
    notice = null;
    const paths = libForm.paths
      .split('\n')
      .map((p) => p.trim())
      .filter(Boolean);
    const body = { name: libForm.name.trim(), kind: libForm.kind, paths };
    const isNew = !libForm.id;
    const { status, error: err } = await api(
      isNew ? '/api/v1/admin/libraries' : `/api/v1/admin/libraries/${libForm.id}`,
      { method: isNew ? 'POST' : 'PATCH', body: JSON.stringify(body) },
    );
    if (status === 200 || status === 201) {
      libForm = { id: '', name: '', kind: 'movies', paths: '' };
      notice = 'Library saved.';
      await loadLibraries();
      // The sidebar's Libraries section reads the session store.
      void loadSession();
    } else error = err ?? 'Could not save the library.';
  }

  async function deleteLibrary(lib: LibraryDetail) {
    if (!confirm(`Delete library "${lib.name}"? Its catalog goes with it (files stay on disk).`))
      return;
    await api(`/api/v1/admin/libraries/${lib.id}`, { method: 'DELETE' });
    await loadLibraries();
    void loadSession();
  }

  async function runJob(path: string, done: string) {
    error = null;
    notice = null;
    const { status, error: err } = await api(path, { method: 'POST' });
    if (status === 202) notice = done;
    else error = err ?? 'Could not start that job.';
  }
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}
  <p class="notice" role="status">{notice} <a class="link" href="/admin/jobs">Open Jobs</a></p>
{/if}

<h2>Libraries</h2>
{#if loaded && libraries.length === 0}
  <p class="dim">No libraries yet. Add one below, then scan it.</p>
{/if}
<div class="table-wrap"><table>
  <thead><tr><th>Name</th><th>Kind</th><th>Paths</th><th>Actions</th></tr></thead>
  <tbody>
    {#each libraries as lib (lib.id)}
      <tr>
        <td>{lib.name}</td>
        <td>{lib.kind}</td>
        <td class="dim mono">{lib.paths.join(', ')}</td>
        <td><div class="actions">
          <button class="btn small" onclick={() => editLibrary(lib)}>Edit</button>
          <button
            class="btn small"
            onclick={() => runJob(`/api/v1/admin/libraries/${lib.id}/scan`, 'Scan started.')}
            >Scan</button
          >
          <button
            class="btn small"
            onclick={() =>
              runJob(`/api/v1/admin/libraries/${lib.id}/metadata`, 'Metadata match started.')}
            >Match metadata</button
          >
          <button class="btn small danger" onclick={() => deleteLibrary(lib)}>Delete</button>
        </div></td>
      </tr>
    {/each}
  </tbody>
</table></div>

<form class="form" onsubmit={saveLibrary}>
  <h3>{libForm.id ? 'Edit library' : 'New library'}</h3>
  <div class="field">
    <label for="lname">Name</label>
    <input id="lname" bind:value={libForm.name} required />
  </div>
  <div class="field">
    <label for="lkind">Kind</label>
    <select id="lkind" bind:value={libForm.kind}>
      <option value="movies">Movies</option>
      <option value="shows">Shows</option>
      <option value="mixed">Mixed</option>
      <option value="audiobooks">Audiobooks</option>
      <option value="ebooks">Ebooks</option>
      <option value="music">Music</option>
      <option value="podcasts">Podcasts</option>
    </select>
  </div>
  <div class="field">
    <label for="lpaths">Root paths (one per line, absolute, as the server sees them)</label>
    <textarea id="lpaths" bind:value={libForm.paths} rows="3"></textarea>
  </div>
  <button class="btn primary" type="submit">Save library</button>
  {#if libForm.id}
    <button
      class="btn"
      type="button"
      onclick={() => (libForm = { id: '', name: '', kind: 'movies', paths: '' })}>Cancel edit</button
    >
  {/if}
</form>


<h2>Emby import</h2>
<p class="dim">
  Carry watched state, resume positions and favourites over from Emby.
  <a class="link" href="/admin/library/emby-import">Open Emby import</a>
</p>

<style>
  .link {
    color: var(--accent);
  }
</style>
