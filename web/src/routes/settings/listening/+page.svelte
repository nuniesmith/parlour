<script lang="ts">
  // <profile> Preferences → Listening (AB8, D29/D35): this profile's
  // own listening sessions and stats, kept by the server. What it is
  // listening to now (open sessions still reporting), its totals — all
  // time, today, per day and per book, counted in this browser's time
  // zone — and its history, newest first. One listen can be several
  // sessions: each runs from pressing play to leaving the book.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import {
    formatDateTime,
    formatDurationMs,
    formatListened,
    posterUrl,
    type ListeningSession,
    type ListeningSessionPage,
    type ListeningStats,
  } from '$lib/types';

  const DAYS_SHOWN = 14;

  let stats = $state<ListeningStats | null>(null);
  let sessions = $state<ListeningSession[] | null>(null);
  let nextBefore = $state<string | null>(null);
  let error = $state<string | null>(null);
  let loadingMore = $state(false);

  const live = $derived((sessions ?? []).filter((s) => s.live));
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';

  /** The last DAYS_SHOWN days, today first, with 0 for days off. */
  const recentDays = $derived.by(() => {
    const byDate = new Map((stats?.days ?? []).map((d) => [d.date, d.listened_ms]));
    const out: { date: string; label: string; ms: number }[] = [];
    const fmt = new Intl.DateTimeFormat(undefined, {
      weekday: 'short',
      month: 'short',
      day: 'numeric',
    });
    for (let i = 0; i < DAYS_SHOWN; i++) {
      const d = new Date();
      d.setDate(d.getDate() - i);
      const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(
        d.getDate(),
      ).padStart(2, '0')}`;
      out.push({ date: key, label: i === 0 ? 'Today' : fmt.format(d), ms: byDate.get(key) ?? 0 });
    }
    return out;
  });
  const longestDay = $derived(Math.max(1, ...recentDays.map((d) => d.ms)));

  async function load() {
    const [s, h] = await Promise.all([
      api<ListeningStats>(`/api/v1/listening/stats?tz=${encodeURIComponent(timeZone)}`),
      api<ListeningSessionPage>('/api/v1/listening/sessions?limit=30'),
    ]);
    if (s.data) stats = s.data;
    if (h.data) {
      sessions = h.data.sessions;
      nextBefore = h.data.next_before;
    }
    error = s.data && h.data ? null : (s.error ?? h.error ?? 'Could not load your listening.');
  }

  async function more() {
    if (!nextBefore) return;
    loadingMore = true;
    const { data, error: err } = await api<ListeningSessionPage>(
      `/api/v1/listening/sessions?limit=30&before=${encodeURIComponent(nextBefore)}`,
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
    const timer = setInterval(() => void load(), 30_000);
    return () => clearInterval(timer);
  });

  function span(s: ListeningSession): string {
    const from = formatDurationMs(s.start_position_ms) || '0:00';
    const to = formatDurationMs(s.position_ms) || '0:00';
    const end = s.duration_ms && s.position_ms >= s.duration_ms ? ' (end)' : '';
    return `from ${from} to ${to}${end}`;
  }

  function device(s: ListeningSession): string {
    return [s.client_name, s.device_name].filter(Boolean).join(' · ');
  }
</script>

<h2>Listening</h2>
<p class="dim">
  What this profile has listened to, kept by the server so it follows you between devices. Times
  are counted while a book is playing, not while it is paused.
</p>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if live.length > 0}
  <section aria-label="Listening now">
    <h3>Listening now</h3>
    <ul class="rows">
      {#each live as s (s.id)}
        <li>
          <a class="thumb" href={`/book/${s.item_id}`} aria-hidden="true" tabindex="-1">
            {#if s.has_cover}<img src={posterUrl(s.item_id)} alt="" loading="lazy" />{:else}<span
                >{s.title.charAt(0).toUpperCase()}</span
              >{/if}
          </a>
          <div>
            <a class="title" href={`/book/${s.item_id}`}>{s.title}</a>
            <p class="dim small">
              At {formatDurationMs(s.position_ms) || '0:00'} · {formatListened(s.listened_ms)} so far{device(
                s,
              )
                ? ` · ${device(s)}`
                : ''}
            </p>
          </div>
        </li>
      {/each}
    </ul>
  </section>
{/if}

{#if stats}
  <section aria-label="Stats">
    <h3>Stats</h3>
    <dl class="totals">
      <div><dt>Today</dt><dd>{formatListened(stats.today_ms)}</dd></div>
      <div><dt>All time</dt><dd>{formatListened(stats.total_ms)}</dd></div>
      <div><dt>Days listened</dt><dd>{stats.days_listened}</dd></div>
      <div><dt>Sessions</dt><dd>{stats.sessions}</dd></div>
    </dl>

    <h4>The last {DAYS_SHOWN} days</h4>
    <table class="days">
      <caption class="sr-only">Listening time per day, today first</caption>
      <tbody>
        {#each recentDays as d (d.date)}
          <tr>
            <th scope="row">{d.label}</th>
            <td class="bar-cell">
              <span class="bar" style={`width: ${((d.ms / longestDay) * 100).toFixed(1)}%`}></span>
            </td>
            <td class="num">{d.ms > 0 ? formatListened(d.ms) : '—'}</td>
          </tr>
        {/each}
      </tbody>
    </table>

    {#if stats.books.length > 0}
      <h4>By book</h4>
      <ol class="rows">
        {#each stats.books as b (b.item_id)}
          <li>
            <a class="thumb" href={`/book/${b.item_id}`} aria-hidden="true" tabindex="-1">
              {#if b.has_cover}<img src={posterUrl(b.item_id)} alt="" loading="lazy" />{:else}<span
                  >{b.title.charAt(0).toUpperCase()}</span
                >{/if}
            </a>
            <div>
              <a class="title" href={`/book/${b.item_id}`}>{b.title}</a>
              <p class="dim small">
                {formatListened(b.listened_ms)} over {b.sessions}
                {b.sessions === 1 ? 'session' : 'sessions'} · last {formatDateTime(b.last_listened_at)}
              </p>
            </div>
          </li>
        {/each}
      </ol>
    {/if}
    <p class="dim small">Days are counted in {stats.time_zone}.</p>
  </section>
{/if}

<section aria-label="History">
  <h3>History</h3>
  {#if sessions === null}
    {#if !error}<p class="dim">Loading…</p>{/if}
  {:else if sessions.length === 0}
    <p class="dim">Nothing yet. Play an audiobook and its sessions show up here.</p>
  {:else}
    <ul class="rows">
      {#each sessions as s (s.id)}
        <li>
          <a class="thumb" href={`/book/${s.item_id}`} aria-hidden="true" tabindex="-1">
            {#if s.has_cover}<img src={posterUrl(s.item_id)} alt="" loading="lazy" />{:else}<span
                >{s.title.charAt(0).toUpperCase()}</span
              >{/if}
          </a>
          <div>
            <a class="title" href={`/book/${s.item_id}`}>{s.title}</a>
            <p class="small">
              {formatDateTime(s.started_at)} · listened {span(s)} ·
              {formatListened(s.listened_ms)}{#if s.live}<span class="now"> · now</span>{/if}
            </p>
            {#if device(s)}<p class="dim small">{device(s)}</p>{/if}
          </div>
        </li>
      {/each}
    </ul>
    {#if nextBefore}
      <p><button class="btn" disabled={loadingMore} onclick={more}>Load older</button></p>
    {/if}
  {/if}
</section>

<style>
  h3 {
    margin: 1.6rem 0 0.6rem;
  }
  h4 {
    margin: 1.2rem 0 0.5rem;
    font-size: 0.95rem;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.6rem;
    max-width: 44rem;
  }
  .rows li {
    display: grid;
    grid-template-columns: 3rem minmax(0, 1fr);
    gap: 0.75rem;
    align-items: center;
  }
  .rows p {
    margin: 0.1rem 0 0;
  }
  .thumb {
    display: grid;
    place-items: center;
    aspect-ratio: 1;
    border-radius: 0.35rem;
    overflow: hidden;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    color: var(--text-dim);
    text-decoration: none;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .title {
    font-weight: 600;
    color: inherit;
  }
  .small {
    font-size: 0.85rem;
  }
  .now {
    color: var(--accent);
    font-weight: 600;
  }
  .totals {
    display: flex;
    flex-wrap: wrap;
    gap: 0.8rem;
    margin: 0;
  }
  .totals div {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.6rem 0.9rem;
    min-width: 8rem;
  }
  .totals dt {
    font-size: 0.8rem;
    color: var(--text-dim);
  }
  .totals dd {
    margin: 0.2rem 0 0;
    font-size: 1.15rem;
    font-weight: 600;
  }
  .days {
    border-collapse: collapse;
    width: 100%;
    max-width: 36rem;
  }
  .days th {
    text-align: left;
    font-weight: normal;
    font-size: 0.85rem;
    padding: 0.15rem 0.8rem 0.15rem 0;
    white-space: nowrap;
  }
  .bar-cell {
    width: 100%;
  }
  .bar {
    display: block;
    height: 0.7rem;
    min-width: 1px;
    border-radius: 0 0.25rem 0.25rem 0;
    background: var(--accent);
  }
  .num {
    font-size: 0.85rem;
    padding-left: 0.8rem;
    white-space: nowrap;
    text-align: right;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
