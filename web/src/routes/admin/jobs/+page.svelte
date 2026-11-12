<script lang="ts">
  // Server → Jobs: what the server has been doing in the
  // background, with each job's errors, and "run now" for the
  // job kinds that can be started by hand (per-library scan
  // and metadata match).
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { Job, Library } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let jobs = $state<Job[] | null>(null);
  let libraries = $state<Library[]>([]);

  const KIND: Record<string, string> = {
    scan_library: 'Library scan',
    fetch_metadata: 'Metadata match',
    download_artwork: 'Artwork download',
    import_emby: 'Emby import',
    purge_activity: 'Activity clean-up',
  };

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
    Scans also run on their own: when files change, and once a night. The other job kinds are
    started by the server and cannot be run from here.
  </p>
{/if}

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
            {KIND[job.kind] ?? job.kind}
            {#if libraryName(job.library_id)}<span class="dim"> · {libraryName(job.library_id)}</span
              >{/if}
          </span>
          <span class="chip" class:bad={job.status === 'failed'}>{job.status}</span>
          {#if job.progress_total > 0}
            <span class="dim">{job.progress_done}/{job.progress_total}</span>
          {/if}
          <span class="dim when">{new Date(job.created_at).toLocaleString()}</span>
        </div>
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
