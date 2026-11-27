<script lang="ts">
  // POD6: an episode's page and its player — AB5's player, for an
  // episode (UI-AUDIO-REFERENCE §4.4 "Player"): the show's name over
  // the episode's title, an episode-wide scrubber with the current
  // chapter's title above it, back 15 s / play / forward 30 s, next
  // chapter, then the show notes and the chapter list.
  //
  // Where the audio comes from is POD5's `delivery`:
  // - `file`: the download, with Range;
  // - `aac`: AB10's AAC copy for a file the web profile can't play —
  //   it answers 503 until it is made, so the player waits for it, as
  //   the book page does;
  // - `enclosure`: not downloaded; the stream route redirects to the
  //   publisher (D32 as amended), so it plays whatever they send, with
  //   no audio plan, and the page says so.
  //
  // Progress (AB11's rules, POD5's routes) is written every 15 s while
  // playing, on pause, on a seek and when the page goes away — only
  // once this page has played, so opening an episode changes nothing —
  // and the end of the audio marks it played. Listening is a session
  // (AB8, widened by POD5): the playback reports name `episode_id`.
  //
  // Chapters are read by the server (`…/chapters`: the download's own
  // marks, else the feed's chapter list); the page never asks a
  // publisher for anything but the audio itself.
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import EpisodeMark from '$lib/EpisodeMark.svelte';
  import { feedHtml } from '$lib/feedHtml';
  import { session } from '$lib/session.svelte';
  import ShowArt from '$lib/ShowArt.svelte';
  import {
    chapterIndexAt,
    deviceLabel,
    episodeFraction,
    episodeMarks,
    episodeState,
    formatDate,
    formatDurationMs,
    type BookProgress,
    type EpisodeChapters,
    type ListenerEpisode,
  } from '$lib/types';

  const SAVE_EVERY_MS = 15_000;
  const REPORT_EVERY_MS = 15_000;
  const BACK_MS = 15_000;
  const FORWARD_MS = 30_000;
  /** The profile this page plays under (AB10, CL5's profiles). */
  const PROFILE = 'web';

  let ep = $state<ListenerEpisode | null>(null);
  let missing = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let chapters = $state<EpisodeChapters | null>(null);
  let audio = $state<HTMLAudioElement | null>(null);
  let position = $state(0);
  let mediaMs = $state(0);
  let playing = $state(false);
  let started = $state(false);
  let saveError = $state<string | null>(null);
  let playError = $state<string | null>(null);

  let pendingMs: number | null = null;
  let lastSavedAt = 0;
  let lastSavedPos = -1;

  // AB8: the listening session this page has open.
  let sessionId: string | null = null;
  let sessionOpening = false;
  let reportTimer: ReturnType<typeof setInterval> | null = null;
  let listenedMs = 0;
  let playingSince: number | null = null;
  let stoppedByAdmin = $state(false);

  // AB10: waiting for the AAC copy.
  let preparing = $state(false);
  let preparedSeconds = $state(0);
  let wantPlay = false;
  let destroyed = false;

  /** The length the scrubber runs over: what the browser measures once
   *  it knows, else the server's (the download's, else the feed's). */
  const total = $derived(mediaMs > 0 ? mediaMs : (ep?.length_ms ?? 0));
  const list = $derived(chapters?.chapters ?? []);
  const chapterIndex = $derived(chapterIndexAt(list, position));
  const resumable = $derived(!!ep && !ep.progress.finished && ep.progress.position_ms > 0);
  const notes = $derived(feedHtml(ep?.description));
  const status = $derived(ep ? episodeState(ep.progress) : 'new');

  async function load(id: string) {
    const { status, data, error: err } = await api<ListenerEpisode>(
      `/api/v1/podcast-episodes/${id}?client=${PROFILE}`,
    );
    if (status === 404) {
      missing = true;
      return;
    }
    if (!data) {
      error = err ?? 'Could not load this episode.';
      return;
    }
    ep = data;
    position = data.progress.finished ? 0 : data.progress.position_ms;
    pendingMs = position;
    void loadChapters(id);
    const autoplay = page.url.searchParams.get('play') === '1';
    await Promise.resolve();
    void setSource(autoplay);
  }

  async function loadChapters(id: string) {
    const { data } = await api<EpisodeChapters>(`/api/v1/podcast-episodes/${id}/chapters`);
    if (data && !destroyed) chapters = data;
  }

  /** Point the <audio> at the episode: at once for the download or the
   *  publisher, after the server has made it for the AAC copy. */
  async function setSource(play: boolean) {
    if (!ep) return;
    if (ep.delivery === 'aac') {
      preparing = true;
      wantPlay = play;
      preparedSeconds = 0;
      const ready = await waitForCopy(ep.stream_url);
      if (destroyed) return;
      preparing = false;
      if (!ready) {
        playError = 'This episode could not be converted for this browser.';
        return;
      }
      play = wantPlay;
      wantPlay = false;
    }
    // The <audio> renders with the episode; wait for it if need be.
    for (let i = 0; !audio && i < 50; i++) await new Promise((r) => setTimeout(r, 20));
    if (!audio || destroyed) return;
    audio.src = ep.stream_url;
    audio.load();
    if (play) void audio.play().catch(() => {});
  }

  /** Poll the copy until it is there (200), honouring Retry-After. */
  async function waitForCopy(url: string): Promise<boolean> {
    const t0 = Date.now();
    while (!destroyed) {
      const res = await fetch(url, { method: 'HEAD', credentials: 'same-origin' }).catch(() => null);
      if (res?.ok) return true;
      if (res && res.status !== 503) return false;
      const wait = Number(res?.headers.get('retry-after')) || 2;
      await new Promise((r) => setTimeout(r, wait * 1000));
      preparedSeconds = Math.round((Date.now() - t0) / 1000);
    }
    return false;
  }

  function onLoadedMetadata() {
    if (!audio) return;
    if (Number.isFinite(audio.duration) && audio.duration > 0) {
      mediaMs = Math.round(audio.duration * 1000);
    }
    if (pendingMs !== null) {
      audio.currentTime = pendingMs / 1000;
      pendingMs = null;
    }
  }

  function onDurationChange() {
    if (audio && Number.isFinite(audio.duration) && audio.duration > 0) {
      mediaMs = Math.round(audio.duration * 1000);
    }
  }

  function onTimeUpdate() {
    if (!audio || pendingMs !== null) return;
    position = Math.round(audio.currentTime * 1000);
    if (playing && Date.now() - lastSavedAt >= SAVE_EVERY_MS) void save();
  }

  function onEnded() {
    playing = false;
    position = total;
    void save({ finished: true });
    endSession(false);
  }

  function onError() {
    if (!ep || !audio?.error) return;
    playError =
      ep.delivery === 'enclosure'
        ? "The publisher's copy of this episode couldn't be played. Their server may be down or the file moved."
        : "This episode couldn't be played.";
  }

  /** Listening time since the last report, and start counting again. */
  function takeListened(): number {
    const now = performance.now();
    if (playingSince !== null) {
      listenedMs += now - playingSince;
      playingSince = now;
    }
    const out = Math.round(listenedMs);
    listenedMs = 0;
    return out;
  }

  function onPlay() {
    playing = true;
    started = true;
    playError = null;
    playingSince = performance.now();
    void openSession();
  }

  function onPause() {
    if (playingSince !== null) listenedMs += performance.now() - playingSince;
    playingSince = null;
    playing = false;
    void save();
    void report();
  }

  function reportBody(extra: Record<string, unknown> = {}): Record<string, unknown> {
    return {
      episode_id: ep?.id,
      position_ms: Math.round(position),
      duration_ms: total > 0 ? total : undefined,
      ...extra,
    };
  }

  /** Open a session on the first play (and again after one closed). */
  async function openSession() {
    if (!ep || sessionId || sessionOpening || stoppedByAdmin) return;
    sessionOpening = true;
    const { data } = await api<{ session_id: string }>('/api/v1/playback/start', {
      method: 'POST',
      body: JSON.stringify(
        reportBody({ client_name: 'Parlour Web', device_name: deviceLabel(navigator.userAgent) }),
      ),
    });
    sessionOpening = false;
    if (!data) return;
    sessionId = data.session_id;
    listenedMs = 0;
    if (playingSince !== null) playingSince = performance.now();
    if (!reportTimer) reportTimer = setInterval(() => void report(), REPORT_EVERY_MS);
  }

  async function report() {
    if (!ep || !sessionId) return;
    const { status, data } = await api<{ session_id: string }>('/api/v1/playback/progress', {
      method: 'POST',
      body: JSON.stringify(reportBody({ session_id: sessionId, listened_ms: takeListened() })),
    });
    if (data?.session_id && sessionId) sessionId = data.session_id;
    // 409: an admin stopped this session and its stream.
    if (status === 409) {
      stoppedByAdmin = true;
      endSession(false, false);
      audio?.pause();
    }
  }

  /** Close the session: the episode ended, or the page is going away
   *  (`keepalive` so the report survives the page). */
  function endSession(keepalive: boolean, tell = true) {
    if (reportTimer) clearInterval(reportTimer);
    reportTimer = null;
    const sid = sessionId;
    sessionId = null;
    if (!ep || !sid || !tell) return;
    const body = JSON.stringify(reportBody({ session_id: sid, listened_ms: takeListened() }));
    playingSince = null;
    if (keepalive) {
      void fetch('/api/v1/playback/stopped', {
        method: 'POST',
        credentials: 'same-origin',
        keepalive: true,
        headers: { 'content-type': 'application/json' },
        body,
      }).catch(() => {});
    } else {
      void api('/api/v1/playback/stopped', { method: 'POST', body });
    }
  }

  function progressPath(): string {
    return `/api/v1/podcast-episodes/${ep?.id}/progress`;
  }

  async function save(extra: { finished?: boolean } = {}) {
    if (!ep || (!started && extra.finished === undefined)) return;
    const pos = Math.round(position);
    if (pos === lastSavedPos && extra.finished === undefined) return;
    lastSavedAt = Date.now();
    lastSavedPos = pos;
    const { data, error: err } = await api<BookProgress>(progressPath(), {
      method: 'PUT',
      body: JSON.stringify({ position_ms: pos, ...extra, client_updated_at: new Date().toISOString() }),
    });
    if (data && ep) {
      ep = { ...ep, progress: data };
      saveError = null;
    } else {
      saveError = err ?? 'Could not save your place.';
    }
  }

  /** Last write on the way out: keepalive so it survives the page. */
  function saveOnLeave() {
    if (!ep || !started) return;
    const pos = Math.round(position);
    if (pos === lastSavedPos) return;
    lastSavedPos = pos;
    void fetch(progressPath(), {
      method: 'PUT',
      credentials: 'same-origin',
      keepalive: true,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ position_ms: pos, client_updated_at: new Date().toISOString() }),
    }).catch(() => {});
  }

  /** Move the playhead; while the source isn't loaded yet, remember it. */
  function seek(ms: number, play = playing) {
    const to = Math.max(0, total > 0 ? Math.min(ms, total) : ms);
    position = to;
    if (!audio || preparing || !audio.src || audio.readyState < 1) {
      pendingMs = to;
      wantPlay = wantPlay || play;
      return;
    }
    audio.currentTime = to / 1000;
    if (play) void audio.play().catch(() => {});
  }

  function toggle() {
    if (!audio) return;
    if (preparing) {
      wantPlay = !wantPlay;
      return;
    }
    if (audio.paused) void audio.play().catch(() => {});
    else audio.pause();
  }

  function skip(deltaMs: number) {
    seek(position + deltaMs);
    void save();
  }

  function seekInput(e: Event) {
    seek(Number((e.currentTarget as HTMLInputElement).value));
    void save();
  }

  function nextChapter() {
    const next = list[chapterIndex + 1];
    if (next) {
      seek(next.start_ms);
      void save();
    }
  }

  async function setFinished(finished: boolean) {
    if (!ep) return;
    const { data } = await api<BookProgress>(progressPath(), {
      method: 'PUT',
      body: JSON.stringify({ finished, client_updated_at: new Date().toISOString() }),
    });
    if (data && ep) {
      ep = { ...ep, progress: data };
      // Un-playing by hand starts the episode over (AB11): follow the
      // server, or the next save would put the old position back.
      if (!finished && data.position_ms !== Math.round(position)) {
        lastSavedPos = data.position_ms;
        seek(data.position_ms);
      }
    }
  }

  async function startOver() {
    if (!ep) return;
    await api(progressPath(), { method: 'DELETE' });
    lastSavedPos = 0;
    const { data } = await api<BookProgress>(progressPath());
    if (data && ep) ep = { ...ep, progress: data };
    seek(0);
  }

  async function download() {
    if (!ep) return;
    notice = null;
    const { status, error: err } = await api(`/api/v1/admin/podcast-episodes/${ep.id}/download`, {
      method: 'POST',
    });
    if (status === 202) {
      notice =
        'The download is queued. Until it is in, this episode keeps playing from the publisher; open it again afterwards to play Parlour’s copy.';
    } else if (status === 409) {
      notice = 'This episode is already downloaded. Open it again to play Parlour’s copy.';
    } else {
      notice = err ?? 'Could not queue the download.';
    }
  }

  /** Why the audio is a copy, in the profile's words (as AB10's book). */
  function fallbackNote(e: ListenerEpisode): string {
    const codec = (e.codec ?? 'this format').toUpperCase();
    if (e.plan?.action === 'Remux') {
      return `Playing this ${codec} audio repackaged as MP4 (not re-encoded): the web player's profile doesn't open ${(e.container ?? 'this').toUpperCase()} files directly.`;
    }
    const what =
      e.plan?.reason === 'ContainerUnsupported'
        ? `${codec} in a ${(e.container ?? 'this').toUpperCase()} file`
        : codec;
    return `Playing an AAC copy: ${what} isn't in the web player's profile.`;
  }

  function chapterEnd(i: number): number {
    return list[i + 1]?.start_ms ?? total;
  }

  function onLeave() {
    saveOnLeave();
    endSession(true);
  }

  onMount(() => {
    void load(page.params.id ?? '');
    window.addEventListener('pagehide', onLeave);
    return () => {
      destroyed = true;
      window.removeEventListener('pagehide', onLeave);
      onLeave();
      audio?.pause();
    };
  });
