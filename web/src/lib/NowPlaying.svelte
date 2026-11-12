<script lang="ts">
  // Now Playing (UI3): one card per live playback session, fed by
  // GET /api/v1/admin/sessions every 5 s. Each card shows what is
  // playing, who and where from, and the stream decision —
  // source → what the server does with it, and why.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { formatDurationMs, type PlaybackSession, type StreamPlan } from '$lib/types';

  const POLL_MS = 5_000;

  let sessions = $state<PlaybackSession[] | null>(null);
  let error = $state<string | null>(null);
  let stopping = $state<string | null>(null);
  let brokenPosters = $state<string[]>([]);

  async function load() {
    const { data, error: err } = await api<PlaybackSession[]>('/api/v1/admin/sessions');
    if (data) {
      sessions = data;
      error = null;
    } else {
      error = err ?? 'Could not load playback sessions.';
    }
  }

  async function stop(s: PlaybackSession) {
    stopping = s.session_id;
    const { status, error: err } = await api(`/api/v1/admin/sessions/${s.session_id}/stop`, {
      method: 'POST',
    });
    stopping = null;
    // 404 = it already ended on its own; either way, refresh.
    if (status !== 204 && status !== 404) error = err ?? 'Could not stop that session.';
    await load();
  }

  onMount(() => {
    void load();
    const timer = setInterval(() => void load(), POLL_MS);
    return () => clearInterval(timer);
  });

  function heading(s: PlaybackSession): string {
    return s.series_title ?? s.item_title;
  }

  function episodeLine(s: PlaybackSession): string | null {
    if (s.item_kind !== 'episode') return null;
    const where =
      s.season_number !== null && s.episode_number !== null
        ? `S${s.season_number}:E${s.episode_number}`
        : null;
    return [where, s.item_title].filter(Boolean).join(' · ');
  }

  function fraction(s: PlaybackSession): number {
    if (!s.duration_ms || s.duration_ms <= 0) return 0;
    return Math.max(0, Math.min(1, s.position_ms / s.duration_ms));
  }

  function clientLine(s: PlaybackSession): string {
    const app = [s.client.name, s.client.version].filter(Boolean).join(' ');
    const parts = [app, s.client.device].filter(Boolean);
    if (parts.length) return parts.join(' · ');
    return s.client.user_agent ?? 'Unknown client';
  }

  function remoteLine(s: PlaybackSession): string {
    const { ip, forwarded_for, protocol } = s.remote;
    const from = forwarded_for
      ? `${forwarded_for} (as reported by proxy ${ip ?? 'unknown'})`
      : (ip ?? 'Unknown address');
    return [from, protocol].filter(Boolean).join(' · ');
  }

  function rate(kbps: number | null): string {
    if (!kbps) return '';
    return kbps >= 1000 ? `${(kbps / 1000).toFixed(1)} Mbps` : `${kbps} kbps`;
  }

  function upper(v: string | null): string {
    return v ? v.toUpperCase() : 'Unknown';
  }

  const MODE: Record<string, string> = {
    direct_play: 'Direct play',
    copy: 'Copied as-is',
    transcode: 'Transcoded',
  };

  const REASON: Record<string, string> = {
    Direct: 'The client can play this file as it is.',
    ContainerUnsupported:
      'The client cannot open this container, so the streams are repackaged without re-encoding.',
    CodecUnsupported:
      'A codec in this file is outside what the client supports. It is repackaged only, and may not play until transcoding exists.',
    RemoteBandwidth:
      "This file is over the profile's remote bitrate cap. The original is sent until transcoding exists.",
    HdrToneMap: 'HDR source on an SDR client: tone mapping is needed.',
    SubtitleBurnIn: 'The chosen subtitle has to be drawn into the picture for this client.',
  };

  function streamRow(p: StreamPlan): { from: string; to: string } {
    const size = rate(p.bitrate_kbps);
    const from = `${upper(p.container)}${size ? ` (${size})` : ''}`;
    const to =
      p.delivery === 'hls'
        ? p.action === 'Transcode'
          ? 'HLS (transcode)'
          : 'HLS (remux)'
        : 'Original file';
    return { from, to };
  }

  function videoSource(p: StreamPlan): string {
    const v = p.video;
    if (!v) return '';
    const res = v.height ? (v.height >= 2160 ? '4K' : `${v.height}p`) : '';
    return [res, upper(v.codec), v.hdr_format?.toUpperCase()].filter(Boolean).join(' ');
  }

  function languageName(code: string | null): string {
    if (!code || code === 'und') return '';
    try {
      return new Intl.DisplayNames(undefined, { type: 'language' }).of(code) ?? code;
    } catch {
      return code;
    }
  }

  function channelLayout(n: number | null): string {
    if (!n) return '';
    if (n === 1) return 'Mono';
    if (n === 2) return 'Stereo';
    if (n === 6) return '5.1';
    if (n === 8) return '7.1';
    return `${n} ch`;
  }

  function audioSource(p: StreamPlan): string {
    const a = p.audio;
    if (!a) return '';
    const label = [languageName(a.language), upper(a.codec), channelLayout(a.channels)]
      .filter(Boolean)
      .join(' ');
    return a.is_default ? `${label} (default)` : label;
  }
</script>

