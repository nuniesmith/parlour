<script lang="ts">
  // Preferences → Home Screen: which rails Home shows, and in
  // what order. Saved on the profile, so every device signed in
  // to it sees the same Home.
  import { session } from '$lib/session.svelte';
  import { ALWAYS_SHOWN_RAILS, RAIL_LABELS, updateActiveProfile } from '$lib/preferences';
  import type { HomeRail } from '$lib/types';

  let rails = $state<HomeRail[]>(
    (session.me?.active_profile?.home_rails ?? []).map((r) => ({ ...r })),
  );
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);

  function move(index: number, by: -1 | 1) {
    const to = index + by;
    if (to < 0 || to >= rails.length) return;
    const next = [...rails];
    [next[index], next[to]] = [next[to], next[index]];
    rails = next;
    notice = null;
  }

  function reset() {
    rails = Object.keys(RAIL_LABELS).map((id) => ({ id, visible: true }));
    notice = null;
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    error = null;
    notice = null;
    const r = await updateActiveProfile({ home_rails: rails });
    saving = false;
    if (r.ok) notice = 'Saved.';
    else error = r.error;
  }
</script>

<h2>Home Screen</h2>
<p class="dim">
  Choose which rows Home shows for this profile, and their order. The audiobook shelves also set
  the order of an audiobooks library's own Home; Continue listening is always shown.
</p>

<form class="form" onsubmit={save}>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  <ol class="rails">
    {#each rails as rail, i (rail.id)}
      <li>
        <label class="check">
          {#if ALWAYS_SHOWN_RAILS.includes(rail.id)}
            <input type="checkbox" checked disabled />
            {RAIL_LABELS[rail.id] ?? rail.id} <span class="dim">(always shown)</span>
          {:else}
            <input type="checkbox" bind:checked={rail.visible} />
            {RAIL_LABELS[rail.id] ?? rail.id}
          {/if}
        </label>
        <span class="order">
          <button
            type="button"
            class="btn small"
            disabled={i === 0}
            aria-label={`Move ${RAIL_LABELS[rail.id] ?? rail.id} up`}
            onclick={() => move(i, -1)}>Up</button
          >
          <button
            type="button"
            class="btn small"
            disabled={i === rails.length - 1}
            aria-label={`Move ${RAIL_LABELS[rail.id] ?? rail.id} down`}
            onclick={() => move(i, 1)}>Down</button
          >
        </span>
      </li>
    {/each}
  </ol>
  <p>
    <button class="btn primary" type="submit" disabled={saving}>Save</button>
    <button class="btn" type="button" onclick={reset}>Reset to default</button>
  </p>
</form>

<style>
  .rails {
    list-style: none;
    padding: 0;
    margin: 0 0 1rem;
    display: grid;
    gap: 0.5rem;
  }
  .rails li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.6rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.5rem 0.8rem;
  }
  .rails li :global(.check) {
    margin: 0;
  }
  .order {
    display: flex;
    gap: 0.4rem;
  }
</style>
