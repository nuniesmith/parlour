<script lang="ts">
  import { episodeLabel, itemHref, posterUrl, type RailItem } from './types';

  let {
    item,
    progress = null,
  }: { item: RailItem; progress?: number | null } = $props();

  // An episode reads as its show first (UI11, like the Now Playing
  // card): the series title on top, "S1:E2 · Title" under it, and
  // the series' poster. Rows without `series_title` (search, the
  // library grid) keep the item's own title.
  const isEpisode = $derived(item.kind === 'episode');
  const heading = $derived(item.series_title ?? item.title);
  const epLine = $derived(
    isEpisode ? episodeLabel(item.season_number, item.episode_number, item.series_title ? item.title : null) : null,
  );
  // Rail rows say which item's poster to use; everything else
  // uses the item's own when it has one.
  const artId = $derived(
    item.poster_item_id !== undefined ? item.poster_item_id : item.has_poster ? item.id : null,
  );
  const label = $derived([heading, epLine].filter(Boolean).join(', '));
</script>

<a class="poster" href={itemHref(item)} aria-label={label}>
  <span class="frame">
    {#if artId}
      <img src={posterUrl(artId)} alt="" loading="lazy" />
    {:else}
      <span class="tile" aria-hidden="true">
        <span class="initial">{heading.charAt(0).toUpperCase()}</span>
        <span class="tile-title">{heading}</span>
      </span>
    {/if}
    {#if progress !== null}
      <span class="progress"><span style={`width: ${Math.round(progress * 100)}%`}></span></span>
    {/if}
    {#if !item.available}
      <span class="missing">Unavailable</span>
    {/if}
  </span>
  <span class="caption">
    <span class="title">{heading}</span>
    {#if epLine}
      <span class="meta ep">{epLine}</span>
    {:else}
      <span class="meta">
        {#if item.year}{item.year}{/if}
        {#if item.content_rating} · {item.content_rating}{/if}
        {#if item.vote_average} · ★ {item.vote_average.toFixed(1)}{/if}
      </span>
    {/if}
  </span>
</a>

<style>
  .poster {
    display: grid;
    gap: 0.45rem;
  }
  .frame {
    position: relative;
    display: block;
    aspect-ratio: var(--poster-ratio);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg-card);
    border: 1px solid var(--line);
  }
  .poster:hover .frame,
  .poster:focus-visible .frame {
    border-color: var(--accent);
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .tile {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
    align-content: center;
    gap: 0.5rem;
    padding: 0.6rem;
    text-align: center;
    background: linear-gradient(160deg, #2a3350, #171c2a);
  }
  .initial {
    font-size: 2.6rem;
    font-weight: 700;
    color: var(--accent);
  }
  .tile-title {
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .progress {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 5px;
    background: rgba(0, 0, 0, 0.55);
  }
  .progress > span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .missing {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
    background: rgba(0, 0, 0, 0.7);
    color: var(--danger);
    font-size: 0.7rem;
    padding: 0.15rem 0.45rem;
    border-radius: 999px;
  }
  .caption {
    display: grid;
    gap: 0.1rem;
    min-width: 0;
  }
  .title {
    font-size: 0.92rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta {
    font-size: 0.78rem;
    color: var(--text-dim);
  }
  .ep {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
