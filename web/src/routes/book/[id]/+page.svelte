<script lang="ts">
  // A book and its player (Phase 10, AB5). One <audio> plays the
  // book track after track; every position here is book-global
  // (ms from the start of track 0), the same number the server
  // stores. Progress is written every 15 s while playing, on pause,
  // on a seek, and when the page goes away — but only once this
  // page has actually played, so opening a book changes nothing.
  // The last track ending marks the book finished.
  // AB11 (as Audiobookshelf): the server also finishes a book whose
  // position leaves less than the library's threshold, un-finishing by
  // hand puts the position back to 0, and every write carries this
  // browser's own time (client_updated_at).
  // AB6: admins can match the book by hand (BookMatch).
  // AB7: its authors and series link to their pages.
  import { onMount, tick } from 'svelte';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import BookMatch from '$lib/BookMatch.svelte';
  import { session } from '$lib/session.svelte';
  import {
    chapterAt,
    formatDurationMs,
    locateInBook,
    posterUrl,
    type BookDetail,
    type BookProgress,
  } from '$lib/types';

  const SAVE_EVERY_MS = 15_000;
  const SKIP_MS = 30_000;

  let book = $state<BookDetail | null>(null);
  let error = $state<string | null>(null);
  let audio = $state<HTMLAudioElement | null>(null);
  let track = $state(0);
  let position = $state(0);
  let playing = $state(false);
  let started = $state(false);
  let saveError = $state<string | null>(null);
  /** Bumped after a match, so the cover image is fetched again. */
  let coverKey = $state(0);

  let pendingOffsetMs: number | null = null;
  let lastSavedAt = 0;
  let lastSavedPos = -1;

  const total = $derived(book?.duration_ms ?? 0);
  const chapterIndex = $derived(book ? chapterAt(book.chapters, position) : -1);
  const resumable = $derived(
    !!book && !book.progress.finished && book.progress.position_ms > 0,
  );

  async function load(id: string) {
    const { data, error: err } = await api<BookDetail>(`/api/v1/audiobooks/${id}`);
    if (!data) {
      error = err ?? 'Could not load that book.';
      return;
    }
    book = data;
    const start = data.progress.finished ? 0 : data.progress.position_ms;
    position = start;
    await tick(); // the <audio> exists once the book has rendered
    if (data.tracks.length > 0) loadAt(start, false);
  }

  /** Point the player at a book-global position. */
  function loadAt(globalMs: number, play: boolean) {
    if (!book || !audio || book.tracks.length === 0) return;
    const loc = locateInBook(book.tracks, Math.max(0, Math.min(globalMs, total)));
    position = book.tracks[loc.track].start_ms + loc.offsetMs;
    if (loc.track !== track || !audio.src) {
      track = loc.track;
      pendingOffsetMs = loc.offsetMs;
      audio.src = book.tracks[loc.track].stream_url;
      audio.load();
    } else {
      audio.currentTime = loc.offsetMs / 1000;
    }
    if (play) void audio.play().catch(() => {});
  }

  function onLoadedMetadata() {
    if (audio && pendingOffsetMs !== null) {
      audio.currentTime = pendingOffsetMs / 1000;
      pendingOffsetMs = null;
    }
  }

  function onTimeUpdate() {
    if (!book || !audio || pendingOffsetMs !== null) return;
    position = book.tracks[track].start_ms + Math.round(audio.currentTime * 1000);
    if (playing && Date.now() - lastSavedAt >= SAVE_EVERY_MS) void save();
  }

  function onEnded() {
    if (!book) return;
    if (track + 1 < book.tracks.length) {
      loadAt(book.tracks[track + 1].start_ms, true);
    } else {
      playing = false;
      position = total;
      void save({ finished: true });
    }
  }

  async function save(extra: { finished?: boolean } = {}) {
    if (!book || (!started && extra.finished === undefined)) return;
    const pos = Math.round(position);
    if (pos === lastSavedPos && extra.finished === undefined) return;
    lastSavedAt = Date.now();
    lastSavedPos = pos;
    const { data, error: err } = await api<BookProgress>(`/api/v1/audiobooks/${book.id}/progress`, {
      method: 'PUT',
      body: JSON.stringify({
        position_ms: pos,
        ...extra,
        client_updated_at: new Date().toISOString(),
      }),
    });
    if (data && book) {
      book = { ...book, progress: data };
      saveError = null;
    } else {
      saveError = err ?? 'Could not save your place.';
    }
  }

  /** Last write on the way out: keepalive so it survives the page. */
  function saveOnLeave() {
    if (!book || !started) return;
    const pos = Math.round(position);
    if (pos === lastSavedPos) return;
    lastSavedPos = pos;
    void fetch(`/api/v1/audiobooks/${book.id}/progress`, {
      method: 'PUT',
      credentials: 'same-origin',
      keepalive: true,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ position_ms: pos, client_updated_at: new Date().toISOString() }),
    }).catch(() => {});
  }

  function toggle() {
    if (!audio) return;
    if (audio.paused) void audio.play().catch(() => {});
    else audio.pause();
  }

  function skip(deltaMs: number) {
    loadAt(position + deltaMs, playing);
    void save();
  }

  function seekInput(e: Event) {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    loadAt(v, playing);
  }

  /** After a match: the book's metadata and cover, not the player. */
  async function reloadMetadata() {
    if (!book) return;
    const { data } = await api<BookDetail>(`/api/v1/audiobooks/${book.id}`);
    if (data && book) {
      book = { ...data, progress: book.progress };
      coverKey = Date.now();
    }
  }

  async function setFinished(finished: boolean) {
    if (!book) return;
    const { data } = await api<BookProgress>(`/api/v1/audiobooks/${book.id}/progress`, {
      method: 'PUT',
      body: JSON.stringify({ finished, client_updated_at: new Date().toISOString() }),
    });
    if (data && book) {
      book = { ...book, progress: data };
      // Un-finishing by hand starts the book over (AB11): follow the
      // server, or the next save would put the old position back.
      if (!finished && data.position_ms !== Math.round(position)) {
        lastSavedPos = data.position_ms;
        loadAt(data.position_ms, playing);
      }
    }
  }

  async function startOver() {
    if (!book) return;
    await api(`/api/v1/audiobooks/${book.id}/progress`, { method: 'DELETE' });
    lastSavedPos = 0;
    const { data } = await api<BookProgress>(`/api/v1/audiobooks/${book.id}/progress`);
    if (data && book) book = { ...book, progress: data };
    loadAt(0, playing);
  }

  onMount(() => {
    void load(page.params.id ?? '');
    window.addEventListener('pagehide', saveOnLeave);
    return () => {
      window.removeEventListener('pagehide', saveOnLeave);
      saveOnLeave();
    };
  });
