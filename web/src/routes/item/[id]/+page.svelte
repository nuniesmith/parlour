<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import Poster from '$lib/Poster.svelte';
  import Rail from '$lib/Rail.svelte';
  import {
    backdropUrl,
    formatBytes,
    formatDurationMs,
    type Item,
    type ItemDetail,
    type UserState,
  } from '$lib/types';

  let detail = $state<ItemDetail | null>(null);
  let error = $state<string | null>(null);
  let seasons = $state<Record<string, Item[]>>({});

  async function mark(body: { played?: boolean; favorite?: boolean }) {
    if (!detail) return;
    const { data } = await api<UserState>(`/api/v1/items/${detail.id}/mark`, {
      method: 'POST',
      body: JSON.stringify(body),
    });
    if (data && detail) detail = { ...detail, user_state: data };
  }
  let openSeason = $state<string | null>(null);

  async function load(id: string) {
    detail = null;
    seasons = {};
    openSeason = null;
    const { data, error: err } = await api<ItemDetail>(`/api/v1/items/${id}`);
    if (data) detail = data;
    else error = err ?? 'Could not load that item.';
  }

  onMount(() => load(page.params.id ?? ''));

  $effect(() => {
    const id = page.params.id;
    if (id && detail && detail.id !== id) void load(id);
  });

  const hasBackdrop = $derived(
    detail?.images.some(
      (i) => i.kind === 'backdrop' && i.path !== '' && !i.path.startsWith('pending:'),
    ) ?? false,
  );

  async function toggleSeason(season: Item) {
    if (openSeason === season.id) {
      openSeason = null;
      return;
    }
    openSeason = season.id;
    if (!seasons[season.id]) {
      const { data } = await api<ItemDetail>(`/api/v1/items/${season.id}`);
      if (data) seasons = { ...seasons, [season.id]: data.children };
    }
  }
