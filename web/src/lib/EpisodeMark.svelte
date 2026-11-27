<script lang="ts">
  // POD6: this profile's state for an episode, as a small mark: a
  // finished check, a progress ring for a part-played one, or a "new"
  // dot. The label is for screen readers (and the tooltip).
  import type { EpisodeState } from '$lib/types';

  let { status, fraction = 0 }: { status: EpisodeState; fraction?: number } = $props();

  const R = 8;
  const C = 2 * Math.PI * R;
  const pct = $derived(Math.round(Math.max(0, Math.min(1, fraction)) * 100));
  const label = $derived(
    status === 'finished' ? 'Played' : status === 'in_progress' ? `${pct}% played` : 'New',
  );
</script>

<span class="mark {status}" role="img" aria-label={label} title={label} data-state={status} data-pct={pct}>
  {#if status === 'finished'}
    <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true">
      <circle cx="10" cy="10" r="9" class="fill" />
      <path d="M5.5 10.5l3 3 6-7" class="tick" />
    </svg>
  {:else if status === 'in_progress'}
    <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true">
      <circle cx="10" cy="10" r={R} class="track" />
      <circle
        cx="10"
        cy="10"
        r={R}
        class="ring"
        stroke-dasharray={`${(C * pct) / 100} ${C}`}
        transform="rotate(-90 10 10)"
      />
    </svg>
  {:else}
    <svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true">
      <circle cx="10" cy="10" r="4" class="dot" />
    </svg>
  {/if}
</span>

<style>
  .mark {
    display: inline-grid;
    place-items: center;
    width: 20px;
    height: 20px;
    flex: none;
  }
  .fill {
    fill: var(--accent);
  }
  .tick {
    fill: none;
    stroke: var(--accent-ink);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .track {
    fill: none;
    stroke: var(--line);
    stroke-width: 3;
  }
  .ring {
    fill: none;
    stroke: var(--accent);
    stroke-width: 3;
    stroke-linecap: round;
  }
  .dot {
    fill: var(--accent);
  }
</style>
