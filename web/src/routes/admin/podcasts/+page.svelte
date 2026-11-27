<script lang="ts">
  // Server → Podcasts (POD4, D31/D32): an admin manages a podcasts
  // library's shows, as Audiobookshelf lets one — add a show by its feed
  // address, import or export OPML, refresh a show now. Only admins add
  // shows (D31); everyone with access to the library sees every show
  // (the listener pages are POD6's). A feed's address may hold a private
  // token, so errors never repeat it. POD5 adds the library's finish
  // rule for episodes (AB11's, for a podcasts library). POD7 adds the
  // third way to add a show: searching Apple's podcast directory. Only
  // the server asks Apple; this page sends the query to Parlour and adds
  // a result through the same add as the address form.
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import ShowArt from '$lib/ShowArt.svelte';
  import {
    formatDate,
    formatDateTime,
    OPML_OUTCOME_LABELS,
    type DirectoryResult,
    type DirectorySearch,
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
  /** POD7: Apple's directory search. */
  let query = $state('');
  let country = $state('US');
  let searching = $state(false);
  let directory = $state<DirectorySearch | null>(null);
  let searchError = $state<string | null>(null);
  /** The result being added (its Apple id), and each result's error. */
  let addingId = $state<number | null>(null);
  let resultErrors = $state<Record<number, string>>({});

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
    directory = null;
    searchError = null;
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

  async function searchDirectory(event: SubmitEvent) {
    event.preventDefault();
    if (!libraryId || searching) return;
    error = null;
    notice = null;
    searchError = null;
    resultErrors = {};
    searching = true;
    const { data, error: err } = await api<DirectorySearch>(
      `/api/v1/admin/libraries/${libraryId}/podcasts/search`,
      { method: 'POST', body: JSON.stringify({ query, country }) },
    );
    searching = false;
    directory = data;
    if (data) country = data.country;
    else searchError = err ?? 'The search failed.';
  }

  async function addFromResult(result: DirectoryResult) {
    if (!libraryId || !result.feed_url || addingId !== null) return;
    error = null;
    notice = null;
    resultErrors = { ...resultErrors, [result.itunes_id]: '' };
    addingId = result.itunes_id;
    const { data, error: err } = await api<PodcastShow>(
      `/api/v1/admin/libraries/${libraryId}/podcasts`,
      {
        method: 'POST',
        body: JSON.stringify({ feed_url: result.feed_url, itunes_id: result.itunes_id }),
      },
    );
    addingId = null;
    if (data) {
      result.added = { podcast_id: data.id, title: data.title };
      const n = data.episodes.length;
      notice = `Added “${data.title}” with ${n} episode${n === 1 ? '' : 's'}. Nothing is downloaded until you ask, or turn on auto-download.`;
      await loadShows();
    } else {
      resultErrors = { ...resultErrors, [result.itunes_id]: err ?? 'Could not add that show.' };
    }
  }

  function resultFacts(r: DirectoryResult): string {
    const parts: string[] = [];
    if (r.genres.length) parts.push(r.genres.slice(0, 3).join(', '));
    if (r.episode_count !== null)
      parts.push(`${r.episode_count} episode${r.episode_count === 1 ? '' : 's'}`);
    if (r.latest_episode_at) parts.push(`latest ${formatDate(r.latest_episode_at)}`);
    return parts.join(' · ');
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
      <p class="dim">No shows yet. Find one in Apple's podcast directory, add one by its feed address, or import an OPML file.</p>
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

    <form class="form" onsubmit={searchDirectory} aria-label="Search Apple's podcast directory">
      <h3>Find a show in Apple's podcast directory</h3>
      <div class="row search-row">
        <label class="sr-only" for="directory-query">Show or host name</label>
        <input
          id="directory-query"
          type="search"
          autocomplete="off"
          placeholder="Show or host name"
          maxlength="200"
          bind:value={query}
          required
        />
        <span class="store">
          <label for="directory-country">Store</label>
          <input
            id="directory-country"
            class="country"
            type="text"
            autocomplete="off"
            spellcheck="false"
            maxlength="2"
            title="Apple's store country, two letters (US, CA, GB …)"
            bind:value={country}
          />
        </span>
        <button class="btn primary" type="submit" disabled={searching}
          >{searching ? 'Searching…' : 'Search'}</button
        >
      </div>
      <p class="dim small">
        Parlour asks Apple's podcast directory for you; no Apple account or key is needed. Pick a
        result to add it: its feed is read right away, exactly as when adding by address.
      </p>
    </form>

    {#if searchError}<p class="error" role="alert">{searchError}</p>{/if}
    {#if directory}
      <section class="directory" aria-label="Directory results">
        <p class="dim" role="status">
          {#if directory.results.length === 0}
            Apple's directory ({directory.country}) found no shows for that search.
          {:else}
            {directory.results.length} show{directory.results.length === 1 ? '' : 's'} in Apple's
            directory ({directory.country}).
          {/if}
        </p>
        {#if directory.results.length}
          <ul class="results">
            {#each directory.results as r (r.itunes_id)}
              <li class="result" data-itunes-id={r.itunes_id}>
                <div class="result-art">
                  <ShowArt url={r.artwork_url} title={r.title} size="sm" noReferrer />
                </div>
                <div class="result-info">
                  <div class="result-title">
                    <strong>{r.title}</strong>
                    {#if r.explicit}<span class="badge">Explicit</span>{/if}
                  </div>
                  {#if r.author}<div class="small">{r.author}</div>{/if}
                  {#if resultFacts(r)}<div class="dim small">{resultFacts(r)}</div>{/if}
                  {#if r.feed_url}<div class="dim small mono feed">{r.feed_url}</div>{/if}
                  {#if resultErrors[r.itunes_id]}
                    <p class="error small" role="alert">{resultErrors[r.itunes_id]}</p>
                  {/if}
                </div>
                <div class="result-act">
                  {#if r.added}
                    <span class="added">Already added</span>
                    <a class="btn small" href={`/admin/podcasts/${r.added.podcast_id}`}>Open</a>
                  {:else if !r.feed_url}
                    <span class="dim small"
                      >Apple lists no feed address for this show, so it can't be added.</span
                    >
                  {:else}
                    <button
                      class="btn small primary"
                      disabled={addingId !== null}
                      onclick={() => addFromResult(r)}
                      >{addingId === r.itunes_id ? 'Checking the feed…' : 'Add'}</button
                    >
                  {/if}
                </div>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    <form class="form" onsubmit={addShow}>
      <h3>Add a show by its address</h3>
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
  .search-row input[type='search'] {
    flex: 1 1 16rem;
    min-width: 0;
  }
  .store {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
  }
  .country {
    width: 3.2rem;
    text-transform: uppercase;
  }
  .directory {
    margin-bottom: 1.5rem;
  }
  .results {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .result {
    display: grid;
    grid-template-columns: 4rem 1fr auto;
    gap: 0.8rem;
    align-items: start;
    padding: 0.7rem 0;
    border-bottom: 1px solid var(--line);
  }
  .result-info {
    min-width: 0;
  }
  .result-title {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .feed {
    overflow-wrap: anywhere;
  }
  .result-act {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    align-items: flex-end;
    max-width: 14rem;
    text-align: right;
  }
  .added {
    color: var(--text-dim);
    font-size: 0.85rem;
  }
  .badge {
    font-size: 0.7rem;
    padding: 0.05rem 0.35rem;
    border: 1px solid var(--line);
    border-radius: 0.3rem;
    color: var(--text-dim);
  }
  @media (max-width: 560px) {
    .result {
      grid-template-columns: 3rem 1fr;
    }
    .result-act {
      grid-column: 2;
      align-items: flex-start;
      text-align: left;
    }
  }
  h3 {
    margin-top: 1.4rem;
  }
</style>