</script>

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if !detail}
  <p class="dim">Loading…</p>
{:else}
  <div class="hero">
    {#if hasBackdrop}
      <img class="backdrop" src={backdropUrl(detail)} alt="" />
    {/if}
    <div class="hero-inner">
      <div class="poster-wrap"><Poster item={detail} /></div>
      <div class="info">
        <h1>{detail.title}</h1>
        <p class="meta">
          {#if detail.year}<span>{detail.year}</span>{/if}
          {#if detail.content_rating}<span class="chip">{detail.content_rating}</span>{/if}
          {#if detail.vote_average}<span>★ {detail.vote_average.toFixed(1)}</span>{/if}
          {#if detail.metadata_status !== 'matched'}<span class="chip">{detail.metadata_status}</span>{/if}
        </p>
        {#if detail.genres.length}
          <p class="genres">
            {#each detail.genres as g (g)}<span class="chip">{g}</span>{/each}
          </p>
        {/if}
        {#if detail.overview}<p class="overview">{detail.overview}</p>{/if}
        {#if detail.files.length > 0}
          <p class="actions">
            <a class="btn primary big" href={`/watch/${detail.id}`}>▶ Play</a>
            <button
              class="btn"
              onclick={() => mark({ played: !(detail?.user_state?.played ?? false) })}
            >
              {detail.user_state?.played ? 'Mark unplayed' : 'Mark played'}
            </button>
            <button
              class="btn"
              aria-pressed={detail.user_state?.is_favorite ?? false}
              onclick={() => mark({ favorite: !(detail?.user_state?.is_favorite ?? false) })}
            >
              {detail.user_state?.is_favorite ? '★ Favourite' : '☆ Favourite'}
            </button>
          </p>
        {:else if detail.kind === 'series'}
          <p class="dim">Pick an episode below to play.</p>
        {/if}
      </div>
    </div>
  </div>

  {#if detail.cast.length}
    <h2>Cast</h2>
    <div class="cast">
      {#each detail.cast as member (member.name)}
        <div class="person">
          <strong>{member.name}</strong>
          {#if member.character_name}<span class="dim">as {member.character_name}</span>{/if}
        </div>
      {/each}
    </div>
  {/if}

  {#if detail.kind === 'series' && detail.children.length}
    <h2>Episodes</h2>
    {#each detail.children as season (season.id)}
      <div class="season">
        <button class="season-head" onclick={() => toggleSeason(season)}>
          {season.title}
          <span class="dim">{openSeason === season.id ? '▾' : '▸'}</span>
        </button>
        {#if openSeason === season.id}
          {#if seasons[season.id]}
            <ul class="episodes">
              {#each seasons[season.id] as ep (ep.id)}
                <li>
                  <a href={`/item/${ep.id}`}>
                    <span class="ep-num">{ep.episode_number ?? ''}</span>
                    <span class="ep-title">{ep.title}</span>
                    {#if !ep.available}<span class="error">Unavailable</span>{/if}
                  </a>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="dim">Loading episodes…</p>
          {/if}
        {/if}
      </div>
    {/each}
  {/if}

  {#if detail.files.length}
    <h2>Files</h2>
    {#each detail.files as file (file.id)}
      <div class="file">
        <p class="file-name">{file.relative_path}</p>
        <p class="dim">
          {file.container ?? 'unknown container'} · {formatBytes(file.size_bytes)} · {formatDurationMs(
            file.duration_ms,
          )}
          {#if file.probe_error}<span class="error">probe failed: {file.probe_error}</span>{/if}
        </p>
        <p class="dim streams">
          {#each file.streams as s (s.stream_index)}
            <span class="chip">
              {s.kind}
              {s.codec ?? ''}{#if s.width} {s.width}×{s.height}{/if}{#if s.hdr_format}
                {s.hdr_format}{/if}{#if s.language} {s.language}{/if}
            </span>
          {/each}
          {#each file.subtitles as sub (sub.path ?? sub.stream_index)}
            <span class="chip">
              sub {sub.language ?? '?'}{#if sub.is_forced} forced{/if}{#if sub.is_sdh} SDH{/if}
              ({sub.source})
            </span>
          {/each}
        </p>
      </div>
    {/each}
  {/if}

  <Rail title="More like this" items={detail.similar} />
{/if}

<style>
  .hero {
    position: relative;
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg-raise);
    border: 1px solid var(--line);
  }
  .backdrop {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    opacity: 0.32;
  }
  .hero-inner {
    position: relative;
    display: flex;
    gap: 1.6rem;
    padding: 1.6rem;
    flex-wrap: wrap;
  }
  .poster-wrap {
    width: 200px;
    flex: 0 0 auto;
  }
  .info {
    flex: 1 1 320px;
    min-width: 0;
  }
  .info h1 {
    margin: 0.2rem 0 0.6rem;
    font-size: 2rem;
  }
  .meta {
    display: flex;
    gap: 0.8rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .genres {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .overview {
    max-width: 65ch;
  }
  .big {
    font-size: 1.1rem;
    padding: 0.7rem 1.6rem;
  }
  .actions {
    display: flex;
    gap: 0.6rem;
    flex-wrap: wrap;
    align-items: center;
  }
  .cast {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 0.7rem;
  }
  .person {
    display: grid;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.6rem 0.8rem;
  }
  .season {
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    margin-bottom: 0.6rem;
    background: var(--bg-raise);
  }
  .season-head {
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    padding: 0.8rem 1rem;
    cursor: pointer;
    font-size: 1rem;
    display: flex;
    justify-content: space-between;
  }
  .episodes {
    list-style: none;
    margin: 0;
    padding: 0 1rem 0.8rem;
  }
  .episodes a {
    display: flex;
    gap: 0.8rem;
    padding: 0.45rem 0.2rem;
    border-top: 1px solid var(--line);
  }
  .episodes a:hover {
    color: var(--accent);
  }
  .ep-num {
    color: var(--text-dim);
    min-width: 1.6rem;
  }
  .file {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.8rem 1rem;
    margin-bottom: 0.7rem;
  }
  .file-name {
    margin: 0 0 0.3rem;
    font-family: ui-monospace, monospace;
    font-size: 0.9rem;
    word-break: break-all;
  }
  .file p {
    margin: 0.2rem 0;
  }
  .streams {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  @media (max-width: 640px) {
    .poster-wrap {
      width: 130px;
    }
    .info h1 {
      font-size: 1.5rem;
    }
  }
</style>
