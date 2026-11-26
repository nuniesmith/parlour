<script lang="ts">
  // Server → Import / Export (XFER8, D14/D25/D37/D38/D39). One admin
  // page over machinery that already exists:
  // - Export: XFER2's export job — artwork and, behind its warning and
  //   an explicit confirm, password and PIN hashes; Download / Delete.
  // - Import: XFER3's upload → dry run → apply job. The dry run's
  //   report is rendered before anything is applied, with D38's
  //   take-over and D37's no-password warning on top.
  // - Nightly backups: XFER7's job, set up here (D39) — folder, how
  //   many to keep, and hashes only if the admin turns them on.
  // - Sidecar files: XFER5's per-library opt-in and folder check, with
  //   the counts XFER4–XFER6 already keep. The switch stays on
  //   Server → Library.
  // Finished jobs still land on Server → Jobs and in the Activity feed.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import ImportReport from '$lib/ImportReport.svelte';
  import {
    asImportReport,
    formatBytes,
    formatDateTime,
    isNightlyBackup,
    type BackupSettingsView,
    type ExportEstimate,
    type ImportReport as Report,
    type Job,
    type LibrarySidecars,
    type PendingUpload,
  } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  function clear() {
    error = null;
    notice = null;
  }

  // D14 / §6: what the hashes warning has to make plain.
  const HASH_WARNING =
    'Anyone who gets hold of this file can try passwords against the hashes on their own ' +
    'computer, as long as they like. A 4-digit PIN can be worked out in minutes. Treat the file ' +
    'as you would the passwords themselves: keep it private, and delete it once it is safe.';

  // ------------------------------------------------------------ export
  let includeArtwork = $state(false);
  let includeHashes = $state(false);
  let estimate = $state<ExportEstimate | null>(null);
  let exporting = $state(false);
  let exportJobs = $state<Job[] | null>(null);
  const manualExports = $derived((exportJobs ?? []).filter((j) => !isNightlyBackup(j)));
  const nightlyJobs = $derived((exportJobs ?? []).filter((j) => isNightlyBackup(j)).slice(0, 7));

  async function loadExports() {
    const { data, error: err } = await api<Job[]>(
      '/api/v1/admin/jobs?kind=export_bundle&limit=40',
    );
    if (data) exportJobs = data;
    else error = err ?? 'Could not load exports.';
  }

  async function loadEstimate() {
    if (!includeArtwork || estimate) return;
    const { data } = await api<ExportEstimate>('/api/v1/admin/export/estimate');
    estimate = data;
  }

  async function startExport() {
    clear();
    if (
      includeHashes &&
      !confirm(
        `Put every password and PIN hash in this bundle?\n\n${HASH_WARNING}\n\n` +
          'Without them, imported accounts arrive with no password until an admin sets one.',
      )
    ) {
      return;
    }
    exporting = true;
    const { status, error: err } = await api('/api/v1/admin/export', {
      method: 'POST',
      body: JSON.stringify({ include_artwork: includeArtwork, include_hashes: includeHashes }),
    });
    exporting = false;
    if (status === 202) {
      notice = 'Export started. Its download appears below when it finishes.';
      includeHashes = false;
      await loadExports();
    } else error = err ?? 'Could not start the export.';
  }

  async function deleteBundle(job: Job) {
    if (!confirm('Delete this bundle from the server? Download it first if you still need it.'))
      return;
    clear();
    const { status, error: err } = await api(`/api/v1/admin/export/${job.id}`, {
      method: 'DELETE',
    });
    if (status === 204) {
      notice = 'Bundle deleted from the server.';
      await loadExports();
    } else error = err ?? 'Could not delete that bundle.';
  }

  // ------------------------------------------------------------ import
  let pending = $state<PendingUpload[]>([]);
  let file = $state<File | null>(null);
  let uploading = $state(false);
  let checking = $state(false);
  let current = $state<{ id: string; bytes: number; report: Report } | null>(null);
  let folders = $state<Record<string, string>>({});
  let applying = $state<Job | null>(null);
  let importJobs = $state<Job[]>([]);

  async function loadImports() {
    const [p, j] = await Promise.all([
      api<PendingUpload[]>('/api/v1/admin/import'),
      api<Job[]>('/api/v1/admin/jobs?kind=import_bundle&limit=10'),
    ]);
    pending = p.data ?? [];
    importJobs = j.data ?? [];
  }

  function chooseFile(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    file = input.files?.[0] ?? null;
  }

  async function upload() {
    if (!file) return;
    clear();
    uploading = true;
    const { status, data, error: err } = await api<{ id: string; bytes: number; report: Report }>(
      '/api/v1/admin/import',
      { method: 'POST', headers: { 'content-type': 'application/zip' }, body: file },
    );
    uploading = false;
    if (status === 201 && data) {
      current = data;
      folders = {};
      applying = null;
      await loadImports();
    } else error = err ?? 'Could not read that file as a Parlour bundle.';
  }

  function options() {
    const library_paths: Record<string, string[]> = {};
    for (const [name, text] of Object.entries(folders)) {
      const paths = (text ?? '')
        .split('\n')
        .map((p) => p.trim())
        .filter(Boolean);
      if (paths.length) library_paths[name] = paths;
    }
    return { library_paths };
  }

  /** The dry run for an upload, as the server is now. */
  async function check(id: string, bytes: number, keepFolders: boolean) {
    clear();
    checking = true;
    const { data, error: err } = await api<Report>(`/api/v1/admin/import/${id}/dry-run`, {
      method: 'POST',
      body: JSON.stringify(keepFolders ? options() : {}),
    });
    checking = false;
    if (data) {
      current = { id, bytes, report: data };
      if (!keepFolders) folders = {};
      applying = null;
    } else error = err ?? 'Could not check that upload.';
  }

  async function apply() {
    if (!current) return;
    const r = current.report;
    const locked = r.users.filter((u) => u.sign_in === 'locked').length;
    const lines = [
      'Apply this import?',
      '',
      'It adds what the report lists and never deletes anything here.',
    ];
    if (locked) lines.push(`${locked} account(s) will have no password until you set one.`);
    if (r.conflicts_total) lines.push(`${r.conflicts_total} conflict(s) will be skipped.`);
    if (!confirm(lines.join('\n'))) return;
    clear();
    const { status, data, error: err } = await api<Job>(
      `/api/v1/admin/import/${current.id}/apply`,
      { method: 'POST', body: JSON.stringify(options()) },
    );
    if (status === 202 && data) {
      applying = data;
      notice = 'Import started.';
    } else error = err ?? 'Could not start the import.';
  }

  async function followApply() {
    if (!applying) return;
    const { data } = await api<Job>(`/api/v1/admin/jobs/${applying.id}`);
    if (!data) return;
    applying = data;
    if (data.status === 'succeeded') {
      current = null;
      folders = {};
      notice = 'Import applied. Scan the imported libraries so their items find their files.';
      await Promise.all([loadImports(), loadSidecars()]);
    } else if (data.status === 'failed') {
      error = 'The import failed and changed nothing; the upload is kept so you can try again.';
      await loadImports();
    }
  }

  async function discard(id: string) {
    if (!confirm('Delete this upload from the server?')) return;
    clear();
    const { status, error: err } = await api(`/api/v1/admin/import/${id}`, { method: 'DELETE' });
    if (status === 204) {
      if (current?.id === id) current = null;
      notice = 'Upload deleted.';
      await loadImports();
    } else error = err ?? 'Could not delete that upload.';
  }

  // ------------------------------------------------------------ backups
  let backup = $state<BackupSettingsView | null>(null);
  let form = $state({ folder: '', keep: 7, include_hashes: false });
  let saving = $state(false);

  async function loadBackup() {
    const { data, error: err } = await api<BackupSettingsView>('/api/v1/admin/backup');
    if (data) {
      backup = data;
      form = { folder: data.folder ?? '', keep: data.keep, include_hashes: data.include_hashes };
    } else error = err ?? 'Could not load the backup setting.';
  }

  async function saveBackup(event: SubmitEvent) {
    event.preventDefault();
    clear();
    if (
      form.include_hashes &&
      !backup?.include_hashes &&
      !confirm(
        `Put every password and PIN hash in each nightly backup?\n\n${HASH_WARNING}\n\n` +
          'The backups sit in that folder every night, unattended.',
      )
    ) {
      return;
    }
    saving = true;
    const folder = form.folder.trim();
    const { data, error: err } = await api<BackupSettingsView>('/api/v1/admin/backup', {
      method: 'PUT',
      body: JSON.stringify({
        folder: folder || null,
        keep: Number(form.keep),
        include_hashes: folder ? form.include_hashes : false,
      }),
    });
    saving = false;
    if (data) {
      backup = data;
      form = { folder: data.folder ?? '', keep: data.keep, include_hashes: data.include_hashes };
      notice = data.folder ? 'Nightly backups saved.' : 'Nightly backups are off.';
    } else error = err ?? 'Could not save the backup setting.';
  }

  async function resetBackup() {
    if (!confirm("Forget this setting and use the server's environment default again?")) return;
    clear();
    const { data, error: err } = await api<BackupSettingsView>('/api/v1/admin/backup', {
      method: 'DELETE',
    });
    if (data) {
      backup = data;
      form = { folder: data.folder ?? '', keep: data.keep, include_hashes: data.include_hashes };
      notice = 'Back to the environment default.';
    } else error = err ?? 'Could not reset the backup setting.';
  }

  // ------------------------------------------------------------ sidecars
  let sidecars = $state<LibrarySidecars[] | null>(null);

  async function loadSidecars() {
    const { data, error: err } = await api<LibrarySidecars[]>('/api/v1/admin/sidecars');
    if (data) sidecars = data;
    else error = err ?? 'Could not load the sidecar status.';
  }

  function writingState(lib: LibrarySidecars): { text: string; cls: string } {
    if (!lib.supported) return { text: 'Not for this kind of library yet', cls: 'dim' };
    if (!lib.write_sidecars) return { text: 'Off — Parlour only reads', cls: 'dim' };
    if (lib.folders.some((f) => !f.writable))
      return { text: 'On, but a folder can’t be written', cls: 'bad' };
    return { text: 'On', cls: 'ok' };
  }

  // ------------------------------------------------------------ polling
  const busy = $derived(
    (exportJobs ?? []).some((j) => j.status === 'queued' || j.status === 'running') ||
      (applying !== null && (applying.status === 'queued' || applying.status === 'running')),
  );

  onMount(() => {
    void loadExports();
    void loadImports();
    void loadBackup();
    void loadSidecars();
    let ticks = 0;
    const timer = setInterval(() => {
      ticks += 1;
      if (applying && (applying.status === 'queued' || applying.status === 'running'))
        void followApply();
      if (busy || ticks % 10 === 0) void loadExports();
    }, 2000);
    return () => clearInterval(timer);
  });
