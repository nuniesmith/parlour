<script lang="ts">
  // Server → Jobs: what the server has been doing in the
  // background, with each job's errors, and "run now" for the
  // job kinds that can be started by hand (per-library scan,
  // metadata match, and the server export — XFER2 — whose
  // finished bundle is downloaded from its row here).
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import {
    formatBytes,
    formatDateTime,
    jobKindLabel,
    type ExportEstimate,
    type Job,
    type Library,
  } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let jobs = $state<Job[] | null>(null);
  let libraries = $state<Library[]>([]);
  let includeArtwork = $state(false);
  let estimate = $state<ExportEstimate | null>(null);
  let exporting = $state(false);

  const active = $derived((jobs ?? []).some((j) => j.status === 'queued' || j.status === 'running'));

  function libraryName(id: string | null): string {
    if (!id) return '';
    return libraries.find((l) => l.id === id)?.name ?? '';
  }

  async function loadJobs() {
    const { data, error: err } = await api<Job[]>('/api/v1/admin/jobs?limit=50');
    if (data) jobs = data;
    else error = err ?? 'Could not load jobs.';
  }

  async function runNow(path: string, done: string) {
    error = null;
    notice = null;
    const { status, error: err } = await api(path, { method: 'POST' });
    if (status === 202) {
      notice = done;
      await loadJobs();
    } else error = err ?? 'Could not start that job.';
  }

  // The size warning's numbers, fetched when artwork is first ticked.
  async function loadEstimate() {
    if (!includeArtwork || estimate) return;
    const { data } = await api<ExportEstimate>('/api/v1/admin/export/estimate');
    estimate = data;
  }

  async function startExport() {
    error = null;
    notice = null;
    exporting = true;
    const { status, error: err } = await api('/api/v1/admin/export', {
      method: 'POST',
      // Hashes stay home: the explicit confirm belongs to the
      // Import/Export page (XFER8), with its warning.
      body: JSON.stringify({ include_artwork: includeArtwork, include_hashes: false }),
    });
    exporting = false;
    if (status === 202) {
      notice = 'Server export started. Its download appears in the job’s row when it finishes.';
      await loadJobs();
    } else error = err ?? 'Could not start the export.';
  }

  function isBundle(job: Job): boolean {
    return job.kind === 'export_bundle' && job.status === 'succeeded' && job.result !== null;
  }

  async function deleteBundle(job: Job) {
    if (!confirm('Delete this bundle from the server? Download it first if you still need it.')) {
      return;
    }
    error = null;
    notice = null;
    const { status, error: err } = await api(`/api/v1/admin/export/${job.id}`, {
      method: 'DELETE',
    });
    if (status === 204) {
      notice = 'Bundle deleted from the server.';
      await loadJobs();
    } else error = err ?? 'Could not delete that bundle.';
  }

  onMount(() => {
    void loadJobs();
    void api<Library[]>('/api/v1/admin/libraries').then((r) => (libraries = r.data ?? []));
    // Follow running work closely; otherwise a slow refresh is enough.
    let ticks = 0;
    const timer = setInterval(() => {
      ticks += 1;
      if (active || ticks % 10 === 0) void loadJobs();
    }, 3000);
    return () => clearInterval(timer);
  });
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" role="status">{notice}</p>{/if}

