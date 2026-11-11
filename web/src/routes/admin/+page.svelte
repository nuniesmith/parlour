<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { session } from '$lib/session.svelte';
  import type {
    AdminUser,
    Candidate,
    Invite,
    Job,
    LibraryDetail,
    ReviewItem,
  } from '$lib/types';

  type Tab = 'users' | 'libraries' | 'jobs' | 'review' | 'emby';
  let tab = $state<Tab>('users');
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  // users + invites
  let users = $state<AdminUser[]>([]);
  let invites = $state<Invite[]>([]);
  let newInviteLink = $state<string | null>(null);

  // libraries
  let libraries = $state<LibraryDetail[]>([]);
  let libForm = $state({ id: '', name: '', kind: 'movies', paths: '' });
  let grantProfile = $state('');
  let grantLibrary = $state('');

  // jobs
  let jobs = $state<Job[]>([]);

  interface EmbyUserOption { id: string; name: string }
  interface ProfileOption { id: string; name: string; username: string }
  interface ImportReport {
    dry_run: boolean;
    emby_items_with_state: number;
    matched: number;
    applied: number;
    unmatched: number;
    ambiguous: number;
    skipped_no_state: number;
    unmatched_items: string[];
    ambiguous_items: string[];
  }
  let embyStatus = $state<{ configured: boolean; reachable: boolean; error: string | null; users: EmbyUserOption[] } | null>(null);
  let embyProfiles = $state<ProfileOption[]>([]);
  let embyUserId = $state('');
  let embyProfileId = $state('');
  let embyReport = $state<ImportReport | null>(null);
  let embyBusy = $state(false);

  async function loadEmby() {
    const [st, pr] = await Promise.all([
      api('/api/v1/admin/emby/status'),
      api('/api/v1/admin/emby/profiles'),
    ]);
    if (st.data) {
      embyStatus = st.data as typeof embyStatus;
      if (!embyUserId && embyStatus?.users.length) embyUserId = embyStatus.users[0].id;
    }
    if (pr.data) {
      embyProfiles = pr.data as ProfileOption[];
      if (!embyProfileId && embyProfiles.length) embyProfileId = embyProfiles[0].id;
    }
    const j = await api('/api/v1/admin/jobs?limit=30');
    const list = (j.data as unknown as Job[] | null) ?? [];
    const last = list.find((x) => x.kind === 'import_emby' && x.result);
    if (last) embyReport = last.result as unknown as ImportReport;
  }

  async function runEmbyImport(dryRun: boolean) {
    embyBusy = true;
    error = null;
    const { status, error: err } = await api('/api/v1/admin/emby/import', {
      method: 'POST',
      body: JSON.stringify({ emby_user_id: embyUserId, profile_id: embyProfileId, dry_run: dryRun }),
    });
    embyBusy = false;
    if (err) {
      error = err;
      return;
    }
    notice = dryRun ? 'Dry run queued — the report lands here in a few seconds.' : 'Import queued — watch the report below.';
    // Poll the jobs feed for the finished import's report.
    for (let i = 0; i < 30; i++) {
      await new Promise((r) => setTimeout(r, 2000));
      const j = await api('/api/v1/admin/jobs?limit=30');
      const list = (j.data as unknown as Job[] | null) ?? [];
      const done = list.find((x) => x.kind === 'import_emby' && x.status === 'succeeded' && x.result);
      if (done) {
        embyReport = done.result as unknown as ImportReport;
        break;
      }
      const failed = list.find((x) => x.kind === 'import_emby' && x.status === 'failed');
      if (failed) {
        error = 'The import job failed — see Jobs.';
        break;
      }
    }
    void loadJobs();
  }

  // review queue
  let review = $state<ReviewItem[]>([]);
  let searchKind = $state('movie');
  let searchQ = $state('');
  let searchFor = $state<ReviewItem | null>(null);
  let searchResults = $state<(Candidate & { confidence: number })[]>([]);

  async function loadUsers() {
    const [u, i] = await Promise.all([
      api<AdminUser[]>('/api/v1/admin/users'),
      api<Invite[]>('/api/v1/admin/invites'),
    ]);
    users = u.data ?? [];
    invites = i.data ?? [];
  }

  async function loadLibraries() {
    const { data } = await api<LibraryDetail[]>('/api/v1/admin/libraries');
    const base = data ?? [];
    // The list endpoint has no paths; fetch each detail to fill them in.
    libraries = await Promise.all(
      base.map(async (lib) => {
        const detail = await api<LibraryDetail>(`/api/v1/admin/libraries/${lib.id}`);
        return detail.data ?? { ...lib, paths: [] };
      }),
    );
  }

  async function loadJobs() {
    const { data } = await api<Job[]>('/api/v1/admin/jobs?limit=30');
    jobs = data ?? [];
  }

  async function loadReview() {
    const { data } = await api<ReviewItem[]>('/api/v1/admin/metadata/review-queue');
    review = data ?? [];
  }

  onMount(async () => {
    await Promise.all([loadUsers(), loadLibraries(), loadJobs(), loadReview()]);
  });

  async function createInvite() {
    const { data, error: err } = await api<{ invite_path: string }>('/api/v1/admin/invites', {
      method: 'POST',
      body: JSON.stringify({}),
    });
    if (data) {
      newInviteLink = `${location.origin}${data.invite_path}`;
      await loadUsers();
    } else error = err ?? 'Could not create an invite.';
  }

  async function revokeInvite(id: string) {
    await api(`/api/v1/admin/invites/${id}`, { method: 'DELETE' });
    await loadUsers();
  }

  function editLibrary(lib: LibraryDetail) {
    libForm = { id: lib.id, name: lib.name, kind: lib.kind, paths: lib.paths.join('\n') };
  }

  async function saveLibrary(event: SubmitEvent) {
    event.preventDefault();
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
    } else error = err ?? 'Could not save the library.';
  }

  async function deleteLibrary(lib: LibraryDetail) {
    if (!confirm(`Delete library "${lib.name}"? Its catalog goes with it (files stay on disk).`))
      return;
    await api(`/api/v1/admin/libraries/${lib.id}`, { method: 'DELETE' });
    await loadLibraries();
  }

  async function runJob(path: string, done: string) {
    const { status, error: err } = await api(path, { method: 'POST' });
    if (status === 202) {
      notice = done;
      await loadJobs();
    } else error = err ?? 'Could not start that job.';
  }

  async function grantAccess() {
    if (!grantLibrary || !grantProfile) return;
    const { status, error: err } = await api(
      `/api/v1/admin/libraries/${grantLibrary}/access`,
      { method: 'POST', body: JSON.stringify({ profile_id: grantProfile }) },
    );
    if (status === 204) notice = 'Access granted.';
    else error = err ?? 'Could not grant access.';
  }

  async function applyCandidate(itemId: string, candidate: Candidate) {
    const { status, error: err } = await api(
      `/api/v1/admin/items/${itemId}/metadata/apply`,
      {
        method: 'POST',
        body: JSON.stringify({ provider_id: candidate.provider_id, provider: candidate.provider }),
      },
    );
    if (status === 200) {
      notice = 'Match applied.';
      searchFor = null;
      searchResults = [];
      await loadReview();
    } else error = err ?? 'Could not apply that match.';
  }

  async function manualSearch(event: SubmitEvent) {
    event.preventDefault();
    if (!searchFor) return;
    const { data, error: err } = await api<(Candidate & { confidence: number })[]>(
      `/api/v1/admin/metadata/search?kind=${searchKind}&q=${encodeURIComponent(searchQ)}`,
    );
    if (data) searchResults = data;
    else error = err ?? 'Search failed.';
  }
