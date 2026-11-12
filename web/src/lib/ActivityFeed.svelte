<script lang="ts">
  // Activity feed (UI4): newest-first audit trail from
  // GET /api/v1/admin/activity. `compact` is the Dashboard
  // preview (a few lines, auto-refreshing); otherwise it is the
  // full page with a category filter and "Load older".
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { ActivityEntry, ActivityPage } from '$lib/types';

  let { compact = false }: { compact?: boolean } = $props();

  const CATEGORIES: { value: string; label: string }[] = [
    { value: '', label: 'Everything' },
    { value: 'auth', label: 'Sign-ins & passwords' },
    { value: 'playback', label: 'Playback' },
    { value: 'scan', label: 'Library scans' },
    { value: 'import', label: 'Imports' },
    { value: 'invite', label: 'Invites' },
    { value: 'user', label: 'Users' },
  ];

  let entries = $state<ActivityEntry[] | null>(null);
  let nextBefore = $state<string | null>(null);
  let category = $state('');
  let error = $state<string | null>(null);
  let loadingMore = $state(false);

  function url(before: string | null): string {
    const params = new URLSearchParams({ limit: compact ? '8' : '50' });
    if (category) params.set('category', category);
    if (before) params.set('before', before);
    return `/api/v1/admin/activity?${params}`;
  }

  async function load() {
    const { data, error: err } = await api<ActivityPage>(url(null));
    if (data) {
      entries = data.entries;
      nextBefore = data.next_before;
      error = null;
    } else {
      error = err ?? 'Could not load activity.';
    }
  }

  async function loadOlder() {
    if (!nextBefore || !entries) return;
    loadingMore = true;
    const { data, error: err } = await api<ActivityPage>(url(nextBefore));
    loadingMore = false;
    if (data) {
      entries = [...entries, ...data.entries];
      nextBefore = data.next_before;
    } else {
      error = err ?? 'Could not load older activity.';
    }
  }

  onMount(() => {
    void load();
    if (!compact) return;
    // The Dashboard preview keeps itself current; the full page
    // does not, so "Load older" never shifts under the reader.
    const timer = setInterval(() => void load(), 15_000);
    return () => clearInterval(timer);
  });

  function categoryOf(kind: string): string {
    return kind.split('.')[0];
  }

  const BADGE: Record<string, string> = {
    auth: 'Sign-in',
    playback: 'Playback',
    scan: 'Scan',
    import: 'Import',
    invite: 'Invite',
    user: 'User',
  };

  function failed(kind: string): boolean {
    return kind.endsWith('.failed');
  }

  function when(iso: string): string {
    return new Date(iso).toLocaleString(undefined, {
      dateStyle: 'medium',
      timeStyle: 'short',
    });
  }
</script>

<div class="head">
  <h2>Activity</h2>
  {#if compact}
    <a class="btn small" href="/admin?tab=activity">All activity</a>
  {:else}
    <label class="filter">
      <span class="sr-only">Show</span>
      <select bind:value={category} onchange={load}>
        {#each CATEGORIES as c (c.value)}
          <option value={c.value}>{c.label}</option>
        {/each}
      </select>
    </label>
  {/if}
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if entries === null}
  {#if !error}<p class="dim">Loading…</p>{/if}
{:else if entries.length === 0}
  <p class="dim">
    {category ? 'Nothing of that kind in the last 90 days.' : 'Nothing has happened yet.'}
  </p>
{:else}
  <ul class="feed">
    {#each entries as e (e.id)}
      <li>
        <span class="chip" class:bad={failed(e.kind)}
          >{BADGE[categoryOf(e.kind)] ?? categoryOf(e.kind)}</span
        >
        <span class="summary" class:error={failed(e.kind)}>
          {#if e.item_id}
            <a href={`/item/${e.item_id}`}>{e.summary}</a>
          {:else}
            {e.summary}
          {/if}
        </span>
        <time class="dim" datetime={e.created_at}>{when(e.created_at)}</time>
      </li>
    {/each}
  </ul>
  {#if !compact}
    {#if nextBefore}
      <p>
        <button class="btn" disabled={loadingMore} onclick={loadOlder}>
          {loadingMore ? 'Loading…' : 'Load older'}
        </button>
      </p>
    {:else}
      <p class="dim end">That is everything. Activity is kept for 90 days.</p>
    {/if}
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    max-width: 52rem;
    margin: 1.4rem 0 0.7rem;
  }
  .head h2 {
    margin: 0;
  }
  .filter select {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.4rem 0.6rem;
  }
  .feed {
    list-style: none;
    margin: 0;
    padding: 0;
    max-width: 52rem;
  }
  .feed li {
    display: grid;
    grid-template-columns: 5.5rem minmax(0, 1fr) auto;
    gap: 0.7rem;
    align-items: baseline;
    padding: 0.5rem 0.1rem;
    border-bottom: 1px solid var(--line);
  }
  .chip {
    text-align: center;
  }
  .chip.bad {
    border-color: var(--danger);
    color: var(--danger);
  }
  .summary {
    overflow-wrap: anywhere;
  }
  .summary a:hover {
    color: var(--accent);
  }
  time {
    font-size: 0.85rem;
    white-space: nowrap;
  }
  .end {
    font-size: 0.9rem;
  }
  @media (max-width: 640px) {
    .feed li {
      grid-template-columns: minmax(0, 1fr) auto;
    }
    .feed .summary {
      grid-column: 1 / -1;
      grid-row: 2;
    }
    .chip {
      justify-self: start;
    }
  }
</style>
