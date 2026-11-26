<script lang="ts">
  // AB6 (D30): an admin matches one book by hand against Google Books
  // or Open Library, and chooses whether the result fills only what
  // the book lacks or replaces its values. Mirrors Audiobookshelf's
  // per-book Match; its code and wording are not used.
  import { api } from '$lib/api';
  import {
    BOOK_PROVIDERS,
    type Book,
    type BookMatchReport,
    type BookSearchResult,
    type MatchMode,
  } from '$lib/types';

  let { book, onchanged }: { book: Book; onchanged: () => void } = $props();

  let open = $state(false);
  let provider = $state('google');
  let title = $state('');
  let author = $state('');
  let mode = $state<MatchMode>('fill_gaps');
  let cover = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let result = $state<BookSearchResult | null>(null);

  function start() {
    open = true;
    title = book.title;
    author = book.authors[0] ?? '';
    result = null;
    error = null;
    notice = null;
  }

  async function search(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = null;
    notice = null;
    const q = new URLSearchParams({ provider, title: title.trim(), author: author.trim() });
    const { data, error: err } = await api<BookSearchResult>(
      `/api/v1/admin/audiobooks/${book.id}/match/search?${q}`,
    );
    busy = false;
    result = data;
    if (!data) error = err ?? 'The search failed.';
  }

  async function apply(providerId: string) {
    busy = true;
    error = null;
    notice = null;
    const { data, error: err } = await api<BookMatchReport>(
      `/api/v1/admin/audiobooks/${book.id}/match`,
      {
        method: 'POST',
        body: JSON.stringify({ provider, provider_id: providerId, mode, cover }),
      },
    );
    busy = false;
    if (!data) {
      error = err ?? 'The match could not be applied.';
      return;
    }
    const parts = [data.changed ? 'Matched.' : 'Matched; nothing about the book changed.'];
    if (data.cover === 'applied') parts.push('The cover was replaced.');
    if (data.cover === 'failed') parts.push("The provider's cover could not be fetched.");
    if (data.sidecar_written) parts.push('book.parlour.json was written next to the book.');
    if (data.cover_written) parts.push('cover.jpg was written next to the book.');
    for (const note of data.left_alone) parts.push(`Left alone: ${note}`);
    for (const e of data.errors) parts.push(`Not written: ${e}`);
    notice = parts.join(' ');
    result = null;
    open = false;
    onchanged();
  }

  async function forget() {
    if (
      !confirm(
        'Forget this match? The next scan builds the book from its own files again. ' +
          'Files written next to the book stay until you delete them.',
      )
    )
      return;
    const { status, error: err } = await api(`/api/v1/admin/audiobooks/${book.id}/match`, {
      method: 'DELETE',
    });
    if (status === 200) {
      notice = 'Match forgotten. It takes effect at the next scan of the library.';
      onchanged();
    } else error = err ?? 'Could not forget the match.';
  }

  const providerLabel = (id: string) => BOOK_PROVIDERS.find((p) => p.id === id)?.label ?? id;
</script>

<section class="match" aria-label="Match this book">
  <div class="head">
    <h2>Metadata</h2>
    {#if book.match}
      <span class="dim">
        Matched with {providerLabel(book.match.provider)}
        ({book.match.mode === 'overwrite' ? 'replaced values' : 'filled gaps'}),
        {new Date(book.match.matched_at).toLocaleDateString()}
      </span>
    {:else}
      <span class="dim">From the book's own files</span>
    {/if}
    {#if !open}
      <button class="btn small" onclick={start}>Match…</button>
    {/if}
    {#if book.match}
      <button class="btn small" onclick={forget}>Forget match</button>
    {/if}
  </div>
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if open}
    <form class="search" onsubmit={search}>
      <div class="field">
        <label for="mprov">Provider</label>
        <select id="mprov" bind:value={provider}>
          {#each BOOK_PROVIDERS as p (p.id)}<option value={p.id}>{p.label}</option>{/each}
        </select>
      </div>
      <div class="field">
        <label for="mtitle">Title</label>
        <input id="mtitle" bind:value={title} required />
      </div>
      <div class="field">
        <label for="mauthor">Author</label>
        <input id="mauthor" bind:value={author} />
      </div>
      <div class="buttons">
        <button class="btn primary" type="submit" disabled={busy}>Search</button>
        <button class="btn" type="button" onclick={() => (open = false)}>Cancel</button>
      </div>
    </form>

    <fieldset class="options">
      <legend>When applying a result</legend>
      <label
        ><input type="radio" bind:group={mode} value="fill_gaps" /> Fill in only what the book
        is missing</label
      >
      <label
        ><input type="radio" bind:group={mode} value="overwrite" /> Replace the book's values
        with the provider's</label
      >
      <label
        ><input type="checkbox" bind:checked={cover} /> Use the provider's cover
        {mode === 'fill_gaps' ? '(only if the book has none)' : ''}</label
      >
    </fieldset>

    {#if result}
      {#if result.results.length === 0}
        <p class="dim">{providerLabel(result.provider)} found nothing for that.</p>
      {:else}
        <ul class="results">
          {#each result.results as r (r.provider_id)}
            <li>
              <div class="thumb">
                {#if r.cover_url}<img src={r.cover_url} alt="" loading="lazy" />{/if}
              </div>
              <div class="what">
                <strong>{r.title}</strong>{#if r.subtitle}: {r.subtitle}{/if}
                <div class="dim">
                  {[r.authors.join(', '), r.year, r.publisher].filter(Boolean).join(' · ')}
                </div>
                {#if r.description}<p class="desc">{r.description}</p>{/if}
              </div>
              <button class="btn small" disabled={busy} onclick={() => apply(r.provider_id)}
                >Use this</button
              >
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  {/if}
</section>

<style>
  .match {
    max-width: 720px;
    margin-bottom: 1.6rem;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
  }
  .head h2 {
    font-size: 1.15rem;
    margin: 0;
  }
  .search {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 0.6rem;
    align-items: end;
    margin-top: 0.8rem;
  }
  .buttons {
    display: flex;
    gap: 0.5rem;
  }
  .options {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    margin: 0.8rem 0;
    padding: 0.5rem 0.8rem;
    display: grid;
    gap: 0.3rem;
  }
  .results {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.5rem;
  }
  .results li {
    display: grid;
    grid-template-columns: 56px 1fr auto;
    gap: 0.75rem;
    align-items: start;
    border-bottom: 1px solid var(--line);
    padding-bottom: 0.5rem;
  }
  .thumb {
    width: 56px;
    aspect-ratio: 2 / 3;
    background: var(--bg-raise);
    border-radius: 4px;
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .desc {
    margin: 0.3rem 0 0;
    font-size: 0.88rem;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
</style>
