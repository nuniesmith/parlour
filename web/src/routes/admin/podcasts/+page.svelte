<script lang="ts">
  // Server → Podcasts (POD4, D31/D32): an admin manages a podcasts
  // library's shows, as Audiobookshelf lets one — add a show by its feed
  // address, import or export OPML, refresh a show now. Only admins add
  // shows (D31); everyone with access to the library sees every show
  // (the listener pages are POD6's). A feed's address may hold a private
  // token, so errors never repeat it. POD5 adds the library's finish
  // rule for episodes (AB11's, for a podcasts library).
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import {
    formatDateTime,
    OPML_OUTCOME_LABELS,
    type Library,
    type OpmlImportReport,
    type PodcastSettings,
    type PodcastShow,
    type PodcastSummary,
  } from '$lib/types';

  let libraries = $state<Library[]>([]);
  let loaded = $state(false);
  let shows = $state<PodcastSummary[] | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let feedUrl = $state('');
  let adding = $state(false);
  let importing = $state(false);
  let report = $state<OpmlImportReport | null>(null);
  let opmlInput = $state<HTMLInputElement | null>(null);
  /** POD5: the library's finish rule; a blank percentage is "off". */
  let finish = $state<{ seconds: number; percent: number | null } | null>(null);
  let finishDefault = $state<PodcastSettings['default'] | null>(null);
  let savingFinish = $state(false);

  const libraryId = $derived(page.url.searchParams.get('library') ?? libraries[0]?.id ?? null);
  const library = $derived(libraries.find((l) => l.id === libraryId) ?? null);

  async function loadLibraries() {
    const { data, error: err } = await api<Library[]>('/api/v1/admin/libraries');
    if (err) error = err;
    libraries = (data ?? []).filter((l) => l.kind === 'podcasts');
    loaded = true;
  }

  async function loadShows() {
    if (!libraryId) {
      shows = null;
      return;
    }
    const { data, error: err } = await api<PodcastSummary[]>(
      `/api/v1/admin/libraries/${libraryId}/podcasts`,
    );
    if (err) error = err;
    shows = data ?? [];
  }

  onMount(async () => {
    await loadLibraries();
  });

  async function loadFinish() {
    finish = null;
    if (!libraryId) return;
    const { data, error: err } = await api<PodcastSettings>(
      `/api/v1/admin/libraries/${libraryId}/podcast-settings`,
    );
    if (err) error = err;
    if (data) {
      finish = { seconds: data.finish_remaining_seconds, percent: data.finish_percent };
      finishDefault = data.default;
    }
  }

  async function saveFinish(event: SubmitEvent) {
    event.preventDefault();
    if (!libraryId || !finish || savingFinish) return;
    error = null;
    notice = null;
    // An emptied number input binds null (or ''): both mean "off".
    const raw: unknown = finish.percent;
    const percent =
      raw === null || raw === undefined || raw === '' || Number.isNaN(Number(raw))
        ? null
        : Number(raw);
    savingFinish = true;
    const { data, error: err } = await api<PodcastSettings>(
      `/api/v1/admin/libraries/${libraryId}/podcast-settings`,
      {
        method: 'PUT',
        body: JSON.stringify({
          finish_remaining_seconds: Number(finish.seconds),
          finish_percent: percent,
        }),
      },
    );
    savingFinish = false;
    if (data) {
      finish = { seconds: data.finish_remaining_seconds, percent: data.finish_percent };
      notice = 'Finish rule saved. It applies from the next listen.';
    } else error = err ?? 'Could not save the finish rule.';
  }

  $effect(() => {
    // Reload whenever the chosen library changes.
    void libraryId;
    report = null;
    void loadShows();
    void loadFinish();
  });

  function pickLibrary(id: string) {
    void goto(`/admin/podcasts?library=${encodeURIComponent(id)}`, { keepFocus: true });
  }

  async function addShow(event: SubmitEvent) {
    event.preventDefault();
    if (!libraryId || adding) return;
    error = null;
    notice = null;
    report = null;
    adding = true;
    const { data, error: err } = await api<PodcastShow>(
      `/api/v1/admin/libraries/${libraryId}/podcasts`,
      { method: 'POST', body: JSON.stringify({ feed_url: feedUrl }) },
    );
    adding = false;
    if (data) {
      feedUrl = '';
      const n = data.episodes.length;
      notice = `Added “${data.title}” with ${n} episode${n === 1 ? '' : 's'}. Nothing is downloaded until you ask, or turn on auto-download.`;
      await loadShows();
    } else error = err ?? 'Could not add that feed.';
  }

  async function importOpml(event: SubmitEvent) {
    event.preventDefault();
    const file = opmlInput?.files?.[0];
    if (!libraryId || !file || importing) return;
    error = null;
    notice = null;
    report = null;
    importing = true;
    const text = await file.text();
    const { data, error: err } = await api<OpmlImportReport>(
      `/api/v1/admin/libraries/${libraryId}/podcasts/opml`,
      { method: 'POST', headers: { 'content-type': 'text/x-opml' }, body: text },
    );
    importing = false;
    if (data) {
      report = data;
      if (opmlInput) opmlInput.value = '';
      await loadShows();
    } else error = err ?? 'Could not import that file.';
  }

  async function refresh(show: PodcastSummary) {
    error = null;
    notice = null;
    const { status, error: err } = await api(`/api/v1/admin/podcasts/${show.id}/refresh`, {
      method: 'POST',
    });
    if (status === 202) notice = `A check of “${show.title}” is queued.`;
    else error = err ?? 'Could not queue the check.';
  }

  function status(show: PodcastSummary): { text: string; bad: boolean } {
    if (show.consecutive_failures > 0) {
      const n = show.consecutive_failures;
      return { text: `Failing (${n} check${n === 1 ? '' : 's'} in a row)`, bad: true };
    }
    if (!show.last_checked_at) return { text: 'Not checked yet', bad: false };
    return { text: 'OK', bad: false };
  }
