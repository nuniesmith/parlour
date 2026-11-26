<script lang="ts">
  // One of Audiobookshelf's book shelves (AB8, D29), as a sideways row:
  // Continue listening, Continue series, Recently added, Recent series,
  // Discover, Listen again, Newest authors. Home shows them across the
  // profile's audiobooks libraries; an audiobooks library's Home view
  // shows them for that library alone.
  //
  // A Continue listening card carries the book's percent and time left,
  // and a menu to hide it from the shelf (AB11's flag, which the next
  // change of position clears) or mark it finished.
  import { api } from './api';
  import { RAIL_LABELS } from './preferences';
  import {
    posterUrl,
    sequenceLabel,
    type AudioRail,
    type AudioShelves,
    type Book,
    type BookProgress,
  } from './types';

  let {
    id,
    shelves,
    inLibrary = false,
    libraryId = null,
    onchanged = () => {},
  }: {
    id: AudioRail;
    shelves: AudioShelves;
    /** On a library's own Home view the headings drop "audiobooks". */
    inLibrary?: boolean;
    /** Keeps an author page to this library (AB7's `?library_id=`). */
    libraryId?: string | null;
    /** After a hide or a finish, so the caller can load the shelves again. */
    onchanged?: () => void;
  } = $props();

  const LIBRARY_LABELS: Partial<Record<AudioRail, string>> = {
    recently_added_books: 'Recently added',
    discover_books: 'Discover',
  };
  const title = $derived((inLibrary && LIBRARY_LABELS[id]) || RAIL_LABELS[id] || id);

  const books = $derived.by((): { book: Book; line: string | null }[] => {
    switch (id) {
      case 'continue_listening':
        return shelves.continue_listening.map((book) => ({ book, line: timeLeft(book) }));
      case 'continue_series':
        return shelves.continue_series.map((e) => ({
          book: e.book,
          line: [sequenceLabel(e.book, e.series_id), e.series_name].filter(Boolean).join(' · '),
        }));
      case 'recently_added_books':
        return shelves.recently_added.map((book) => ({ book, line: authorsOf(book) }));
      case 'discover_books':
        return shelves.discover.map((book) => ({ book, line: authorsOf(book) }));
      case 'listen_again':
        return shelves.listen_again.map((book) => ({ book, line: authorsOf(book) }));
      default:
        return [];
    }
  });
  const count = $derived(
    id === 'recent_series'
      ? shelves.recent_series.length
      : id === 'newest_authors'
        ? shelves.newest_authors.length
        : books.length,
  );

  let menuFor = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let error = $state<string | null>(null);

  function authorsOf(b: Book): string | null {
    return b.authors.length ? b.authors.join(', ') : null;
  }

  function percent(b: Book): number {
    if (b.duration_ms <= 0) return 0;
    return Math.min(100, Math.floor((b.progress.position_ms / b.duration_ms) * 100));
  }

  function timeLeft(b: Book): string | null {
    if (b.duration_ms <= 0) return authorsOf(b);
    const left = Math.max(0, b.duration_ms - b.progress.position_ms);
    const mins = Math.max(1, Math.round(left / 60_000));
    const h = Math.floor(mins / 60);
    const m = mins % 60;
    return h > 0 ? `${h} h ${m} min left` : `${m} min left`;
  }

  function initials(name: string): string {
    return name
      .split(/\s+/)
      .filter(Boolean)
      .map((w) => w.charAt(0).toUpperCase())
      .slice(0, 2)
      .join('');
  }

  async function write(b: Book, body: Record<string, unknown>) {
    busy = b.id;
    error = null;
    const { data, error: err } = await api<BookProgress>(`/api/v1/audiobooks/${b.id}/progress`, {
      method: 'PUT',
      body: JSON.stringify({ ...body, client_updated_at: new Date().toISOString() }),
    });
    busy = null;
    menuFor = null;
    if (data) onchanged();
    else error = err ?? 'Could not change that book.';
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') menuFor = null;
  }
</script>

<svelte:window onkeydown={onKey} />

