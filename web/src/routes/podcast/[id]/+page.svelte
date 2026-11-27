<script lang="ts">
  // POD6: a show's page for a listener (UI-AUDIO-REFERENCE §4.4 "Show
  // page"): artwork (Parlour's cached copy, else an initial tile),
  // title, author, the feed's description shown safely (`feedHtml`),
  // and the episode list as POD5 serves it — a serial show oldest
  // first, an episodic one newest first. Each row has the episode's
  // title, date, length and this profile's state: new, part-played
  // (a progress ring and the time left) or played (a check).
  //
  // Not downloaded (D32 as amended 2026-11-27): the row says so, and
  // that it plays from the publisher (its stream redirects there).
  // Admins can queue the download here (POD4's `{"episode_id"}` hook);
  // the row follows the job until the file is in. Play opens the
  // episode's page and starts it; "Mark as played" / "unplayed" write
  // progress through POD5 (AB11's rules: un-playing goes back to 0).
  //
  // A show the profile can't reach — no grant, or an explicit show with
  // D41's flag off — is the API's 404 and reads as "not found" here,
  // with nothing about the show shown.
  import { onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import EpisodeMark from '$lib/EpisodeMark.svelte';
  import { feedHtml, feedText } from '$lib/feedHtml';
  import { session } from '$lib/session.svelte';
  import ShowArt from '$lib/ShowArt.svelte';
  import {
    episodeFraction,
    episodeMarks,
    episodeState,
    formatDate,
    formatDurationMs,
    type BookProgress,
    type ListenerEpisode,
    type ListenerShowDetail,
  } from '$lib/types';

  const PAGE = 50;

  let show = $state<ListenerShowDetail | null>(null);
  let missing = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let shown = $state(PAGE);
  let expanded = $state(false);
  /** Episodes whose download this page queued, until they are in. */
  let queued = $state<string[]>([]);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let polls = 0;

  const id = $derived(page.params.id ?? '');
  const isAdmin = $derived(!!session.me?.user.is_admin);

  async function load() {
    const { status, data, error: err } = await api<ListenerShowDetail>(
      `/api/v1/podcasts/${id}?client=web`,
    );
    if (status === 404) {
      missing = true;
      show = null;
      return;
    }
    if (!data) {
      error = err ?? 'Could not load this show.';
      return;
    }
    show = data;
    queued = queued.filter((q) => !data.episodes.some((e) => e.id === q && e.downloaded));
    schedulePoll();
  }

  /** While a download this page queued is waiting, look again every
   *  3 s (for at most ten minutes). */
  function schedulePoll() {
    if (timer) clearTimeout(timer);
    timer = null;
    if (queued.length === 0 || polls > 200) return;
    polls += 1;
    timer = setTimeout(() => void load(), 3000);
  }

  $effect(() => {
    void id;
    missing = false;
    show = null;
    error = null;
    shown = PAGE;
    expanded = false;
    queued = [];
    polls = 0;
    void load();
  });

  onDestroy(() => {
    if (timer) clearTimeout(timer);
  });

  const description = $derived(feedHtml(show?.description));
  const longDescription = $derived((show?.description ?? '').length > 600);

  function timeLine(e: ListenerEpisode): string {
    const len = e.length_ms;
    const st = episodeState(e.progress);
    if (st === 'in_progress' && len) {
      const left = Math.max(0, len - e.progress.position_ms);
      return `${formatDurationMs(left) || '0:00'} left`;
    }
    return formatDurationMs(len);
  }

  function summary(e: ListenerEpisode): string {
    const text = feedText(e.subtitle || e.description);
    return text.length > 180 ? `${text.slice(0, 177).trimEnd()}…` : text;
  }

  function setProgress(e: ListenerEpisode, p: BookProgress) {
    if (!show) return;
    show = {
      ...show,
      episodes: show.episodes.map((x) => (x.id === e.id ? { ...x, progress: p } : x)),
    };
  }

  async function mark(e: ListenerEpisode, finished: boolean) {
    error = null;
    const { data, error: err } = await api<BookProgress>(
      `/api/v1/podcast-episodes/${e.id}/progress`,
      {
        method: 'PUT',
        body: JSON.stringify({ finished, client_updated_at: new Date().toISOString() }),
      },
    );
    if (data) setProgress(e, data);
    else error = err ?? 'Could not save that.';
  }

  async function download(e: ListenerEpisode) {
    error = null;
    notice = null;
    const { status, error: err } = await api(`/api/v1/admin/podcast-episodes/${e.id}/download`, {
      method: 'POST',
    });
    if (status === 202) {
      notice = `“${e.title}” is queued for download.`;
      queued = [...queued, e.id];
      polls = 0;
      schedulePoll();
    } else if (status === 409) {
      notice = `“${e.title}” is already downloaded.`;
      await load();
    } else {
      error = err ?? 'Could not queue the download.';
    }
  }

  function play(e: ListenerEpisode) {
    void goto(`/episode/${e.id}?play=1`);
  }
</script>

{#if missing}
  <h1>Not found</h1>
  <p class="dim">There is no such show, or it isn't available to this profile.</p>
{:else if error && !show}
  <p class="error" role="alert">{error}</p>
{:else if !show}
  <p class="dim">Loading…</p>
{:else}
  <div class="head">
    <div class="cover"><ShowArt url={show.artwork_url} title={show.title} size="lg" /></div>
    <div class="info">
      <h1>{show.title}</h1>
      {#if show.author}<p class="author">{show.author}</p>{/if}
      <p class="dim facts">
        {[
          show.episode_count === 1 ? '1 episode' : `${show.episode_count} episodes`,
          show.podcast_type === 'serial' ? 'Serial' : null,
          show.genres.join(', ') || null,
          show.language,
        ]
          .filter(Boolean)
          .join(' · ')}
        {#if show.explicit}<span class="badge" title="The feed marks this show explicit">Explicit</span
          >{/if}
      </p>
      <p class="dim mine" data-testid="show-counts">
        {show.finished_count} played · {show.in_progress_count} in progress
      </p>
      {#if description}
        <div class="notes" class:clamped={longDescription && !expanded}>
          <!-- feedHtml rebuilt this from an allowlist: no scripts, no
               images, no attributes but a checked link address. -->
          {@html description}
        </div>
        {#if longDescription}
          <button class="link" type="button" onclick={() => (expanded = !expanded)}
            >{expanded ? 'Less' : 'More'}</button
          >
        {/if}
      {/if}
      {#if isAdmin}
        <p class="admin"><a href={`/admin/podcasts/${show.id}`}>Manage this show</a></p>
      {/if}
    </div>
  </div>

  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <section aria-label="Episodes">
    <h2>Episodes</h2>
    {#if show.episodes.length === 0}
      <p class="dim">The feed lists no episodes yet.</p>
    {:else}
      <ol class="episodes">
        {#each show.episodes.slice(0, shown) as e (e.id)}
          {@const st = episodeState(e.progress)}
          <li class="row" class:played={st === 'finished'} data-episode-id={e.id} data-state={st}>
            <EpisodeMark status={st} fraction={episodeFraction(e)} />
            <div class="main">
              <a class="title" href={`/episode/${e.id}`}>{e.title}</a>
              <p class="meta dim">
                {[formatDate(e.published_at), timeLine(e), episodeMarks(e)]
                  .filter(Boolean)
                  .join(' · ')}
              </p>
              {#if !e.downloaded}
                <p class="where dim" data-testid="not-downloaded">
                  Not downloaded to Parlour — plays from the publisher.
                  {#if queued.includes(e.id)}<span class="queued">Download queued…</span>{/if}
                </p>
              {/if}
              {#if summary(e)}<p class="summary dim">{summary(e)}</p>{/if}
            </div>
            <div class="actions">
              <button class="btn small primary" type="button" onclick={() => play(e)}
                >{st === 'in_progress' ? 'Resume' : 'Play'}</button
              >
              {#if st === 'finished'}
                <button class="btn small" type="button" onclick={() => mark(e, false)}
                  >Mark as unplayed</button
                >
              {:else}
                <button class="btn small" type="button" onclick={() => mark(e, true)}
                  >Mark as played</button
                >
              {/if}
              {#if isAdmin && !e.downloaded}
                <button
                  class="btn small"
                  type="button"
                  disabled={queued.includes(e.id)}
                  onclick={() => download(e)}>Download</button
                >
              {/if}
            </div>
          </li>
        {/each}
      </ol>
      {#if show.episodes.length > shown}
        <p>
          <button class="btn" type="button" onclick={() => (shown += PAGE)}
            >Show more ({show.episodes.length - shown} left)</button
          >
        </p>
      {/if}
    {/if}
  </section>
{/if}

<style>
  .head {
    display: grid;
    grid-template-columns: minmax(140px, 220px) 1fr;
    gap: 1.6rem;
    margin: 1rem 0 1.6rem;
  }
  @media (max-width: 640px) {
    .head {
      grid-template-columns: 1fr;
    }
    .cover {
      max-width: 220px;
    }
  }
  h1 {
    margin: 0 0 0.3rem;
  }
  .info p {
    margin: 0.2rem 0;
  }
  .author {
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .facts {
    font-size: 0.9rem;
  }
  .badge {
    display: inline-block;
    margin-left: 0.4rem;
    padding: 0 0.4rem;
    border: 1px solid var(--line);
    border-radius: 0.3rem;
    font-size: 0.75rem;
  }
  .mine {
    font-size: 0.9rem;
  }
  .notes {
    margin-top: 0.8rem;
    max-width: 65ch;
    line-height: 1.5;
    overflow-wrap: anywhere;
  }
  .notes.clamped {
    max-height: 9.5em;
    overflow: hidden;
    mask-image: linear-gradient(to bottom, black 70%, transparent);
  }
  .notes :global(p) {
    margin: 0 0 0.6rem;
  }
  .notes :global(a) {
    color: var(--accent);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
  .admin {
    margin-top: 0.8rem !important;
    font-size: 0.9rem;
  }
  .admin a {
    color: var(--text-dim);
  }
  .notice {
    color: var(--accent);
  }
  h2 {
    font-size: 1.15rem;
  }
  .episodes {
    list-style: none;
    margin: 0;
    padding: 0;
    max-width: 56rem;
  }
  .row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 0.8rem;
    align-items: start;
    padding: 0.75rem 0.2rem;
    border-bottom: 1px solid var(--line);
  }
  .row :global(.mark) {
    margin-top: 0.15rem;
  }
  .row.played .title {
    color: var(--text-dim);
  }
  .main {
    min-width: 0;
  }
  .main p {
    margin: 0.15rem 0 0;
  }
  .title {
    color: inherit;
    font-weight: 600;
    text-decoration: none;
  }
  .title:hover,
  .title:focus-visible {
    color: var(--accent);
  }
  .meta,
  .where {
    font-size: 0.85rem;
  }
  .queued {
    color: var(--accent);
    margin-left: 0.3rem;
  }
  .summary {
    font-size: 0.85rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    justify-content: flex-end;
    max-width: 16rem;
  }
  @media (max-width: 640px) {
    .row {
      grid-template-columns: auto 1fr;
    }
    .actions {
      grid-column: 2;
      justify-content: flex-start;
    }
  }
</style>
