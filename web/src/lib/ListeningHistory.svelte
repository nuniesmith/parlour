<script lang="ts">
  // Server → Sessions, below Now Playing (AB8, D29): every profile's
  // listening sessions, newest first, from
  // GET /api/v1/admin/listening/sessions — what Audiobookshelf shows
  // its admins. Open ones are marked; Now Playing above has the live
  // controls.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import {
    formatDateTime,
    formatDurationMs,
    formatListened,
    type ListeningSession,
    type ListeningSessionPage,
  } from '$lib/types';

  let sessions = $state<ListeningSession[] | null>(null);
  let nextBefore = $state<string | null>(null);
  let error = $state<string | null>(null);
  let loadingMore = $state(false);

  const REASON: Record<string, string> = {
    stopped: 'Stopped',
    replaced: 'Started again',
    timeout: 'Player went away',
    admin: 'Stopped by an admin',
  };

  async function load() {
    const { data, error: err } = await api<ListeningSessionPage>(
      '/api/v1/admin/listening/sessions?limit=25',
    );
    if (data) {
      sessions = data.sessions;
      nextBefore = data.next_before;
      error = null;
    } else {
      error = err ?? 'Could not load listening sessions.';
    }
  }

  async function more() {
    if (!nextBefore) return;
    loadingMore = true;
    const { data, error: err } = await api<ListeningSessionPage>(
      `/api/v1/admin/listening/sessions?limit=25&before=${encodeURIComponent(nextBefore)}`,
    );
    loadingMore = false;
    if (data) {
      sessions = [...(sessions ?? []), ...data.sessions];
      nextBefore = data.next_before;
    } else {
      error = err ?? 'Could not load older sessions.';
    }
  }

  onMount(() => {
    void load();
  });

  function stateOf(s: ListeningSession): string {
    if (s.live) return 'Listening now';
    if (!s.ended_at) return 'Open';
    return REASON[s.end_reason ?? ''] ?? 'Ended';
  }
</script>

<h2 class="head">Listening sessions</h2>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if sessions === null}
  {#if !error}<p class="dim">Loading…</p>{/if}
{:else if sessions.length === 0}
  <p class="dim quiet">Nobody has listened to an audiobook yet.</p>
{:else}
  <div class="wrap">
    <table>
      <thead>
        <tr>
          <th scope="col">Started</th>
          <th scope="col">Who</th>
          <th scope="col">Book</th>
          <th scope="col">Listened</th>
          <th scope="col">Position</th>
          <th scope="col">Client</th>
          <th scope="col">State</th>
        </tr>
      </thead>
      <tbody>
        {#each sessions as s (s.id)}
          <tr>
            <td>{formatDateTime(s.started_at)}</td>
            <td>
              {s.username}{#if s.profile_name !== s.username}<span class="dim">
                  · {s.profile_name}</span
                >{/if}
            </td>
            <td><a href={`/book/${s.item_id}`}>{s.title}</a></td>
            <td>{formatListened(s.listened_ms)}</td>
            <td>
              {formatDurationMs(s.start_position_ms) || '0:00'} → {formatDurationMs(s.position_ms) ||
                '0:00'}
            </td>
            <td class="dim">{[s.client_name, s.device_name].filter(Boolean).join(' · ')}</td>
            <td class:live={s.live}>{stateOf(s)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if nextBefore}
    <p><button class="btn" disabled={loadingMore} onclick={more}>Load older</button></p>
  {/if}
{/if}

<style>
  .head {
    margin: 2rem 0 0.7rem;
  }
  .quiet {
    margin: 0;
  }
  .wrap {
    overflow-x: auto;
    max-width: 68rem;
  }
  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.88rem;
  }
  th,
  td {
    text-align: left;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid var(--line);
    white-space: nowrap;
  }
  th {
    color: var(--text-dim);
    font-weight: 600;
  }
  td a {
    color: inherit;
  }
  .live {
    color: var(--accent);
    font-weight: 600;
  }
</style>
