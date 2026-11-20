<script lang="ts">
  // The next-episode countdown card (SEG6, D23): the episode that plays
  // next — its show's poster (the same pick as a Next Up tile), show,
  // and "S1:E3 · Title" — a 10 s countdown, Play now and Cancel. The
  // player decides when it shows and what the buttons do. When it asks
  // instead (SEG7, D24) it says "Still watching?" in place of the
  // countdown, and the same two buttons read Keep watching and Stop:
  // they stay the same elements either way, so a key press that turns
  // the question back into the countdown never takes the focus away.
  import { COUNTDOWN_MS } from './autoplay';
  import { episodeLabel, posterUrl, type RailItem } from './types';

  let {
    next,
    remainingMs,
    asking = false,
    onplaynow,
    oncancel,
  }: {
    next: RailItem;
    remainingMs: number;
    /** Ask "Still watching?" rather than count down. */
    asking?: boolean;
    onplaynow: () => void;
    oncancel: () => void;
  } = $props();

  const heading = $derived(next.series_title ?? next.title);
  const epLine = $derived(
    episodeLabel(next.season_number, next.episode_number, next.series_title ? next.title : null),
  );
  const artId = $derived(
    next.poster_item_id !== undefined ? next.poster_item_id : next.has_poster ? next.id : null,
  );
  const seconds = $derived(Math.max(1, Math.ceil(remainingMs / 1000)));
  const left = $derived(Math.min(1, Math.max(0, remainingMs / COUNTDOWN_MS)));
</script>

<section class="next-card" class:asking aria-label="Next episode">
  <span class="art" aria-hidden="true">
    {#if artId}
      <img src={posterUrl(artId)} alt="" />
    {:else}
      <span class="tile">{heading.charAt(0).toUpperCase()}</span>
    {/if}
  </span>
  <div class="body">
    <p class="eyebrow">Next episode</p>
    <p class="show" class:has-ep={epLine !== null}>{heading}</p>
    {#if epLine}<p class="ep">{epLine}</p>{/if}
    {#if asking}
      <p class="ask" role="status">Still watching?</p>
    {:else}
      <p class="count"><span class="lead">Starts in</span> {seconds} s</p>
    {/if}
    <!-- `data-card-button`: these count as an interaction when they act
         (`CARD_BUTTON` in $lib/stillWatching). -->
    <div class="actions">
      <button type="button" class="btn primary small" data-card-button onclick={onplaynow}>
        {asking ? 'Keep watching' : 'Play now'}
      </button>
      <button type="button" class="btn small" data-card-button onclick={oncancel}>
        {asking ? 'Stop' : 'Cancel'}
      </button>
    </div>
  </div>
  {#if !asking}
    <span class="bar" aria-hidden="true"><span style={`width: ${left * 100}%`}></span></span>
  {/if}
</section>

<style>
  .next-card {
    /* The player lays the card in a layer over the video that lets the
       pointer through to it; the card itself must take clicks. */
    pointer-events: auto;
    position: relative;
    display: flex;
    gap: 0.8rem;
    align-items: stretch;
    /* The player's layer spans the player, less its margins. */
    width: min(22rem, 100%);
    padding: 0.7rem 0.8rem 0.85rem;
    background: rgba(11, 14, 20, 0.9);
    border: 1px solid var(--line);
    border-radius: 0.7rem;
    box-shadow: 0 0.3rem 1.2rem rgba(0, 0, 0, 0.55);
    overflow: hidden;
  }
  .art {
    flex: none;
    width: 4.2rem;
    aspect-ratio: var(--poster-ratio, 2 / 3);
    border-radius: 0.4rem;
    overflow: hidden;
    background: var(--bg-card);
    align-self: center;
  }
  .art img {
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
    font-size: 1.6rem;
    font-weight: 700;
    color: var(--accent);
    background: linear-gradient(160deg, #2a3350, #171c2a);
  }
  .body {
    min-width: 0;
    display: grid;
    gap: 0.15rem;
    align-content: center;
  }
  .body p {
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .eyebrow {
    font-size: 0.72rem;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-dim);
  }
  .show {
    font-weight: 600;
  }
  .ep,
  .count {
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .ask {
    margin-top: 0.15rem;
    font-weight: 700;
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.35rem;
  }
  .bar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: rgba(255, 255, 255, 0.12);
  }
  .bar > span {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.25s linear;
  }
  /* Phones: a 16:9 player there is ~200 px tall and the card sits above
     the browser's control bar, so it keeps to two rows — the episode,
     then the count beside the buttons — under a smaller poster. */
  @media (max-width: 600px) {
    .next-card {
      gap: 0.6rem;
      padding: 0.5rem 0.6rem 0.65rem;
    }
    .art {
      width: 2.2rem;
    }
    .body {
      grid-template-columns: auto minmax(0, 1fr);
      column-gap: 0.6rem;
      align-items: center;
    }
    .eyebrow,
    .show.has-ep,
    .lead {
      display: none;
    }
    .show,
    .ep {
      grid-column: 1 / -1;
      font-size: 0.88rem;
      font-weight: 600;
      color: inherit;
    }
    .count {
      grid-column: 1;
    }
    .actions {
      grid-column: 2;
      justify-content: flex-end;
      margin-top: 0;
    }
    /* Asking: the question takes the episode's row, and the buttons —
       wider now — get the next row to themselves, where they always sit. */
    .asking .show,
    .asking .ep {
      display: none;
    }
    .ask {
      grid-column: 1 / -1;
      margin-top: 0;
      font-size: 0.88rem;
    }
    .asking .actions {
      grid-column: 1 / -1;
    }
  }
</style>