<h2>Run now</h2>
{#if libraries.length === 0}
  <p class="dim">
    Scans and metadata matches run per library — <a class="link" href="/admin/library"
      >add a library</a
    > first.
  </p>
{:else}
  <div class="table-wrap">
    <table>
      <thead><tr><th>Library</th><th>Start a job</th></tr></thead>
      <tbody>
        {#each libraries as lib (lib.id)}
          <tr>
            <td>{lib.name}</td>
            <td><div class="actions">
              <button
                class="btn small"
                onclick={() =>
                  runNow(`/api/v1/admin/libraries/${lib.id}/scan`, `Scan of ${lib.name} started.`)}
                >Scan</button
              >
              <button
                class="btn small"
                onclick={() =>
                  runNow(
                    `/api/v1/admin/libraries/${lib.id}/metadata`,
                    `Metadata match for ${lib.name} started.`,
                  )}>Match metadata</button
              >
            </div></td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <p class="dim note">
    Scans also run on their own: when files change, and once a night. The other job kinds,
    apart from the export below, are started by the server and cannot be run from here.
  </p>
{/if}

<h3>Export the server</h3>
<div class="export">
  <p class="dim note">
    Writes one bundle file with the users, profiles and their preferences, libraries and who may
    see them, the catalog with its segments, watch history, recent searches and the Activity
    feed. Server secrets never go in. Password and PIN hashes are left out of exports started
    here.
  </p>
  <label>
    <input type="checkbox" bind:checked={includeArtwork} onchange={loadEstimate} />
    Include artwork files
  </label>
  {#if includeArtwork}
    <p class="warn" role="status">
      {#if estimate}
        This adds {estimate.artwork_files}
        {estimate.artwork_files === 1 ? 'image' : 'images'}, about {formatBytes(
          estimate.artwork_bytes,
        )}, to the bundle — it can be many times larger than without them.
        {#if estimate.artwork_unavailable > 0}
          {estimate.artwork_unavailable} cached
          {estimate.artwork_unavailable === 1 ? 'image is' : 'images are'} missing on disk and
          will travel as links only.
        {/if}
      {:else}
        Artwork can make the bundle many times larger.
      {/if}
    </p>
  {:else}
    <p class="dim note">Without artwork the bundle keeps each image’s source link only.</p>
  {/if}
  <div>
    <button class="btn small" disabled={exporting} onclick={startExport}>Export now</button>
  </div>
</div>

<div class="head">
  <h2>Recent jobs</h2>
  <button class="btn small" onclick={loadJobs}>Refresh</button>
</div>
{#if jobs === null}
  {#if !error}<p class="dim">Loading…</p>{/if}
{:else if jobs.length === 0}
  <p class="dim">No jobs have run yet.</p>
{:else}
  <ul class="jobs">
    {#each jobs as job (job.id)}
      <li>
        <div class="line">
          <span class="what">
            {jobKindLabel(job.kind)}
            {#if libraryName(job.library_id)}<span class="dim"> · {libraryName(job.library_id)}</span
              >{/if}
          </span>
          <span class="chip" class:bad={job.status === 'failed'}>{job.status}</span>
          {#if job.progress_total > 0}
            <span class="dim">{job.progress_done}/{job.progress_total}</span>
          {/if}
          <time class="dim when" datetime={job.created_at}>{formatDateTime(job.created_at)}</time>
        </div>
        {#if isBundle(job)}
          <div class="bundle">
            {#if job.result?.deleted}
              <span class="dim">Bundle deleted from the server</span>
            {:else}
              <a
                class="btn small"
                href={`/api/v1/admin/export/${job.id}/download`}
                download={String(job.result?.file_name ?? 'parlour-export.zip')}
                >Download ({formatBytes(Number(job.result?.bytes ?? 0))})</a
              >
              <button class="btn small" onclick={() => deleteBundle(job)}>Delete from server</button>
            {/if}
            {#if job.result?.hashes_included}
              <span class="chip bad">holds password and PIN hashes</span>
            {/if}
          </div>
        {/if}
        {#if job.errors.length || job.result}
          <details>
            <summary>
              {#if job.errors.length}
                <span class="error"
                  >{job.errors.length} {job.errors.length === 1 ? 'error' : 'errors'}</span
                >
              {:else}
                Result
              {/if}
            </summary>
            {#if job.errors.length}
              <ul class="errors">
                {#each job.errors as e, i (i)}
                  <li>
                    {#if e.path}<span class="mono">{e.path}</span><br />{/if}
                    {e.message}
                  </li>
                {/each}
              </ul>
            {/if}
            {#if job.result}
              <dl class="result">
                {#each Object.entries(job.result) as [key, value] (key)}
                  <div>
                    <dt>{key.replaceAll('_', ' ')}</dt>
                    <dd>{typeof value === 'object' ? JSON.stringify(value) : String(value)}</dd>
                  </div>
                {/each}
              </dl>
            {/if}
          </details>
        {/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  .link {
    color: var(--accent);
  }
  .note {
    max-width: 44rem;
    font-size: 0.9rem;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    max-width: 60rem;
    margin: 1.4rem 0 0.4rem;
  }
  .head h2 {
    margin: 0;
  }
  .jobs {
    list-style: none;
    margin: 0;
    padding: 0;
    max-width: 60rem;
  }
  .jobs > li {
    padding: 0.55rem 0.1rem;
    border-bottom: 1px solid var(--line);
  }
  .line {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.7rem;
  }
  .when {
    margin-left: auto;
    font-size: 0.9rem;
  }
  h3 {
    margin: 1.4rem 0 0.4rem;
    font-size: 1rem;
  }
  .export {
    display: grid;
    gap: 0.5rem;
    max-width: 44rem;
  }
  .export p {
    margin: 0;
  }
  .warn {
    color: var(--accent);
    font-size: 0.9rem;
  }
  .bundle {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.6rem;
    margin-top: 0.4rem;
  }
  .chip.bad {
    border-color: var(--danger);
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
  .errors {
    margin: 0.5rem 0;
    padding-left: 1.2rem;
    display: grid;
    gap: 0.4rem;
    overflow-wrap: anywhere;
  }
  .result {
    margin: 0.5rem 0 0;
    display: grid;
    gap: 0.2rem;
    font-size: 0.9rem;
  }
  .result div {
    display: grid;
    grid-template-columns: 12rem minmax(0, 1fr);
    gap: 0.6rem;
  }
  .result dt {
    color: var(--text-dim);
  }
  .result dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
</style>
