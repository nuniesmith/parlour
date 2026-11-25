<script lang="ts">
  // Server → Library: library definitions, their folders, and
  // the scan / metadata-match / artwork actions. Also the per-library
  // opt-in to writing next to the media (XFER5, D14): off by default,
  // turned on only here, behind a confirm that says what gets written.
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
        return detail.data ?? { ...lib, paths: [], write_sidecars: false, folders: [] };
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

  // Video libraries only: the writer covers movies and shows today.
  const SIDECAR_KINDS = ['movies', 'shows'];

  async function setWriteSidecars(lib: LibraryDetail, enabled: boolean) {
    error = null;
    notice = null;
    if (enabled) {
      const sure = confirm(
        `Let Parlour write files next to the media of "${lib.name}"?\n\n` +
          'It will write a .parlour.json and a .nfo beside each matched video, ' +
          'series.parlour.json and tvshow.nfo in each series folder, and poster.jpg / ' +
          'backdrop.jpg in folders that hold one item. It never deletes anything, and never ' +
          "replaces a .nfo or a poster.jpg it didn't write.\n\n" +
          "Nothing is written until the library's folders are also mounted writable on the " +
          'server host. That is a change you make outside Parlour.',
      );
      if (!sure) return;
    }
    const { data, error: err } = await api<LibraryDetail>(
      `/api/v1/admin/libraries/${lib.id}/write-sidecars`,
      { method: 'PUT', body: JSON.stringify({ enabled }) },
    );
    if (data) {
      libraries = libraries.map((l) => (l.id === data.id ? data : l));
      notice = enabled
        ? `${lib.name} may now write next to its media. Sidecars and .nfo files are written by the next metadata match, poster and backdrop copies by the next artwork download.`
        : `${lib.name} no longer writes next to its media. Files already written stay where they are.`;
    } else error = err ?? 'Could not change that setting.';
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
{:else}
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
          <button
            class="btn small"
            onclick={() =>
              runJob(`/api/v1/admin/libraries/${lib.id}/artwork`, 'Artwork download started.')}
            >Download artwork</button
          >
          <button class="btn small danger" onclick={() => deleteLibrary(lib)}>Delete</button>
        </div></td>
      </tr>
    {/each}
  </tbody>
</table></div>
{/if}

{#if libraries.some((l) => SIDECAR_KINDS.includes(l.kind))}
  <h2>Writing next to the media</h2>
  <p class="dim">
    Off by default: Parlour only reads your media folders. A library that is turned on gets
    its metadata saved beside its files (<span class="mono">.parlour.json</span>, plus a
    <span class="mono">.nfo</span> for other media software and
    <span class="mono">poster.jpg</span> / <span class="mono">backdrop.jpg</span>), so it travels
    when the files move. Two things must both be true before anything is written: the switch
    here is on, and the server host mounts that library's folders writable.
  </p>
  <div class="sidecar-list">
    {#each libraries.filter((l) => SIDECAR_KINDS.includes(l.kind)) as lib (lib.id)}
      <section class="sidecar-card" aria-label={`Writing next to the media of ${lib.name}`}>
        <div class="sidecar-head">
          <h3>{lib.name}</h3>
          <span class={lib.write_sidecars ? 'state on' : 'state off'}
            >{lib.write_sidecars ? 'Writes next to the media' : 'Read-only (default)'}</span
          >
          {#if lib.write_sidecars}
            <button class="btn small" onclick={() => setWriteSidecars(lib, false)}>Turn off</button>
          {:else}
            <button class="btn small" onclick={() => setWriteSidecars(lib, true)}
              >Let Parlour write here…</button
            >
          {/if}
        </div>
        {#if lib.folders.length === 0}
          <p class="dim">This library has no folders yet.</p>
        {:else}
          <ul class="folders">
            {#each lib.folders as f (f.path)}
              <li>
                <span class="mono">{f.path}</span>
                {#if f.writable}
                  <span class="ok">writable</span>
                {:else}
                  <span class={lib.write_sidecars ? 'bad' : 'dim'}
                    >not writable{f.problem ? ` (${f.problem})` : ''}</span
                  >
                {/if}
              </li>
            {/each}
          </ul>
          {#if lib.write_sidecars && lib.folders.some((f) => !f.writable)}
            <p class="error" role="alert">
              Nothing can be written in the folders marked not writable. Mount them writable on
              the server host (docs/IMPORT-EXPORT.md §13.11, "The mount step"), or turn this off.
              Failed writes are listed on the job that tried them.
            </p>
          {/if}
        {/if}
      </section>
    {/each}
  </div>
{/if}

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
  .sidecar-list {
    display: grid;
    gap: 0.75rem;
    margin-bottom: 1.5rem;
  }
  .sidecar-card {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg-card);
    padding: 0.75rem 1rem;
  }
  .sidecar-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
  }
  .sidecar-head h3 {
    margin: 0;
    font-size: 1rem;
  }
  .state {
    font-size: 0.85rem;
  }
  .state.on {
    color: var(--accent);
    font-weight: 600;
  }
  .state.off {
    color: var(--text-dim);
  }
  .folders {
    list-style: none;
    margin: 0.5rem 0 0;
    padding: 0;
    display: grid;
    gap: 0.25rem;
  }
  .folders li {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    overflow-wrap: anywhere;
  }
  .ok {
    color: var(--ok);
  }
  .bad {
    color: var(--danger);
  }
</style>
