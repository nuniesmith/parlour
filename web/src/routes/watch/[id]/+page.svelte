<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { page } from '$app/state';
  import Hls from 'hls.js';
  import { api } from '$lib/api';
  import { formatDurationMs, type ItemDetail, type PlaybackInfo } from '$lib/types';

  let detail = $state<ItemDetail | null>(null);
  let info = $state<PlaybackInfo | null>(null);
  let error = $state<string | null>(null);
  let video = $state<HTMLVideoElement | null>(null);
  let hls: Hls | null = null;
  let started = $state(false);
  let resumeChoice = $state<'pending' | 'resume' | 'restart'>('pending');
  let progressTimer: ReturnType<typeof setInterval> | null = null;
  let reported = false;

  const itemId = $derived(page.params.id ?? '');

  function report(kind: 'start' | 'progress' | 'stopped') {
    if (!info || !video) return;
    const body = {
      item_id: info.item_id,
      file_id: info.file_id,
      position_ms: Math.round((video.currentTime || 0) * 1000),
      duration_ms: video.duration ? Math.round(video.duration * 1000) : undefined,
    };
    if (kind === 'start') reported = true;
    void api(`/api/v1/playback/${kind}`, {
      method: 'POST',
      body: JSON.stringify(body),
      keepalive: kind === 'stopped',
    });
  }

  async function waitForPlaylist(url: string) {
    // Remux answers 503 + Retry-After until the playlist exists.
    for (let i = 0; i < 45; i++) {
      const res = await fetch(url, { credentials: 'same-origin' });
      if (res.ok) return;
      await new Promise((r) => setTimeout(r, 2000));
    }
    throw new Error('The remux is taking too long — try again in a bit.');
  }

  async function attach() {
    if (!video || !info) return;
    const url = info.stream_url;
    if (info.stream_kind === 'hls') {
      await waitForPlaylist(url);
      if (video.canPlayType('application/vnd.apple.mpegurl')) {
        video.src = url;
      } else if (Hls.isSupported()) {
        hls = new Hls();
        hls.loadSource(url);
        hls.attachMedia(video);
      } else {
        error = 'This browser cannot play HLS streams.';
        return;
      }
    } else {
      video.src = url;
    }
    video.addEventListener(
      'loadedmetadata',
      () => {
        if (resumeChoice === 'resume' && info && info.resume_position_ms > 0 && video) {
          video.currentTime = info.resume_position_ms / 1000;
        }
        applySubtitleDefault();
      },
      { once: true },
    );
  }

  function applySubtitleDefault() {
    if (!video || !info) return;
    // textTracks line up 1:1 with the rendered <track> elements,
    // which are the playable (text, URL-bearing) subtitles in order.
    const playable = info.subtitles.filter((s) => s.text && s.url);
    const tracks = video.textTracks;
    for (let i = 0; i < tracks.length; i++) {
      tracks[i].mode = playable[i]?.id === info.default_subtitle_id ? 'showing' : 'hidden';
    }
  }

  function begin(choice: 'resume' | 'restart') {
    resumeChoice = choice;
    started = true;
    void attach().then(() => video?.play().catch(() => {}));
  }

  function onPlay() {
    if (!reported) report('start');
    if (!progressTimer) {
      progressTimer = setInterval(() => report('progress'), 10_000);
    }
  }
  function onPause() {
    report('progress');
  }
  function onEnded() {
    report('stopped');
  }

  function onKey(event: KeyboardEvent) {
    if (!video) return;
    // Typing in the sidebar search (or any field) is not a player command.
    const target = event.target;
    if (
      target instanceof HTMLElement &&
      (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
    ) {
      return;
    }
    switch (event.code) {
      case 'Space':
      case 'KeyK':
        event.preventDefault();
        if (video.paused) void video.play();
        else video.pause();
        break;
      case 'ArrowRight':
        video.currentTime += event.shiftKey ? 30 : 10;
        break;
      case 'ArrowLeft':
        video.currentTime -= event.shiftKey ? 30 : 10;
        break;
      case 'KeyM':
        video.muted = !video.muted;
        break;
      case 'KeyF':
        if (document.fullscreenElement) void document.exitFullscreen();
        else void video.requestFullscreen().catch(() => {});
        break;
    }
  }

  onMount(async () => {
    const [d, p] = await Promise.all([
      api<ItemDetail>(`/api/v1/items/${itemId}`),
      api<PlaybackInfo>(`/api/v1/items/${itemId}/playback-info?client=web&network=lan`),
    ]);
    detail = d.data;
    if (p.data) {
      info = p.data;
      if (p.data.resume_position_ms <= 0) {
        resumeChoice = 'restart';
        started = true;
        await attach();
      }
    } else {
      error = p.error ?? 'Playback is not available for this item.';
    }
  });

  onDestroy(() => {
    if (progressTimer) clearInterval(progressTimer);
    if (reported) report('stopped');
    hls?.destroy();
  });
</script>

<svelte:window onkeydown={onKey} />

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if !detail || !info}
  <p class="dim">Loading…</p>
{:else}
  <h1>{detail.title}</h1>
  <p class="dim plan">
    {info.action === 'DirectPlay' ? 'Direct play' : 'Remuxed for your browser'}
    · {info.reason}
    {#each info.warnings as w (w)}<br /><span class="warn">{w}</span>{/each}
  </p>

  <div class="player-wrap">
    <video
      bind:this={video}
      controls
      playsinline
      crossorigin="anonymous"
      onplay={onPlay}
      onpause={onPause}
      onended={onEnded}
    >
      {#each info.subtitles.filter((s) => s.text && s.url) as sub (sub.id)}
        <track kind="subtitles" src={sub.url} srclang={sub.language ?? 'und'} label={`${sub.language ?? 'sub'}${sub.is_forced ? ' (forced)' : ''}${sub.is_sdh ? ' SDH' : ''}`} />
      {/each}
    </video>
    {#if !started && info.resume_position_ms > 0}
      <div class="resume" role="dialog" aria-label="Resume playback">
        <p>
          You left off at <strong>{formatDurationMs(info.resume_position_ms)}</strong>
        </p>
        <div class="row">
          <button class="btn primary" onclick={() => begin('resume')}>Resume</button>
          <button class="btn" onclick={() => begin('restart')}>Start over</button>
        </div>
      </div>
    {/if}
  </div>

  {#if info.audio_streams.length > 1}
    <p class="dim">
      Audio tracks: {info.audio_streams.map((a) => `${a.language ?? '?'} (${a.codec ?? '?'})`).join(' · ')}
      — the browser plays the default track for direct play.
    </p>
  {/if}

  <p class="dim keys">
    Keys: space play/pause · ←/→ seek 10s (shift 30s) · F fullscreen · M mute
  </p>
  <p><a class="btn" href={`/item/${detail.id}`}>← Back to details</a></p>
{/if}

<style>
  .player-wrap {
    position: relative;
    background: #000;
    border-radius: var(--radius);
    overflow: hidden;
  }
  video {
    width: 100%;
    max-height: 78vh;
    display: block;
  }
  .resume {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    align-content: center;
    gap: 0.8rem;
    background: rgba(0, 0, 0, 0.72);
    text-align: center;
  }
  .row {
    display: flex;
    gap: 0.7rem;
    justify-content: center;
  }
  .warn {
    color: var(--accent);
  }
  .keys {
    font-size: 0.85rem;
  }
</style>