</script>

{#if missing}
  <h1>Not found</h1>
  <p class="dim">There is no such episode, or it isn't available to this profile.</p>
{:else if error}
  <p class="error" role="alert">{error}</p>
{:else if !ep}
  <p class="dim">Loading…</p>
{:else}
  <div class="head">
    <a class="cover" href={`/podcast/${ep.podcast_id}`} aria-label={`Open ${ep.show_title}`}>
      <ShowArt url={ep.artwork_url} title={ep.show_title} size="lg" />
    </a>
    <div class="info">
      <p class="show"><a href={`/podcast/${ep.podcast_id}`}>{ep.show_title}</a></p>
      <h1>{ep.title}</h1>
      <p class="dim facts">
        {[formatDate(ep.published_at), formatDurationMs(ep.length_ms), episodeMarks(ep)]
          .filter(Boolean)
          .join(' · ')}
      </p>
      <p class="status" data-testid="episode-state" data-state={status}>
        <EpisodeMark {status} fraction={episodeFraction(ep)} />
        {#if status === 'finished'}
          Played
        {:else if resumable && !started}
          Resumes at {formatDurationMs(ep.progress.position_ms)}
        {:else if status === 'new'}
          New
        {:else}
          In progress
        {/if}
      </p>
      {#if ep.delivery === 'enclosure'}
        <p class="dim where" data-testid="from-publisher">
          Not downloaded to Parlour: this episode plays straight from the publisher's server.
        </p>
      {/if}
      {#if stoppedByAdmin}
        <p class="error" role="alert">An administrator stopped this listening session.</p>
      {/if}
      {#if session.me?.user.is_admin && !ep.downloaded}
        <p><button class="btn small" type="button" onclick={download}>Download to Parlour</button></p>
      {/if}
      {#if notice}<p class="notice" role="status">{notice}</p>{/if}
    </div>
  </div>

  <section class="player" aria-label="Player">
    <audio
      bind:this={audio}
      preload="metadata"
      onloadedmetadata={onLoadedMetadata}
      ondurationchange={onDurationChange}
      ontimeupdate={onTimeUpdate}
      onplay={onPlay}
      onpause={onPause}
      onended={onEnded}
      onerror={onError}
    ></audio>
    <p class="now" data-testid="current-chapter">
      {chapterIndex >= 0 ? list[chapterIndex].title : ep.title}
    </p>
    {#if ep.delivery === 'aac'}
      <p class="dim conversion" role="status">
        {#if preparing}
          Converting this episode to AAC for this browser… {preparedSeconds > 0
            ? `${preparedSeconds} s`
            : ''} It is kept once made.
        {:else}
          {fallbackNote(ep)}
        {/if}
      </p>
    {/if}
    {#if playError}<p class="error" role="alert">{playError}</p>{/if}
    <div class="scrubber">
      <input
        class="scrub"
        type="range"
        min="0"
        max={total || 1}
        step="1000"
        value={position}
        onchange={seekInput}
        aria-label="Position in the episode"
      />
      {#if list.length > 0}
        <button
          class="btn small"
          type="button"
          onclick={nextChapter}
          disabled={chapterIndex + 1 >= list.length}
          aria-label="Next chapter">Next chapter</button
        >
      {/if}
    </div>
    <div class="times dim">
      <span data-testid="elapsed">{formatDurationMs(position) || '0:00'}</span>
      <span>-{formatDurationMs(Math.max(0, total - position)) || '0:00'}</span>
    </div>
    <div class="controls">
      <button class="btn" onclick={() => skip(-BACK_MS)} aria-label="Back 15 seconds">−15 s</button>
      <button class="btn primary" onclick={toggle} aria-busy={preparing} data-testid="play"
        >{preparing ? 'Preparing…' : playing ? 'Pause' : resumable && !started ? 'Resume' : 'Play'}</button
      >
      <button class="btn" onclick={() => skip(FORWARD_MS)} aria-label="Forward 30 seconds">+30 s</button>
    </div>
    <div class="controls secondary">
      {#if ep.progress.finished}
        <button class="btn" onclick={() => setFinished(false)}>Mark as unplayed</button>
      {:else}
        <button class="btn" onclick={() => setFinished(true)}>Mark as played</button>
      {/if}
      {#if ep.progress.updated_at}
        <button class="btn" onclick={startOver}>Start over</button>
      {/if}
    </div>
    {#if saveError}<p class="error" role="alert">{saveError}</p>{/if}
  </section>

  {#if list.length > 0}
    <section aria-label="Chapters">
      <h2>Chapters</h2>
      <ol class="chapters">
        {#each list as c, i (c.start_ms)}
          {@const end = chapterEnd(i)}
          {@const done = end > 0 && position >= end}
          {@const current = i === chapterIndex}
          <li class:current class:done>
            <button
              onclick={() => {
                seek(c.start_ms, true);
                void save();
              }}
              aria-current={current ? 'true' : undefined}
            >
              {#if current && end > c.start_ms}
                <span
                  class="fill"
                  style={`width: ${(((position - c.start_ms) / (end - c.start_ms)) * 100).toFixed(1)}%`}
                ></span>
              {/if}
              <span class="check" aria-label={done ? 'Played' : undefined}>{done ? '✓' : ''}</span>
              <span class="ctitle">{c.title}</span>
              <span class="dim">
                {current
                  ? `−${formatDurationMs(Math.max(0, end - position)) || '0:00'}`
                  : formatDurationMs(c.start_ms) || '0:00'}
              </span>
            </button>
          </li>
        {/each}
      </ol>
    </section>
  {:else if chapters?.error}
    <p class="dim small">{chapters.error}</p>
  {/if}

  {#if notes}
    <section aria-label="Show notes">
      <h2>Show notes</h2>
      <!-- feedHtml rebuilt this from an allowlist: no scripts, no images,
           no attributes but a checked link address. -->
      <div class="notes" data-testid="show-notes">{@html notes}</div>
    </section>
  {/if}
{/if}

<style>
  .head {
    display: grid;
    grid-template-columns: minmax(120px, 200px) 1fr;
    gap: 1.6rem;
    margin: 1rem 0 1.4rem;
  }
  @media (max-width: 640px) {
    .head {
      grid-template-columns: 1fr;
    }
    .cover {
      max-width: 200px;
    }
  }
  .cover {
    display: block;
  }
  .show {
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-size: 0.85rem;
    margin: 0 0 0.3rem;
  }
  .show a {
    color: var(--text-dim);
    text-decoration: none;
  }
  .show a:hover {
    color: var(--accent);
  }
  h1 {
    margin: 0 0 0.4rem;
  }
  .info p {
    margin: 0.25rem 0;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--accent);
    font-weight: 600;
  }
  .where {
    font-size: 0.9rem;
  }
  .notice {
    color: var(--accent);
    font-size: 0.9rem;
  }
  .player {
    background: var(--bg-card);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 1rem 1.2rem;
    margin-bottom: 1.6rem;
    max-width: 720px;
  }
  .now {
    font-weight: 600;
    margin: 0 0 0.6rem;
  }
  .conversion {
    font-size: 0.85rem;
    margin: -0.3rem 0 0.6rem;
  }
  .scrubber {
    display: flex;
    gap: 0.6rem;
    align-items: center;
  }
  .scrub {
    flex: 1;
    accent-color: var(--accent);
  }
  .times {
    display: flex;
    justify-content: space-between;
    font-size: 0.82rem;
    font-variant-numeric: tabular-nums;
  }
  .controls {
    display: flex;
    gap: 0.6rem;
    justify-content: center;
    margin-top: 0.8rem;
    flex-wrap: wrap;
  }
  .controls.secondary {
    justify-content: flex-start;
  }
  h2 {
    font-size: 1.15rem;
  }
  .small {
    font-size: 0.85rem;
  }
  .chapters {
    list-style: none;
    padding: 0;
    margin: 0 0 1.6rem;
    max-width: 720px;
  }
  .chapters button {
    position: relative;
    display: grid;
    grid-template-columns: 1.2rem 1fr auto;
    gap: 0.6rem;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-bottom: 1px solid var(--line);
    padding: 0.55rem 0.4rem;
    color: inherit;
    cursor: pointer;
    font: inherit;
    overflow: hidden;
  }
  .chapters button > span:not(.fill) {
    position: relative;
  }
  .fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    pointer-events: none;
  }
  .chapters button:hover,
  .chapters button:focus-visible {
    background: var(--bg-raise);
  }
  .chapters .current button {
    color: var(--accent);
    font-weight: 600;
  }
  .chapters .done .ctitle {
    color: var(--text-dim);
  }
  .check {
    color: var(--accent);
  }
  .notes {
    max-width: 65ch;
    line-height: 1.5;
    overflow-wrap: anywhere;
  }
  .notes :global(p) {
    margin: 0 0 0.6rem;
  }
  .notes :global(a) {
    color: var(--accent);
  }
</style>
