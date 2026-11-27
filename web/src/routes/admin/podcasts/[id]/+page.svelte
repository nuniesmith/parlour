<script lang="ts">
  // Server → Podcasts → a show (POD4, D32): its check state, "refresh
  // now", its four per-show settings, and its episode list — downloaded
  // or not, with a per-episode "download". The Dashboard's failing-feed
  // notice links here. This is the management view: listening is POD6's.
  import { onDestroy } from 'svelte';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import {
    episodeLabel,
    formatBytes,
    formatDateTime,
    formatDurationMs,
    type PodcastEpisodeRow,
    type PodcastShow,
    type ShowSettings,
  } from '$lib/types';

  let show = $state<PodcastShow | null>(null);
  let missing = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let form = $state<ShowSettings | null>(null);
  let saving = $state(false);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let polls = 0;

  const id = $derived(page.params.id);

  function busy(s: PodcastShow): boolean {
    return s.refresh_queued || s.episodes.some((e) => e.download_queued);
  }

  async function load(resetForm = false) {
    const { status, data, error: err } = await api<PodcastShow>(`/api/v1/admin/podcasts/${id}`);
    if (status === 404) {
      missing = true;
      show = null;
      return;
    }
    if (err) error = err;
    if (!data) return;
    show = data;
    if (resetForm || !form) form = { ...data.settings };
    schedulePoll();
  }

  /** While a check or a download is waiting, look again every 2 s (for
   *  at most ten minutes), so the list shows the result without a reload. */
  function schedulePoll() {
    if (timer) clearTimeout(timer);
    timer = null;
    if (!show || !busy(show) || polls > 300) return;
    polls += 1;
    timer = setTimeout(() => void load(), 2000);
  }

  $effect(() => {
    void id;
    missing = false;
    show = null;
    form = null;
    polls = 0;
    void load(true);
  });

  onDestroy(() => {
    if (timer) clearTimeout(timer);
  });

  async function refreshNow() {
    if (!show) return;
    error = null;
    notice = null;
    const { status, error: err } = await api(`/api/v1/admin/podcasts/${show.id}/refresh`, {
      method: 'POST',
    });
    if (status === 202) {
      notice = 'A check of the feed is queued.';
      polls = 0;
      await load();
    } else error = err ?? 'Could not queue the check.';
  }

  async function download(ep: PodcastEpisodeRow) {
    error = null;
    notice = null;
    const { status, error: err } = await api(`/api/v1/admin/podcast-episodes/${ep.id}/download`, {
      method: 'POST',
    });
    if (status === 202) {
      notice = `“${ep.title}” is queued for download.`;
      polls = 0;
      await load();
    } else error = err ?? 'Could not queue the download.';
  }

  async function saveSettings(event: SubmitEvent) {
    event.preventDefault();
    if (!show || !form || saving) return;
    error = null;
    notice = null;
    saving = true;
    const { data, error: err } = await api<PodcastShow>(
      `/api/v1/admin/podcasts/${show.id}/settings`,
      {
        method: 'PUT',
        body: JSON.stringify({
          auto_download: form.auto_download,
          check_schedule: form.check_schedule,
          max_episodes_to_keep: Number(form.max_episodes_to_keep),
          max_new_episodes_per_check: Number(form.max_new_episodes_per_check),
        }),
      },
    );
    saving = false;
    if (data) {
      show = data;
      form = { ...data.settings };
      notice = 'Settings saved.';
    } else error = err ?? 'Could not save the settings.';
  }

  function episodeLine(ep: PodcastEpisodeRow): string {
    const where = episodeLabel(ep.season, ep.episode_number, null);
    const kind = ep.episode_type && ep.episode_type !== 'full' ? ep.episode_type : null;
    return [where, kind].filter(Boolean).join(' · ');
  }
</script>