</script>

<h2>Import / Export</h2>
<p class="dim intro">
  Move this whole server to another one — accounts, profiles, libraries, the catalog, watch history
  — keep nightly copies of it, and see which libraries keep their metadata in files beside the
  media. Finished exports and imports also appear on <a class="link" href="/admin/jobs">Jobs</a> and
  in the <a class="link" href="/admin/activity">Activity</a> feed.
</p>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" role="status">{notice}</p>{/if}

<!-- ============================================================ export -->
<section aria-labelledby="export-h">
  <h3 id="export-h">Export</h3>
  <p class="dim note">
    One bundle file with the accounts, profiles and their preferences and recent searches, the
    libraries and who may see them, the catalog with its segments, watch history and the Activity
    feed. Server secrets (API keys, the stream secret, the database password) never go in.
  </p>
  <label class="opt">
    <input type="checkbox" bind:checked={includeArtwork} onchange={loadEstimate} />
    Include artwork files
  </label>
  {#if includeArtwork}
    <p class="warn" role="status">
      {#if estimate}
        Adds {estimate.artwork_files}
        {estimate.artwork_files === 1 ? 'image' : 'images'}, about {formatBytes(
          estimate.artwork_bytes,
        )} — the bundle can be many times larger.
        {#if estimate.artwork_unavailable > 0}
          {estimate.artwork_unavailable} cached
          {estimate.artwork_unavailable === 1 ? 'image is' : 'images are'} missing on disk and
          will travel as links only.
        {/if}
      {:else}
        Artwork can make the bundle many times larger.
      {/if}
    </p>
  {/if}
  <label class="opt">
    <input type="checkbox" bind:checked={includeHashes} />
    Include password and PIN hashes
  </label>
  {#if includeHashes}
    <p class="warn danger" role="alert">
      {HASH_WARNING} You will be asked to confirm.
    </p>
  {:else}
    <p class="dim note">
      Without hashes, accounts imported from the bundle have no password until an admin sets one.
    </p>
  {/if}
  <div>
    <button class="btn small" class:danger={includeHashes} disabled={exporting} onclick={startExport}
      >{includeHashes ? 'Export with hashes…' : 'Export now'}</button
    >
  </div>

  <h4>Bundles on this server</h4>
  {#if exportJobs === null}
    <p class="dim">Loading…</p>
  {:else if manualExports.length === 0}
    <p class="dim">No exports yet.</p>
  {:else}
    <ul class="list">
      {#each manualExports as job (job.id)}
        <li>
          <time class="dim" datetime={job.created_at}>{formatDateTime(job.created_at)}</time>
          {#if job.status === 'succeeded' && job.result}
            {#if job.result.deleted}
              <span class="dim">Deleted from the server</span>
            {:else}
              <a
                class="btn small"
                href={`/api/v1/admin/export/${job.id}/download`}
                download={String(job.result.file_name ?? 'parlour-export.zip')}
                >Download ({formatBytes(Number(job.result.bytes ?? 0))})</a
              >
              <button class="btn small" onclick={() => deleteBundle(job)}>Delete from server</button>
            {/if}
            {#if job.result.hashes_included}
              <span class="chip bad">holds password and PIN hashes</span>
            {/if}
            {#if job.result.artwork_included}<span class="chip">artwork</span>{/if}
          {:else if job.status === 'failed'}
            <span class="chip bad">failed</span>
            <span class="dim">{job.errors.at(-1)?.message ?? ''}</span>
          {:else}
            <span class="chip">{job.status}</span>
            {#if job.progress_total > 0}
              <span class="dim">{job.progress_done}/{job.progress_total}</span>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<!-- ============================================================ import -->
<section aria-labelledby="import-h">
  <h3 id="import-h">Import</h3>
  <p class="dim note">
    Upload a bundle from a Parlour server. Nothing changes until you have read what it would do and
    pressed Apply. An import never deletes anything here, and running it twice changes nothing the
    second time.
  </p>
  <div class="row">
    <input type="file" accept=".zip,application/zip" onchange={chooseFile} aria-label="Bundle file" />
    <button class="btn small" disabled={!file || uploading} onclick={upload}
      >{uploading ? 'Uploading…' : 'Upload and check'}</button
    >
  </div>

  {#if pending.length}
    <h4>Uploads waiting here</h4>
    <ul class="list">
      {#each pending as p (p.id)}
        <li>
          {#if p.uploaded_at}<time class="dim" datetime={p.uploaded_at}
              >{formatDateTime(p.uploaded_at)}</time
            >{/if}
          <span class="dim">{formatBytes(p.bytes)}</span>
          {#if current?.id === p.id}
            <span class="chip">shown below</span>
          {:else}
            <button class="btn small" disabled={checking} onclick={() => check(p.id, p.bytes, false)}
              >Check</button
            >
          {/if}
          <button class="btn small" onclick={() => discard(p.id)}>Delete</button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if current}
    <div class="panel">
      <h4>What this import would do</h4>
      <ImportReport report={current.report} bind:folders />
      <div class="actions">
        <button
          class="btn small"
          disabled={checking}
          onclick={() => current && check(current.id, current.bytes, true)}
          >{checking ? 'Checking…' : 'Check again with these folders'}</button
        >
        <button
          class="btn small primary"
          disabled={checking || (applying !== null && applying.status !== 'failed')}
          onclick={apply}>Apply import…</button
        >
        <button class="btn small" onclick={() => current && discard(current.id)}>Discard</button>
      </div>
      {#if applying && (applying.status === 'queued' || applying.status === 'running')}
        <p class="dim" role="status">
          Applying…
          {#if applying.progress_total > 0}{applying.progress_done}/{applying.progress_total}{/if}
        </p>
      {/if}
    </div>
  {/if}

  {#if importJobs.length}
    <h4>Recent imports</h4>
    <ul class="list">
      {#each importJobs as job (job.id)}
        <li class="block">
          <div class="line">
            <time class="dim" datetime={job.created_at}>{formatDateTime(job.created_at)}</time>
            <span class="chip" class:bad={job.status === 'failed'}>{job.status}</span>
            {#if job.status === 'failed'}
              <span class="dim">{job.errors.at(-1)?.message ?? ''}</span>
            {/if}
          </div>
          {#if asImportReport(job.result)}
            <details open={applying?.id === job.id}>
              <summary>What it did</summary>
              <ImportReport report={asImportReport(job.result)!} />
            </details>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<!-- ============================================================ backups -->
<section aria-labelledby="backup-h">
  <h3 id="backup-h">Nightly backups</h3>
  <p class="dim note">
    Each night (after 07:30 UTC) the server writes an export bundle into a folder and keeps the
    newest few. Restore one with Import above. Leave the folder empty for no backups. The folder
    must exist and be writable as the server sees it — in Docker, a folder mounted into the
    container (for example <code>/backups</code>); Parlour never creates it.
  </p>
  {#if backup}
    <p class="dim note">
      {#if backup.source === 'saved'}
        Set here{backup.updated_by ? ` by ${backup.updated_by}` : ''}{backup.updated_at
          ? `, ${formatDateTime(backup.updated_at)}`
          : ''}.
      {:else}
        Nothing saved here yet, so the server’s environment default applies
        ({backup.environment.folder
          ? `PARLOUR_BACKUP_DIR = ${backup.environment.folder}`
          : 'PARLOUR_BACKUP_DIR is not set: off'}).
      {/if}
    </p>
    <form class="form" onsubmit={saveBackup}>
      <div class="field">
        <label for="backup-folder">Backup folder</label>
        <input
          id="backup-folder"
          bind:value={form.folder}
          placeholder="Empty = no nightly backups"
          autocomplete="off"
        />
      </div>
      <div class="field">
        <label for="backup-keep">Backups to keep</label>
        <input id="backup-keep" type="number" min="1" max="365" bind:value={form.keep} />
      </div>
      <label class="opt">
        <input
          type="checkbox"
          bind:checked={form.include_hashes}
          disabled={!form.folder.trim()}
        />
        Include password and PIN hashes
      </label>
      {#if form.include_hashes && form.folder.trim()}
        <p class="warn danger" role="alert">{HASH_WARNING}</p>
      {:else}
        <p class="dim note">
          Without hashes, a restored server’s accounts have no password until an admin sets one.
        </p>
      {/if}
      <div class="actions">
        <button class="btn small primary" disabled={saving}>Save</button>
        {#if backup.source === 'saved'}
          <button type="button" class="btn small" onclick={resetBackup}
            >Use the environment default</button
          >
        {/if}
      </div>
    </form>
    {#if backup.folder}
      {#if backup.folder_problem}
        <p class="error" role="alert">Tonight’s backup would fail: {backup.folder_problem}.</p>
      {:else}
        <p class="dim note">
          On: <span class="mono">{backup.folder}</span>, keeping {backup.keep}{backup.include_hashes
            ? ', with hashes'
            : ''}.
        </p>
      {/if}
    {:else}
      <p class="dim note">Off — no nightly backups.</p>
    {/if}
  {:else}
    <p class="dim">Loading…</p>
  {/if}

  {#if nightlyJobs.length}
    <h4>Recent nights</h4>
    <ul class="list">
      {#each nightlyJobs as job (job.id)}
        <li>
          <time class="dim" datetime={job.created_at}>{formatDateTime(job.created_at)}</time>
          {#if job.status === 'succeeded' && job.result}
            {#if job.result.deleted}
              <span class="dim">Removed by retention</span>
            {:else}
              <a
                class="btn small"
                href={`/api/v1/admin/export/${job.id}/download`}
                download={String(job.result.file_name ?? 'parlour-backup.zip')}
                >Download ({formatBytes(Number(job.result.bytes ?? 0))})</a
              >
            {/if}
            {#if job.result.hashes_included}
              <span class="chip bad">holds password and PIN hashes</span>
            {/if}
          {:else if job.status === 'failed'}
            <span class="chip bad">failed</span>
            <span class="dim">{job.errors.at(-1)?.message ?? ''}</span>
          {:else}
            <span class="chip">{job.status}</span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<!-- ============================================================ sidecars -->
<section aria-labelledby="sidecar-h">
  <h3 id="sidecar-h">Metadata files beside the media</h3>
  <p class="dim note">
    Parlour always reads <code>.parlour.json</code> and <code>.nfo</code> files it finds next to your
    videos. Where a library allows it, it also writes them — with <code>poster.jpg</code> and
    <code>backdrop.jpg</code> — so a folder copied to a new drive takes its metadata along. Turn
    writing on or off per library on <a class="link" href="/admin/library">Library</a>.
  </p>
  {#if sidecars === null}
    <p class="dim">Loading…</p>
  {:else if sidecars.length === 0}
    <p class="dim">No libraries yet.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Library</th>
            <th>Writing</th>
            <th>Folders</th>
            <th>Sidecars</th>
            <th>NFO read / written</th>
            <th>Artwork written</th>
          </tr>
        </thead>
        <tbody>
          {#each sidecars as lib (lib.id)}
            {@const writing = writingState(lib)}
            <tr>
              <td>{lib.name}<span class="dim"> · {lib.kind}</span></td>
              <td class={writing.cls}>{writing.text}</td>
              <td>
                {#each lib.folders as f (f.path)}
                  <div class="mono">
                    {f.path}
                    <span class:ok={f.writable} class:dim={!f.writable}>
                      — {f.writable ? 'writable' : `can’t be written${f.problem ? ` (${f.problem})` : ''}`}
                    </span>
                  </div>
                {:else}
                  <span class="dim">No folders</span>
                {/each}
              </td>
              <td>
                {lib.sidecars} of {lib.items}
                {#if lib.sidecars_overridden}
                  <div class="dim small">{lib.sidecars_overridden} overridden by a fix-match</div>
                {/if}
              </td>
              <td>{lib.nfos_read} / {lib.nfos_written}</td>
              <td>{lib.artwork_written}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="dim small">
      Sidecars: items with a <code>.parlour.json</code> on record (read by a scan or written by a
      metadata match), of the library’s available movies, series and episodes.
    </p>
  {/if}
</section>

<style>
  .intro,
  .note {
    max-width: 44rem;
  }
  .note,
  .small {
    font-size: 0.9rem;
  }
  section {
    margin: 1.6rem 0 2rem;
    display: grid;
    gap: 0.5rem;
    max-width: 62rem;
  }
  section > p {
    margin: 0;
  }
  h3 {
    margin: 0 0 0.2rem;
    font-size: 1.1rem;
  }
  h4 {
    margin: 0.9rem 0 0.2rem;
    font-size: 0.95rem;
  }
  .opt {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .warn {
    color: var(--accent);
    font-size: 0.9rem;
    max-width: 44rem;
    margin: 0;
  }
  .warn.danger {
    color: var(--danger);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .list > li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.6rem;
    padding: 0.45rem 0.1rem;
    border-bottom: 1px solid var(--line);
  }
  .list > li.block {
    display: block;
  }
  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.6rem;
  }
  .chip.bad {
    border-color: var(--danger);
    color: var(--danger);
  }
  .panel {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg-card);
    padding: 0.8rem 1rem;
    margin-top: 0.6rem;
  }
  .panel h4 {
    margin-top: 0;
  }
  .ok {
    color: var(--ok);
  }
  .bad {
    color: var(--danger);
  }
  details {
    margin-top: 0.4rem;
  }
  summary {
    cursor: pointer;
    color: var(--text-dim);
    font-size: 0.9rem;
    width: fit-content;
  }
  .link {
    color: var(--accent);
  }
  code {
    font-size: 0.85em;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.3rem;
    padding: 0.05rem 0.3rem;
  }
</style>
