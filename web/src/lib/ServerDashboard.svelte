<script lang="ts">
  // Server Dashboard (UI2): the /admin landing. Everything shown
  // comes from GET /api/v1/admin/server and the recent-jobs list —
  // addresses are displayed as configured, never guessed.
  import { onMount } from 'svelte';
  import ActivityFeed from '$lib/ActivityFeed.svelte';
  import AlertsRail from '$lib/AlertsRail.svelte';
  import { api } from '$lib/api';
  import NowPlaying from '$lib/NowPlaying.svelte';
  import type { Job, Notice, ServerInfo } from '$lib/types';

  const REFRESH_MS = 15_000;

  let info = $state<ServerInfo | null>(null);
  let jobs = $state<Job[]>([]);
  let notices = $state<Notice[] | null>(null);
  // The banner is for what should not wait: problems, and a
  // pending restart. Everything is also listed in the Alerts rail.
  const urgent = $derived(
    (notices ?? []).filter((n) => n.severity === 'error' || n.kind === 'restart_required'),
  );
  let error = $state<string | null>(null);
  let copied = $state<string | null>(null);
  let copyTimer: ReturnType<typeof setTimeout> | null = null;

  const urls = $derived(
    info
      ? [
          { key: 'lan', label: 'Home network (LAN)', env: 'PARLOUR_URL_LAN', url: info.urls.lan },
          {
            key: 'tailnet',
            label: 'Tailnet',
            env: 'PARLOUR_URL_TAILNET',
            url: info.urls.tailnet,
          },
          {
            key: 'public',
            label: 'Public',
            env: 'PARLOUR_URL_PUBLIC',
            url: info.urls.public,
          },
        ]
      : [],
  );

  async function load() {
    const [s, j, n] = await Promise.all([
      api<ServerInfo>('/api/v1/admin/server'),
      api<Job[]>('/api/v1/admin/jobs?limit=5'),
      api<Notice[]>('/api/v1/admin/notices'),
    ]);
    if (n.data) notices = n.data;
    if (s.data) {
      info = s.data;
      error = null;
    } else {
      error = s.error ?? 'Could not load server information.';
    }
    if (j.data) jobs = j.data;
  }

  async function dismiss(notice: Notice) {
    // Hide it at once; the next refresh confirms.
    notices = (notices ?? []).filter((n) => n.id !== notice.id);
    const { status, error: err } = await api(`/api/v1/admin/notices/${notice.id}/dismiss`, {
      method: 'POST',
    });
    if (status !== 204 && status !== 404) {
      error = err ?? 'Could not dismiss that alert.';
      await load();
    }
  }

  async function copy(key: string, url: string) {
    try {
      await navigator.clipboard.writeText(url);
      copied = key;
    } catch {
      // Clipboard needs a secure context; the link itself is still selectable.
      copied = `${key}:failed`;
    }
    if (copyTimer) clearTimeout(copyTimer);
    copyTimer = setTimeout(() => (copied = null), 2000);
  }

  onMount(() => {
    void load();
    const timer = setInterval(() => void load(), REFRESH_MS);
    return () => {
      clearInterval(timer);
      if (copyTimer) clearTimeout(copyTimer);
    };
  });
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if !info}
  {#if !error}<p class="dim">Loading…</p>{/if}
{:else}
  <div class="dash">
  <div class="head">
    <h2>{info.name}</h2>
    <button class="btn small" onclick={load}>Refresh</button>
  </div>

  {#if urgent.length}
    <div class="banner" role="status">
      {#each urgent as n (n.id)}
        <p>{n.summary}</p>
      {/each}
    </div>
  {/if}

  <section class="card" aria-label="Server">
    <dl>
      <div class="row">
        <dt>Version</dt>
        <dd>Parlour {info.version}</dd>
      </div>
      <div class="row">
        <dt>Listening on</dt>
        <dd>HTTP port {info.port}</dd>
      </div>
      <div class="row">
        <dt>Database</dt>
        <dd>
          {#if info.db === 'ok'}
            <span class="dot ok" aria-hidden="true"></span>Connected
          {:else}
            <span class="dot bad" aria-hidden="true"></span><span class="error">Not answering</span>
          {/if}
        </dd>
      </div>
      {#each urls as u (u.key)}
        <div class="row">
          <dt>{u.label}</dt>
          <dd>
            {#if u.url}
              <a class="url" href={u.url} target="_blank" rel="noreferrer">{u.url}</a>
              <button
                class="btn small"
                aria-label={`Copy ${u.label} address`}
                onclick={() => copy(u.key, u.url ?? '')}
              >
                {copied === u.key ? 'Copied' : copied === `${u.key}:failed` ? 'Copy failed' : 'Copy'}
              </button>
            {:else}
              <span class="dim">Not set — add <code>{u.env}</code> to the server environment.</span>
            {/if}
          </dd>
        </div>
      {/each}
    </dl>
    {#if !info.actions.restart && !info.actions.shutdown}
      <p class="dim foot">
        Restart and shut down are not available from here yet — use the host (for example
        <code>docker compose restart parlour</code>).
      </p>
    {/if}
  </section>

  <AlertsRail {notices} ondismiss={dismiss} />

  <NowPlaying />

  <ActivityFeed compact />

  <div class="head">
    <h2>Job queue</h2>
    <a class="btn small" href="/admin?tab=jobs">All jobs</a>
  </div>
  {#if info.jobs}
    <ul class="stats">
      <li><strong>{info.jobs.queued}</strong><span class="dim">Queued</span></li>
      <li><strong>{info.jobs.running}</strong><span class="dim">Running</span></li>
      <li>
        <strong class:error={info.jobs.failed_recent > 0}>{info.jobs.failed_recent}</strong>
        <span class="dim">Failed, last 24 h</span>
      </li>
      <li>
        <strong>{info.jobs.succeeded_recent}</strong><span class="dim">Finished, last 24 h</span>
      </li>
    </ul>
  {:else}
    <p class="error">The job queue can't be read while the database isn't answering.</p>
  {/if}
  {#if jobs.length === 0}
    <p class="dim">No jobs have run yet.</p>
  {:else}
    <ul class="jobs">
      {#each jobs as job (job.id)}
        <li>
          <span>{job.kind}</span>
          <span class="chip" class:error={job.status === 'failed'}>{job.status}</span>
          <span class="dim when">{new Date(job.created_at).toLocaleString()}</span>
        </li>
      {/each}
    </ul>
  {/if}
  </div>
{/if}

<style>
  /* One column, Alerts after the server card; from 1280 px the
     Alerts become a right rail beside everything else. */
  .dash {
    display: block;
  }
  .dash > :global(.rail) {
    max-width: 52rem;
    margin-top: 1.4rem;
  }
  @media (min-width: 1280px) {
    .dash {
      display: grid;
      grid-template-columns: minmax(0, 1fr) 19rem;
      column-gap: 1.6rem;
      align-items: start;
    }
    .dash > :global(*) {
      grid-column: 1;
    }
    .dash > :global(.rail) {
      grid-column: 2;
      grid-row: 1 / span 40;
      margin-top: 0;
    }
  }
  .banner {
    max-width: 52rem;
    margin-bottom: 0.9rem;
    padding: 0.7rem 1rem;
    border: 1px solid var(--danger);
    border-left-width: 0.3rem;
    border-radius: var(--radius);
    background: var(--bg-raise);
    display: grid;
    gap: 0.3rem;
  }
  .banner p {
    margin: 0;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    max-width: 52rem;
    margin: 1.4rem 0 0.7rem;
  }
  .head:first-of-type {
    margin-top: 0;
  }
  .head h2 {
    margin: 0;
  }
  .card {
    max-width: 52rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.4rem 1.1rem;
  }
  dl {
    margin: 0;
  }
  .row {
    display: grid;
    grid-template-columns: 11rem minmax(0, 1fr);
    gap: 0.8rem;
    align-items: center;
    padding: 0.65rem 0;
    border-bottom: 1px solid var(--line);
  }
  .row:last-child {
    border-bottom: 0;
  }
  dt {
    color: var(--text-dim);
    font-size: 0.9rem;
  }
  dd {
    margin: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.6rem;
    min-width: 0;
  }
  .url {
    color: var(--accent);
    overflow-wrap: anywhere;
  }
  code {
    font-size: 0.85em;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 0.3rem;
    padding: 0.05rem 0.3rem;
  }
  .dot {
    width: 0.6rem;
    height: 0.6rem;
    border-radius: 50%;
    flex: none;
  }
  .dot.ok {
    background: var(--ok);
  }
  .dot.bad {
    background: var(--danger);
  }
  .foot {
    font-size: 0.9rem;
    margin: 0;
    padding: 0.7rem 0;
    border-top: 1px solid var(--line);
  }
  .stats {
    list-style: none;
    margin: 0 0 0.9rem;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(9rem, 1fr));
    gap: 0.7rem;
    max-width: 52rem;
  }
  .stats li {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.7rem 0.9rem;
    display: grid;
    gap: 0.1rem;
  }
  .stats strong {
    font-size: 1.5rem;
    font-weight: 600;
  }
  .stats span {
    font-size: 0.85rem;
  }
  .jobs {
    list-style: none;
    margin: 0;
    padding: 0;
    max-width: 52rem;
  }
  .jobs li {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    padding: 0.5rem 0.1rem;
    border-bottom: 1px solid var(--line);
  }
  .jobs .when {
    margin-left: auto;
    font-size: 0.9rem;
    text-align: right;
  }
  .chip.error {
    border-color: var(--danger);
  }
  @media (max-width: 640px) {
    .row {
      grid-template-columns: 1fr;
      gap: 0.2rem;
    }
  }
</style>
