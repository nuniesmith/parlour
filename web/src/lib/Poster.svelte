<script lang="ts">
  import { posterUrl, type Item } from './types';

  let {
    item,
    progress = null,
  }: { item: Item; progress?: number | null } = $props();
</script>

<a class="poster" href={`/item/${item.id}`} aria-label={item.title}>
  <span class="frame">
    {#if item.has_poster}
      <img src={posterUrl(item)} alt="" loading="lazy" />
    {:else}
      <span class="tile" aria-hidden="true">
        <span class="initial">{item.title.charAt(0).toUpperCase()}</span>
        <span class="tile-title">{item.title}</span>
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
    <span class="title">{item.title}</span>
    <span class="meta">
      {#if item.year}{item.year}{/if}
      {#if item.content_rating} · {item.content_rating}{/if}
      {#if item.vote_average} · ★ {item.vote_average.toFixed(1)}{/if}
    </span>
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
</style>