</script>

<h1>Admin</h1>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" role="status">{notice}</p>{/if}

<div class="tabs" role="tablist">
  <button class="btn" class:primary={tab === 'users'} onclick={() => (tab = 'users')}>Users & invites</button>
  <button class="btn" class:primary={tab === 'libraries'} onclick={() => (tab = 'libraries')}>Libraries</button>
  <button class="btn" class:primary={tab === 'jobs'} onclick={() => (tab = 'jobs')}>Jobs</button>
  <button class="btn" class:primary={tab === 'emby'} onclick={() => { tab = 'emby'; void loadEmby(); }}>Emby import</button>
  <button class="btn" class:primary={tab === 'review'} onclick={() => (tab = 'review')}>
    Metadata review ({review.length})
  </button>
</div>

{#if tab === 'users'}
  <h2>Users</h2>
  <table>
    <thead><tr><th>Username</th><th>Admin</th><th>Profiles</th><th>Since</th></tr></thead>
    <tbody>
      {#each users as u (u.id)}
        <tr>
          <td>{u.username}</td>
          <td>{u.is_admin ? 'yes' : '—'}</td>
          <td>{u.profile_count}</td>
          <td class="dim">{new Date(u.created_at).toLocaleDateString()}</td>
        </tr>
      {/each}
    </tbody>
  </table>

  <h2>Invites</h2>
  <p><button class="btn primary" onclick={createInvite}>Create invite</button></p>
  {#if newInviteLink}
    <p class="invite-link">
      Share this link (shown once — the raw token is never stored):
      <code>{newInviteLink}</code>
    </p>
  {/if}
  <table>
    <thead><tr><th>Created</th><th>Expires</th><th>Status</th><th></th></tr></thead>
    <tbody>
      {#each invites as inv (inv.id)}
        <tr>
          <td class="dim">{new Date(inv.created_at).toLocaleDateString()}</td>
          <td class="dim">{new Date(inv.expires_at).toLocaleDateString()}</td>
          <td>{inv.used_at ? 'Used' : 'Open'}</td>
          <td>
            {#if !inv.used_at}
              <button class="btn small danger" onclick={() => revokeInvite(inv.id)}>Revoke</button>
            {/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

{#if tab === 'libraries'}
  <h2>Libraries</h2>
  <table>
    <thead><tr><th>Name</th><th>Kind</th><th>Paths</th><th>Actions</th></tr></thead>
    <tbody>
      {#each libraries as lib (lib.id)}
        <tr>
          <td>{lib.name}</td>
          <td>{lib.kind}</td>
          <td class="dim paths">{lib.paths.join(', ')}</td>
          <td class="actions">
            <button class="btn small" onclick={() => editLibrary(lib)}>Edit</button>
            <button
              class="btn small"
              onclick={() => runJob(`/api/v1/admin/libraries/${lib.id}/scan`, 'Scan started — see Jobs.')}
              >Scan</button
            >
            <button
              class="btn small"
              onclick={() =>
                runJob(`/api/v1/admin/libraries/${lib.id}/metadata`, 'Metadata match started — see Jobs.')}
              >Match metadata</button
            >
            <button class="btn small danger" onclick={() => deleteLibrary(lib)}>Delete</button>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>

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

  <h3>Grant a profile access</h3>
  <p class="dim">
    Access is per profile (your own profiles are listed; other users manage theirs by signing in —
    grants here apply to any profile id).
  </p>
  <div class="row">
    <select bind:value={grantLibrary} aria-label="Library">
      <option value="">Library…</option>
      {#each libraries as lib (lib.id)}<option value={lib.id}>{lib.name}</option>{/each}
    </select>
    <select bind:value={grantProfile} aria-label="Profile">
      <option value="">Profile…</option>
      {#each session.profiles as p (p.id)}<option value={p.id}>{p.name} (yours)</option>{/each}
    </select>
    <button class="btn" onclick={grantAccess}>Grant</button>
  </div>
{/if}

{#if tab === 'jobs'}
  <h2>Recent jobs</h2>
  <p><button class="btn" onclick={loadJobs}>Refresh</button></p>
  <table>
    <thead><tr><th>Kind</th><th>Status</th><th>Progress</th><th>Result</th><th>When</th></tr></thead>
    <tbody>
      {#each jobs as job (job.id)}
        <tr>
          <td>{job.kind}</td>
          <td>{job.status}{#if job.errors.length} <span class="error">({job.errors.length} errors)</span>{/if}</td>
          <td>{job.progress_done}/{job.progress_total}</td>
          <td class="dim">{job.result ? JSON.stringify(job.result) : '—'}</td>
          <td class="dim">{new Date(job.created_at).toLocaleString()}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

{#if tab === 'review'}
  <h2>Needs review</h2>
  {#if review.length === 0}
    <p class="dim">Nothing waiting. The matcher only parks items here when it isn't sure.</p>
  {/if}
  {#each review as item (item.id)}
    <div class="review-item">
      <h3>{item.title}{#if item.year} ({item.year}){/if} <span class="chip">{item.kind}</span></h3>
      <div class="candidates">
        {#each item.candidates as cand (cand.provider_id)}
          <div class="candidate">
            {#if cand.poster_url}<img src={cand.poster_url} alt="" loading="lazy" />{/if}
            <div>
              <strong>{cand.title}</strong>
              <span class="dim">
                {cand.year ?? ''} · {Math.round(cand.confidence * 100)}% match
              </span>
              {#if cand.overview}<p class="dim overview">{cand.overview}</p>{/if}
              <button class="btn small primary" onclick={() => applyCandidate(item.id, cand)}>
                This is the one
              </button>
            </div>
          </div>
        {/each}
      </div>
      <form
        class="row"
        onsubmit={(e) => {
          searchFor = item;
          searchKind = item.kind === 'series' ? 'show' : 'movie';
          searchQ = item.title;
          manualSearch(e);
        }}
      >
        <span class="dim">Not listed?</span>
        <button class="btn small" type="submit">Search TMDB manually…</button>
      </form>
      {#if searchFor?.id === item.id && searchResults.length}
        <div class="candidates">
          {#each searchResults as cand (cand.provider_id)}
            <div class="candidate">
              {#if cand.poster_url}<img src={cand.poster_url} alt="" loading="lazy" />{/if}
              <div>
                <strong>{cand.title}</strong>
                <span class="dim">{cand.year ?? ''}</span>
                {#if cand.overview}<p class="dim overview">{cand.overview}</p>{/if}
                <button class="btn small primary" onclick={() => applyCandidate(item.id, cand)}>
                  This is the one
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/each}
  {#if searchFor}
    <form class="row search-manual" onsubmit={manualSearch}>
      <input bind:value={searchQ} aria-label="Search query" />
      <select bind:value={searchKind} aria-label="Kind">
        <option value="movie">Movie</option>
        <option value="show">Show</option>
      </select>
      <button class="btn" type="submit">Search for {searchFor.title}</button>
      <button class="btn" type="button" onclick={() => (searchFor = null)}>Close</button>
    </form>
  {/if}
{/if}

{#if tab === 'emby'}
  <h2>Emby import</h2>
  <p class="dim">
    Carry watched state, resume positions and favourites over from Emby.
    Matching is by TMDB/TVDB/IMDb id, never by title. Run a dry run first —
    it writes nothing and shows exactly what would land.
  </p>
  {#if !embyStatus}
    <p class="dim">Checking the Emby connection…</p>
  {:else if !embyStatus.configured}
    <p class="error">
      Emby is not configured on the server. Set PARLOUR_EMBY_URL and
      PARLOUR_EMBY_API_KEY in the server environment and restart.
    </p>
  {:else if !embyStatus.reachable}
    <p class="error">Emby is configured but unreachable: {embyStatus.error}</p>
  {:else}
    <div class="row">
      <label>Emby user
        <select bind:value={embyUserId}>
          {#each embyStatus.users as u (u.id)}<option value={u.id}>{u.name}</option>{/each}
        </select>
      </label>
      <span aria-hidden="true">→</span>
      <label>Parlour profile
        <select bind:value={embyProfileId}>
          {#each embyProfiles as p (p.id)}<option value={p.id}>{p.username} / {p.name}</option>{/each}
        </select>
      </label>
    </div>
    <div class="row">
      <button class="btn" disabled={embyBusy} onclick={() => runEmbyImport(true)}>Dry run (writes nothing)</button>
      <button class="btn primary" disabled={embyBusy} onclick={() => runEmbyImport(false)}>Import for real</button>
    </div>
    {#if embyReport}
      <h3>Last report{embyReport.dry_run ? ' (dry run)' : ''}</h3>
      <ul>
        <li>{embyReport.emby_items_with_state} Emby items had state worth carrying</li>
        <li>{embyReport.matched} matched{embyReport.dry_run ? '' : `, ${embyReport.applied} item states written`}</li>
        <li>{embyReport.unmatched} unmatched, {embyReport.ambiguous} ambiguous, {embyReport.skipped_no_state} skipped (no state)</li>
      </ul>
      {#if embyReport.unmatched_items.length}
        <h4>Unmatched — nothing was imported for these</h4>
        <ul>{#each embyReport.unmatched_items as name (name)}<li>{name}</li>{/each}</ul>
      {/if}
      {#if embyReport.ambiguous_items.length}
        <h4>Ambiguous — the id matched more than one title; skipped on purpose</h4>
        <ul>{#each embyReport.ambiguous_items as name (name)}<li>{name}</li>{/each}</ul>
      {/if}
    {/if}
  {/if}
{/if}

<style>
  .tabs {
    display: flex;
    gap: 0.6rem;
    flex-wrap: wrap;
    margin-bottom: 1.2rem;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 1.6rem;
  }
  th,
  td {
    text-align: left;
    padding: 0.5rem 0.6rem;
    border-bottom: 1px solid var(--line);
    vertical-align: top;
  }
  th {
    color: var(--text-dim);
    font-size: 0.85rem;
  }
  .paths {
    font-family: ui-monospace, monospace;
    font-size: 0.82rem;
    word-break: break-all;
  }
  .actions {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .form {
    max-width: 30rem;
    margin-bottom: 1.6rem;
  }
  .row {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    flex-wrap: wrap;
    margin: 0.8rem 0;
  }
  .row select,
  .row input {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.5rem 0.6rem;
  }
  .invite-link code {
    display: block;
    margin-top: 0.4rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.6rem;
    word-break: break-all;
  }
  .review-item {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 1rem 1.1rem;
    margin-bottom: 1rem;
    background: var(--bg-raise);
  }
  .candidates {
    display: grid;
    gap: 0.8rem;
  }
  .candidate {
    display: flex;
    gap: 0.9rem;
    align-items: flex-start;
  }
  .candidate img {
    width: 54px;
    border-radius: 0.4rem;
  }
  .overview {
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    margin: 0.3rem 0 0.5rem;
    max-width: 70ch;
  }
  .search-manual {
    position: sticky;
    bottom: 0.8rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.7rem;
  }
</style>