</script>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if !book}
  <p class="dim">Loading…</p>
{:else}
  <div class="head">
    <div class="cover">
      {#if book.has_cover}
        <img
          src={coverKey ? `${posterUrl(book.id, 'original')}&r=${coverKey}` : posterUrl(book.id, 'original')}
          alt=""
        />
      {:else}
        <span class="tile" aria-hidden="true">{book.title.charAt(0).toUpperCase()}</span>
      {/if}
    </div>
    <div class="info">
      <h1>{book.title}</h1>
      {#if book.subtitle}<p class="subtitle">{book.subtitle}</p>{/if}
      {#if book.author_refs.length}
        <p class="by">
          by
          {#each book.author_refs as a, i (a.id)}{i > 0 ? ', ' : ''}<a
              href={`/author/${a.id}?library_id=${book.library_id}`}>{a.name}</a
            >{/each}
        </p>
      {:else if book.authors.length}
        <p class="by">by {book.authors.join(', ')}</p>
      {/if}
      {#if book.series.length}
        <p class="series">
          {#each book.series as s, i (s.id)}{i > 0 ? ' · ' : ''}{s.sequence
              ? `Book ${s.sequence} of `
              : ''}<a href={`/series/${s.id}`}>{s.name}</a>{/each}
        </p>
      {/if}
      {#if book.narrators.length}<p class="dim">Read by {book.narrators.join(', ')}</p>{/if}
      <p class="dim">
        {[book.year, formatDurationMs(book.duration_ms), `${book.chapter_count} chapters`]
          .filter(Boolean)
          .join(' · ')}
      </p>
      {#if book.progress.finished}
        <p class="status">Finished</p>
      {:else if resumable && !started}
        <p class="status">Resumes at {formatDurationMs(book.progress.position_ms)}</p>
      {/if}
      {#if book.overview}<p class="overview">{book.overview}</p>{/if}
      {#if book.publisher || book.language || book.isbn}
        <p class="dim facts">
          {[book.publisher, book.language, book.isbn ? `ISBN ${book.isbn}` : null]
            .filter(Boolean)
            .join(' · ')}
        </p>
      {/if}
    </div>
  </div>

  {#if session.me?.user.is_admin}
    <BookMatch {book} onchanged={reloadMetadata} />
  {/if}

  {#if !book.available || book.tracks.length === 0}
    <p class="error" role="alert">This book's files are not available right now.</p>
  {:else}
    <section class="player" aria-label="Player">
      <audio
        bind:this={audio}
        preload="metadata"
        onloadedmetadata={onLoadedMetadata}
        ontimeupdate={onTimeUpdate}
        onplay={() => {
          playing = true;
          started = true;
        }}
        onpause={() => {
          playing = false;
          void save();
        }}
        onended={onEnded}
      ></audio>
      <p class="now">
        {chapterIndex >= 0 ? book.chapters[chapterIndex].title : book.tracks[track].title}
      </p>
      <input
        class="scrub"
        type="range"
        min="0"
        max={total}
        step="1000"
        value={position}
        onchange={seekInput}
        aria-label="Position in the book"
      />
      <div class="times dim">
        <span>{formatDurationMs(position) || '0:00'}</span>
        <span>-{formatDurationMs(Math.max(0, total - position)) || '0:00'}</span>
      </div>
      <div class="controls">
        <button class="btn" onclick={() => skip(-SKIP_MS)} aria-label="Back 30 seconds">−30 s</button>
        <button class="btn primary" onclick={toggle}>{playing ? 'Pause' : resumable && !started ? 'Resume' : 'Play'}</button>
        <button class="btn" onclick={() => skip(SKIP_MS)} aria-label="Forward 30 seconds">+30 s</button>
      </div>
      <div class="controls secondary">
        {#if book.progress.finished}
          <button class="btn" onclick={() => setFinished(false)}>Mark as not finished</button>
        {:else}
          <button class="btn" onclick={() => setFinished(true)}>Mark as finished</button>
        {/if}
        {#if book.progress.updated_at}
          <button class="btn" onclick={startOver}>Start over</button>
        {/if}
      </div>
      {#if saveError}<p class="error" role="alert">{saveError}</p>{/if}
    </section>

    <section aria-label="Chapters">
      <h2>Chapters</h2>
      <ol class="chapters">
        {#each book.chapters as c, i (c.index)}
          <li class:current={i === chapterIndex}>
            <button
              onclick={() => {
                loadAt(c.start_ms, true);
                void save();
              }}
              aria-current={i === chapterIndex ? 'true' : undefined}
            >
              <span>{c.title}</span>
              <span class="dim">{formatDurationMs(c.start_ms) || '0:00'}</span>
            </button>
          </li>
        {/each}
      </ol>
    </section>
  {/if}
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
  .cover {
    aspect-ratio: 1;
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg-raise);
    border: 1px solid var(--line);
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .tile {
    display: grid;
    place-items: center;
    height: 100%;
    font-size: 3rem;
    color: var(--text-dim);
  }
  h1 {
    margin: 0 0 0.4rem;
  }
  .info p {
    margin: 0.2rem 0;
  }
  .by {
    font-size: 1.05rem;
  }
  .by a,
  .series a {
    color: inherit;
  }
  .series {
    color: var(--text-dim);
  }
  .subtitle {
    font-size: 1.05rem;
    color: var(--text-dim);
    margin-top: -0.2rem !important;
  }
  .facts {
    font-size: 0.85rem;
  }
  .status {
    color: var(--accent);
    font-weight: 600;
  }
  .overview {
    margin-top: 0.8rem !important;
    max-width: 60ch;
    line-height: 1.5;
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
  .scrub {
    width: 100%;
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
  .chapters {
    list-style: none;
    padding: 0;
    margin: 0;
    max-width: 720px;
  }
  .chapters button {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-bottom: 1px solid var(--line);
    padding: 0.55rem 0.4rem;
    color: inherit;
    cursor: pointer;
    font: inherit;
  }
  .chapters button:hover,
  .chapters button:focus-visible {
    background: var(--bg-raise);
  }
  .chapters .current button {
    color: var(--accent);
    font-weight: 600;
  }
</style>