</script>

<h2>Podcasts</h2>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}
  <p class="notice" role="status">{notice} <a class="link" href="/admin/jobs">Open Jobs</a></p>
{/if}

{#if !loaded}
  <p class="dim">Loading…</p>
{:else if libraries.length === 0}
  <p class="dim">
    There is no podcasts library yet. On <a class="link" href="/admin/library">Server → Library</a>,
    add a library of kind Podcasts whose folder is a writable folder kept for Parlour's
    downloads — never one of your media folders. Shows are then added here.
  </p>
{:else}
  {#if libraries.length > 1}
    <div class="row">
      <label for="podcast-library">Library</label>
      <select
        id="podcast-library"
        value={libraryId}
        onchange={(e) => pickLibrary((e.currentTarget as HTMLSelectElement).value)}
      >
        {#each libraries as lib (lib.id)}
          <option value={lib.id}>{lib.name}</option>
        {/each}
      </select>
    </div>
  {/if}

  {#if library}
    <p class="dim">
      Everyone with access to <strong>{library.name}</strong> sees every show in it. New shows
      start with auto-download off, so nothing is downloaded until you turn it on for a show or
      download an episode by hand.
    </p>

    {#if shows === null}
      <p class="dim">Loading…</p>
    {:else if shows.length === 0}
      <p class="dim">No shows yet. Add one by its feed address, or import an OPML file.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Show</th>
              <th>Episodes</th>
              <th>Last checked</th>
              <th>Status</th>
              <th>Auto-download</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {#each shows as show (show.id)}
              {@const st = status(show)}
              <tr>
                <td>
                  <a class="link" href={`/admin/podcasts/${show.id}`}>{show.title}</a>
                  {#if show.author}<div class="dim small">{show.author}</div>{/if}
                </td>
                <td>{show.downloaded} of {show.episodes} downloaded</td>
                <td>{show.last_checked_at ? formatDateTime(show.last_checked_at) : '—'}</td>
                <td class:bad={st.bad}>
                  {st.text}
                  {#if show.last_error}<div class="dim small">{show.last_error}</div>{/if}
                </td>
                <td>
                  {#if show.auto_download}On <span class="dim mono">{show.check_schedule}</span>{:else}Off{/if}
                </td>
                <td>
                  <div class="actions">
                    <a class="btn small" href={`/admin/podcasts/${show.id}`}>Manage</a>
                    <button class="btn small" onclick={() => refresh(show)}>Refresh now</button>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    <form class="form" onsubmit={addShow}>
      <h3>Add a show</h3>
      <div class="field">
        <label for="feed-url">Feed address (RSS)</label>
        <input
          id="feed-url"
          type="text"
          inputmode="url"
          autocomplete="off"
          spellcheck="false"
          placeholder="https://…"
          bind:value={feedUrl}
          required
        />
      </div>
      <p class="dim small">
        The feed is fetched and read right away: an address that isn't a podcast feed is refused,
        and a feed that reads is added with its episode list.
      </p>
      <button class="btn primary" type="submit" disabled={adding}
        >{adding ? 'Checking the feed…' : 'Add show'}</button
      >
    </form>

    <h3>OPML</h3>
    <p class="dim">
      OPML is how podcast apps move a list of shows. Importing adds every feed in the file the way
      adding one does; feeds already here, or listed twice, are skipped. Exporting downloads this
      library's shows as an OPML file — it includes each feed's full address.
    </p>
    <form class="row" onsubmit={importOpml}>
      <label for="opml-file">OPML file</label>
      <input id="opml-file" type="file" accept=".opml,.xml,text/x-opml,text/xml" bind:this={opmlInput} required />
      <button class="btn" type="submit" disabled={importing}
        >{importing ? 'Importing…' : 'Import'}</button
      >
      <a class="btn" href={`/api/v1/admin/libraries/${library.id}/podcasts/opml`} download
        >Export OPML</a
      >
    </form>

    {#if report}
      <section class="report" aria-label="OPML import report">
        <p role="status">
          {report.feeds} feed{report.feeds === 1 ? '' : 's'} in the file: {report.added} added,
          {report.skipped} skipped, {report.refused} not added.
        </p>
        {#if report.lines.length}
          <div class="table-wrap">
            <table>
              <thead><tr><th>Feed</th><th>Result</th></tr></thead>
              <tbody>
                {#each report.lines as line, i (i)}
                  <tr>
                    <td>
                      {#if line.podcast_id}
                        <a class="link" href={`/admin/podcasts/${line.podcast_id}`}>{line.name}</a>
                      {:else}{line.name}{/if}
                    </td>
                    <td class:bad={line.outcome === 'refused'}>
                      {OPML_OUTCOME_LABELS[line.outcome]}{#if line.error}: {line.error}{/if}
                      {#if line.outcome === 'added' && line.new_episodes !== null}
                        <span class="dim">({line.new_episodes} episodes)</span>
                      {/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </section>
    {/if}

    {#if finish}
      <form class="form" onsubmit={saveFinish} aria-label="When an episode counts as finished">
        <h3>When an episode counts as finished</h3>
        <p class="dim small">
          As for audiobooks: an episode counts as finished once less than the given time is left,
          or — when a percentage is set — once listening passes it, which then replaces the time.
          {#if finishDefault}Audiobookshelf's default is {finishDefault.finish_remaining_seconds} s.{/if}
        </p>
        <div class="field">
          <label for="finish-s">Finished with less than this left (seconds, 0 = off)</label>
          <input id="finish-s" type="number" min="0" max="86400" bind:value={finish.seconds} />
        </div>
        <div class="field">
          <label for="finish-p">Or finished past this percentage (blank = off)</label>
          <input id="finish-p" type="number" min="1" max="100" bind:value={finish.percent} />
        </div>
        <button class="btn primary" type="submit" disabled={savingFinish}>Save finish rule</button>
      </form>
    {/if}
  {/if}
{/if}

<style>
  .link {
    color: var(--accent);
  }
  .small {
    font-size: 0.85rem;
  }
  .bad {
    color: var(--danger);
  }
  .report {
    margin-bottom: 1.5rem;
  }
  h3 {
    margin-top: 1.4rem;
  }
</style>