{#if count > 0}
  <section class="shelf" aria-label={title}>
    <h2>{title}</h2>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <ul class="track">
      {#if id === 'recent_series'}
        {#each shelves.recent_series as s (s.id)}
          <li class="cell">
            <a class="card" href={`/series/${s.id}`}>
              <span class="cover">
                {#if s.cover_book_id}
                  <img src={posterUrl(s.cover_book_id)} alt="" loading="lazy" />
                {:else}
                  <span class="letter" aria-hidden="true">{s.name.charAt(0).toUpperCase()}</span>
                {/if}
                {#if s.finished_count > 0 && s.finished_count === s.book_count}
                  <span class="badge left" title="Every book finished">✓</span>
                {/if}
                <span class="badge right" title={`${s.book_count} books`}>{s.book_count}</span>
              </span>
              <span class="name">{s.name}</span>
            </a>
          </li>
        {/each}
      {:else if id === 'newest_authors'}
        {#each shelves.newest_authors as a (a.id)}
          <li class="cell">
            <a
              class="card"
              href={libraryId ? `/author/${a.id}?library_id=${libraryId}` : `/author/${a.id}`}
            >
              <span class="cover round">
                <span class="letter" aria-hidden="true">{initials(a.name)}</span>
              </span>
              <span class="name center">{a.name}</span>
              <span class="dim small center">{a.book_count} {a.book_count === 1 ? 'book' : 'books'}</span>
            </a>
          </li>
        {/each}
      {:else}
        {#each books as { book: b, line } (b.id)}
          <li class="cell">
            <a class="card" href={`/book/${b.id}`}>
              <span class="cover">
                {#if b.has_cover}
                  <img src={posterUrl(b.id)} alt="" loading="lazy" />
                {:else}
                  <span class="letter" aria-hidden="true">{b.title.charAt(0).toUpperCase()}</span>
                {/if}
                {#if id === 'continue_listening'}
                  <span class="badge left">{percent(b)}%</span>
                  <span class="progress" aria-hidden="true"
                    ><span style={`width: ${percent(b)}%`}></span></span
                  >
                {:else if id === 'listen_again'}
                  <span class="badge left" title="Finished">✓</span>
                {/if}
              </span>
              <span class="name">{b.title}</span>
              {#if line}<span class="dim small">{line}</span>{/if}
            </a>
            {#if id === 'continue_listening'}
              <button
                class="more"
                type="button"
                aria-haspopup="menu"
                aria-expanded={menuFor === b.id}
                aria-label={`More for ${b.title}`}
                onclick={() => (menuFor = menuFor === b.id ? null : b.id)}>⋯</button
              >
              {#if menuFor === b.id}
                <div class="menu" role="menu">
                  <button
                    type="button"
                    role="menuitem"
                    disabled={busy === b.id}
                    onclick={() => write(b, { hidden: true })}>Hide from Continue listening</button
                  >
                  <button
                    type="button"
                    role="menuitem"
                    disabled={busy === b.id}
                    onclick={() => write(b, { finished: true })}>Mark as finished</button
                  >
                </div>
              {/if}
            {/if}
          </li>
        {/each}
      {/if}
    </ul>
    {#if id === 'continue_listening'}
      <p class="sr-only">
        Hiding a book takes it off this shelf until you listen to it again. Its place is kept.
      </p>
    {/if}
  </section>
{/if}

<style>
  .shelf {
    margin: 1.6rem 0;
  }
  h2 {
    font-size: 1.15rem;
    margin: 0 0 0.7rem;
  }
  .track {
    list-style: none;
    margin: 0;
    padding: 0 0 0.4rem;
    display: flex;
    gap: 1rem;
    overflow-x: auto;
    scroll-snap-type: x proximity;
  }
  .cell {
    position: relative;
    flex: 0 0 150px;
    scroll-snap-align: start;
  }
  .card {
    display: grid;
    gap: 0.2rem;
    color: inherit;
    text-decoration: none;
  }
  .cover {
    position: relative;
    display: block;
    aspect-ratio: 1;
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg-raise);
    border: 1px solid var(--line);
  }
  .cover.round {
    border-radius: 50%;
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .card:hover .cover,
  .card:focus-visible .cover {
    outline: 2px solid var(--accent);
  }
  .letter {
    display: grid;
    place-items: center;
    height: 100%;
    font-size: 2.2rem;
    color: var(--text-dim);
  }
  .badge {
    position: absolute;
    top: 0.4rem;
    font-size: 0.72rem;
    padding: 0.1rem 0.4rem;
    border-radius: 0.3rem;
  }
  .badge.left {
    left: 0.4rem;
    background: var(--accent);
    color: var(--accent-ink);
  }
  .badge.right {
    right: 0.4rem;
    background: rgba(0, 0, 0, 0.7);
    color: #fff;
  }
  .progress {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 4px;
    background: rgba(0, 0, 0, 0.5);
  }
  .progress span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .name {
    font-weight: 600;
    line-height: 1.25;
    margin-top: 0.3rem;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .center {
    text-align: center;
  }
  .small {
    font-size: 0.82rem;
  }
  .more {
    position: absolute;
    top: 0.35rem;
    right: 0.35rem;
    width: 1.9rem;
    height: 1.9rem;
    border-radius: 50%;
    border: none;
    background: rgba(0, 0, 0, 0.65);
    color: #fff;
    font-size: 1.1rem;
    line-height: 1;
    cursor: pointer;
  }
  .more:focus-visible {
    outline: 2px solid var(--accent);
  }
  .menu {
    position: absolute;
    top: 2.5rem;
    right: 0.35rem;
    z-index: 5;
    display: grid;
    min-width: 13rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.3rem;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
  }
  .menu button {
    text-align: left;
    background: none;
    border: none;
    color: var(--text);
    padding: 0.45rem 0.6rem;
    border-radius: 0.35rem;
    cursor: pointer;
  }
  .menu button:hover,
  .menu button:focus-visible {
    background: var(--bg-card);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  @media (max-width: 640px) {
    .cell {
      flex-basis: 112px;
    }
  }
</style>