<div class="head">
  <h2>Now playing</h2>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if sessions === null}
  {#if !error}<p class="dim">Loading…</p>{/if}
{:else if sessions.length === 0}
  <p class="dim quiet">Nothing is playing.</p>
{:else}
  <ul class="cards">
    {#each sessions as s (s.session_id)}
      {@const ep = episodeLine(s)}
      <li class="card">
        <div class="top">
          <a class="thumb" href={`/item/${s.item_id}`} aria-label={`Open ${heading(s)}`}>
            {#if s.poster_item_id && !brokenPosters.includes(s.poster_item_id)}
              <img
                src={`/api/v1/items/${s.poster_item_id}/artwork/poster?v=w300`}
                alt=""
                loading="lazy"
                onerror={() => (brokenPosters = [...brokenPosters, s.poster_item_id ?? ''])}
              />
            {:else}
              <span class="initial" aria-hidden="true">{heading(s).charAt(0).toUpperCase()}</span>
            {/if}
          </a>
          <div class="what">
            <h3>{heading(s)}</h3>
            {#if ep}<p class="ep">{ep}</p>{/if}
            <p class="time">
              {formatDurationMs(s.position_ms) || '0:00'}
              {#if s.duration_ms}<span class="dim"> / {formatDurationMs(s.duration_ms)}</span>{/if}
            </p>
            <div
              class="bar"
              role="progressbar"
              aria-label="Playback position"
              aria-valuemin="0"
              aria-valuemax="100"
              aria-valuenow={Math.round(fraction(s) * 100)}
            >
              <span style={`width: ${(fraction(s) * 100).toFixed(1)}%`}></span>
            </div>
          </div>
        </div>

        <p class="line">{clientLine(s)}</p>
        <p class="line dim">{remoteLine(s)}</p>

        {#if s.plan}
          {@const st = streamRow(s.plan)}
          <dl class="plan">
            <div>
              <dt>Stream</dt>
              <dd>{st.from} <span class="arrow" aria-label="to">→</span> {st.to}</dd>
            </div>
            {#if s.plan.video}
              <div>
                <dt>Video</dt>
                <dd>
                  {videoSource(s.plan)} <span class="arrow" aria-label="to">→</span>
                  {MODE[s.plan.video.mode] ?? s.plan.video.mode}
                </dd>
              </div>
            {/if}
            {#if s.plan.audio}
              <div>
                <dt>Audio</dt>
                <dd>
                  {audioSource(s.plan)} <span class="arrow" aria-label="to">→</span>
                  {MODE[s.plan.audio.mode] ?? s.plan.audio.mode}
                </dd>
              </div>
            {/if}
          </dl>
          <p class="why dim">{REASON[s.plan.reason] ?? s.plan.reason}</p>
        {:else}
          <p class="why dim">No stream decision was recorded for this session.</p>
        {/if}

        <div class="foot">
          <span class="who">
            {s.username}{#if s.profile_name !== s.username}<span class="dim">
                · {s.profile_name}</span
              >{/if}
          </span>
          <button
            class="btn small danger"
            disabled={stopping === s.session_id}
            onclick={() => stop(s)}
          >
            {stopping === s.session_id ? 'Stopping…' : 'Stop'}
          </button>
        </div>
      </li>
    {/each}
  </ul>
  <p class="dim note">
    Stop ends the stream from the server side and the player is told why. Pausing someone's player
    remotely is not possible yet — that needs support in the player itself.
  </p>
{/if}

<style>
  .head {
    margin: 1.4rem 0 0.7rem;
  }
  .head h2 {
    margin: 0;
  }
  .quiet {
    margin: 0;
  }
  .cards {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 21rem), 1fr));
    gap: 0.9rem;
    max-width: 68rem;
  }
  .card {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.9rem;
    display: grid;
    gap: 0.5rem;
    align-content: start;
  }
  .top {
    display: grid;
    grid-template-columns: 4.5rem minmax(0, 1fr);
    gap: 0.8rem;
  }
  .thumb {
    display: block;
    aspect-ratio: var(--poster-ratio);
    border-radius: 0.4rem;
    overflow: hidden;
    background: var(--bg-card);
    border: 1px solid var(--line);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .initial {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
    font-size: 1.6rem;
    font-weight: 700;
    color: var(--accent);
  }
  .what {
    min-width: 0;
    display: grid;
    gap: 0.2rem;
    align-content: start;
  }
  h3 {
    margin: 0;
    font-size: 1.05rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  p {
    margin: 0;
  }
  .ep {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .time {
    font-variant-numeric: tabular-nums;
    font-size: 0.9rem;
    margin-top: 0.2rem;
  }
  .bar {
    height: 0.3rem;
    border-radius: 999px;
    background: var(--line);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .line {
    font-size: 0.9rem;
    overflow-wrap: anywhere;
  }
  .plan {
    margin: 0.2rem 0 0;
    border-top: 1px solid var(--line);
  }
  .plan div {
    display: grid;
    grid-template-columns: 3.6rem minmax(0, 1fr);
    gap: 0.6rem;
    padding: 0.4rem 0;
    border-bottom: 1px solid var(--line);
    font-size: 0.9rem;
  }
  .plan dt {
    color: var(--text-dim);
  }
  .plan dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .arrow {
    color: var(--accent);
  }
  .why {
    font-size: 0.85rem;
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.8rem;
    margin-top: 0.2rem;
  }
  .who {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .btn:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .note {
    font-size: 0.85rem;
    max-width: 52rem;
    margin-top: 0.7rem;
  }
</style>