{#if missing}
  <h2>Show not found</h2>
  <p class="dim">It may have been removed. <a class="link" href="/admin/podcasts">Back to Podcasts</a></p>
{:else if !show}
  <p class="dim">Loading…</p>
{:else}
  <p class="crumbs dim">
    <a class="link" href={`/admin/podcasts?library=${show.library_id}`}>← All shows in {show.library_name}</a>
  </p>
  <h2>{show.title}</h2>
  {#if show.author}<p class="dim">{show.author}</p>{/if}

  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}
    <p class="notice" role="status">{notice} <a class="link" href="/admin/jobs">Open Jobs</a></p>
  {/if}

  <section class="card" aria-label="Feed checks">
    <h3>Feed checks</h3>
    <dl>
      <dt>Last checked</dt>
      <dd>{show.last_checked_at ? formatDateTime(show.last_checked_at) : 'Not yet'}</dd>
      {#if show.consecutive_failures > 0}
        <dt>Failing</dt>
        <dd class="bad">
          {show.consecutive_failures} check{show.consecutive_failures === 1 ? '' : 's'} in a row
          {#if !show.settings.auto_download && show.consecutive_failures >= 24}
            — its scheduled check turned itself off; turn auto-download on again once the feed works
          {/if}
        </dd>
        {#if show.last_error}
          <dt>Last error</dt>
          <dd>{show.last_error}</dd>
        {/if}
      {/if}
      <dt>Scheduled checks</dt>
      <dd>
        {#if show.settings.auto_download}
          On, at <span class="mono">{show.settings.check_schedule}</span> (UTC)
        {:else}
          Off — the feed is checked only when you refresh it
        {/if}
      </dd>
    </dl>
    <div class="actions">
      <button class="btn small primary" onclick={refreshNow} disabled={show.refresh_queued}
        >{show.refresh_queued ? 'Check queued…' : 'Refresh now'}</button
      >
    </div>
  </section>

  {#if form}
    <form class="card form" onsubmit={saveSettings} aria-label="Show settings">
      <h3>Settings</h3>
      <div class="check-field">
        <label class="check"
          ><input type="checkbox" bind:checked={form.auto_download} />
          Download new episodes automatically</label
        >
        <p class="dim small">
          Off by default. While it is on, the feed is checked on the schedule below and new
          episodes are downloaded; while it is off, nothing is checked on its own.
        </p>
      </div>
      <div class="field">
        <label for="schedule">Check schedule (cron, five fields, UTC)</label>
        <input id="schedule" class="mono" bind:value={form.check_schedule} spellcheck="false" />
        <p class="dim small">
          Minute, hour, day of month, month, day of week. <span class="mono">0 * * * *</span> is
          every hour on the hour; <span class="mono">30 6 * * *</span> is every day at 06:30 UTC.
        </p>
      </div>
      <div class="field">
        <label for="new-per-check">New episodes to download per check (0 = no limit)</label>
        <input id="new-per-check" type="number" min="0" bind:value={form.max_new_episodes_per_check} />
      </div>
      <div class="field">
        <label for="keep">Downloaded episodes to keep (0 = keep all)</label>
        <input id="keep" type="number" min="0" bind:value={form.max_episodes_to_keep} />
        <p class="dim small">
          When there are more, the oldest download is deleted, one per check, whether or not
          anyone has heard it. The episode stays in the list and can be downloaded again.
        </p>
      </div>
      <div class="actions">
        <button class="btn small primary" type="submit" disabled={saving}>Save settings</button>
        <button
          class="btn small"
          type="button"
          onclick={() => show && (form = { ...show.default })}>Back to the defaults</button
        >
      </div>
    </form>
  {/if}

  <section class="card" aria-label="Show details">
    <h3>Details</h3>
    <dl>
      <dt>Feed address</dt>
      <dd class="mono">{show.feed_url}</dd>
      <dt>Folder</dt>
      <dd class:mono={!!show.folder_path}>
        {show.folder_path ?? 'Named in the podcast folder at the first download'}
      </dd>
      {#if show.podcast_type}<dt>Type</dt><dd>{show.podcast_type === 'serial' ? 'Serial (oldest first)' : 'Episodic (newest first)'}</dd>{/if}
      {#if show.language}<dt>Language</dt><dd>{show.language}</dd>{/if}
      {#if show.genres.length}<dt>Genres</dt><dd>{show.genres.join(', ')}</dd>{/if}
      {#if show.explicit}<dt>Explicit</dt><dd>Yes</dd>{/if}
      <dt>Added</dt>
      <dd>{formatDateTime(show.created_at)}</dd>
    </dl>
    <p class="dim small">The feed address can hold a private token; it is shown only to admins.</p>
  </section>

  <h3>Episodes</h3>
  {#if show.episodes.length === 0}
    <p class="dim">The feed lists no episodes yet.</p>
  {:else}
    <p class="dim">
      {show.episodes.filter((e) => e.downloaded_at).length} of {show.episodes.length} downloaded.
      Only downloaded episodes can be played.
    </p>
    <div class="table-wrap">
      <table>
        <thead>
          <tr><th>Episode</th><th>Published</th><th>Length</th><th>Downloaded</th></tr>
        </thead>
        <tbody>
          {#each show.episodes as ep (ep.id)}
            <tr>
              <td>
                {ep.title || 'Untitled episode'}
                {#if episodeLine(ep)}<div class="dim small">{episodeLine(ep)}</div>{/if}
              </td>
              <td>{ep.published_at ? formatDateTime(ep.published_at) : '—'}</td>
              <td>{formatDurationMs(ep.file_duration_ms ?? ep.duration_ms) || '—'}</td>
              <td>
                {#if ep.downloaded_at}
                  <span class="ok">Downloaded</span>
                  <div class="dim small">
                    {formatDateTime(ep.downloaded_at)}{#if ep.file_size_bytes} · {formatBytes(ep.file_size_bytes)}{/if}
                  </div>
                {:else if ep.download_queued}
                  <span class="dim">Queued…</span>
                {:else}
                  <button class="btn small" onclick={() => download(ep)}>Download</button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}

<style>
  .link {
    color: var(--accent);
  }
  .crumbs {
    margin: 0 0 0.3rem;
    font-size: 0.9rem;
  }
  h2 {
    margin-bottom: 0.2rem;
  }
  .card {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg-card);
    padding: 0.75rem 1rem;
    margin: 1rem 0;
    max-width: 46rem;
  }
  .card h3 {
    margin: 0 0 0.5rem;
    font-size: 1rem;
  }
  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 0.3rem 1rem;
    margin: 0 0 0.6rem;
  }
  dt {
    color: var(--text-dim);
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .small {
    font-size: 0.85rem;
  }
  .check-field {
    margin-bottom: 0.9rem;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .check input {
    width: auto;
    margin: 0;
  }
  .bad {
    color: var(--danger);
  }
  .ok {
    color: var(--ok);
  }
</style>
