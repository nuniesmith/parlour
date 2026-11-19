<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { page } from '$app/state';
  import Hls from 'hls.js';
  import { api } from '$lib/api';
  import { formatSegmentTime, previewParam } from '$lib/segments';
  import {
    SKIPPED_NOTICE_MS,
    SKIPPED_NOTICES,
    SKIP_LABELS,
    Skipper,
    playerSkipMode,
    skipLanding,
  } from '$lib/skip';
  import {
    deviceLabel,
    formatDurationMs,
    type ItemDetail,
    type MediaSegment,
    type PlaybackInfo,
    type SegmentKind,
  } from '$lib/types';

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

  // SEG4 "Preview from here" (D21): `?preview=<ms>` opens the player
  // at that time and reports nothing — no start, progress or stopped
  // report, so no watch position, played mark, play count, Now Playing
  // session or activity line. Read once: the mode never changes while
  // this page plays.
  const previewMs = previewParam(page.url.searchParams.get('preview'));

  // The id the server gave this playback session, sent back on
  // every later report so two tabs on one item stay distinct.
  let sessionId: string | null = null;
  // An admin ended this session from the Dashboard (UI3).
  let stoppedByAdmin = $state(false);

  // SEG5 skip (D22): the item's segments under the profile's mode,
  // which playback-info sends with them (a preview always gets the
  // button and never skips by itself). Nothing is offered or skipped
  // until the opening seek — resume or preview — has happened, so a
  // segment at 0:00 isn't skipped on the way to the resume point.
  let skipper: Skipper | null = null;
  let positioned = false;
  let offer = $state.raw<MediaSegment | null>(null);
  let skippedNotice = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
  // The stopped report for reaching the end has gone out; cleared when
  // playback starts again.
  let endReported = false;

  // D22 full screen: the whole player area, so the Skip button and the
  // notice stay in view. The <video>'s own full-screen button would show
  // the bare video, so Chromium-based browsers lose it: `controlslist`
  // turns it (and double-click full screen) off, which in Chrome only
  // greys the button out, and the `::-webkit-media-controls-…` rule
  // below hides it. Firefox and Safari keep theirs. Where the player
  // area can't go full screen (iPhone), nothing is hidden.
  let wrap = $state<HTMLDivElement | null>(null);
  let fullscreen = $state(false);
  const canFullscreen = document.fullscreenEnabled;
  // The full-screen button fades with the browser's own controls: shown
  // while paused, and for a few seconds after playback starts or after
  // pointer / focus activity in the player.
  let paused = $state(true);
  let idle = $state(false);
  let idleTimer: ReturnType<typeof setTimeout> | null = null;

  function report(kind: 'start' | 'progress' | 'stopped') {
    if (previewMs !== null) return;
    if (!info || !video || stoppedByAdmin) return;
    const body: Record<string, unknown> = {
      item_id: info.item_id,
      file_id: info.file_id,
      position_ms: Math.round((video.currentTime || 0) * 1000),
      duration_ms: video.duration ? Math.round(video.duration * 1000) : undefined,
    };
    if (kind === 'start') {
      reported = true;
      body.client_name = 'Parlour Web';
      body.device_name = deviceLabel(navigator.userAgent);
    } else if (sessionId) {
      body.session_id = sessionId;
    }
    void api<{ session_id: string }>(`/api/v1/playback/${kind}`, {
      method: 'POST',
      body: JSON.stringify(body),
      keepalive: kind === 'stopped',
    }).then(({ status, data }) => {
      if (kind === 'start' && data) sessionId = data.session_id;
      // 409: the server stopped this session and revoked its stream.
      if (kind === 'progress' && status === 409) onStoppedByAdmin();
    });
  }

  function onStoppedByAdmin() {
    stoppedByAdmin = true;
    offer = null;
    if (progressTimer) clearInterval(progressTimer);
    progressTimer = null;
    video?.pause();
    hls?.destroy();
    hls = null;
  }

  /** The media's length as the browser measures it, or null while
   *  unknown. */
  function mediaDurationMs(): number | null {
    return video && Number.isFinite(video.duration) ? video.duration * 1000 : null;
  }

  /** Where the playhead is now → the Skip button to show, or (auto
   *  mode, first time there) the segment to skip. Runs on every
   *  timeupdate, seek and play. Once the media has ended nothing is
   *  offered: a segment's end can lie a little past the browser's end. */
  function checkSegments() {
    if (!video || !skipper || !positioned || stoppedByAdmin || video.ended) {
      offer = null;
      return;
    }
    const step = skipper.step(video.currentTime * 1000, !video.paused, mediaDurationMs());
    if (step.skip) {
      skipSegment(step.skip);
      showSkipped(step.skip.kind);
    }
    offer = step.offer;
  }

  /** D22: a skip jumps to the segment's end. Credits that run to the
   *  end of the file end playback there, with the usual stopped report
   *  (played at ≥ 90 %). Playing media stops at its end by itself, but
   *  the report is sent here rather than left to `ended`: Chrome fires
   *  that after a seek to the end only while playing, so a skip made
   *  while paused would otherwise report nothing. */
  function skipSegment(segment: MediaSegment) {
    if (!video) return;
    const landing = skipLanding(segment, mediaDurationMs());
    offer = null;
    if (landing.endsPlayback) {
      video.currentTime = video.duration;
      endPlayback();
    } else {
      video.currentTime = landing.ms / 1000;
    }
  }

  function showSkipped(kind: SegmentKind) {
    skippedNotice = SKIPPED_NOTICES[kind];
    if (noticeTimer) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (skippedNotice = null), SKIPPED_NOTICE_MS);
  }

  function toggleFullscreen() {
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    else if (canFullscreen && wrap) void wrap.requestFullscreen().catch(() => {});
  }

  function onFullscreenChange() {
    fullscreen = wrap !== null && document.fullscreenElement === wrap;
  }

  /** Pointer or focus activity inside the player shows the full-screen
   *  button again; it fades after a few quiet seconds of playback. */
  function onActivity(event: Event) {
    if (!wrap || !(event.target instanceof Node) || !wrap.contains(event.target)) return;
    wake();
  }

  /** Show the full-screen button now; it fades after 3 quiet seconds
   *  while playing. Playback starting counts too, so it also fades when
   *  nothing moved (Space, or a preview that starts by itself). */
  function wake() {
    idle = false;
    if (idleTimer) clearTimeout(idleTimer);
    idleTimer = setTimeout(() => (idle = true), 3000);
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
        if (previewMs !== null && video) {
          video.currentTime = previewMs / 1000;
        } else if (resumeChoice === 'resume' && info && info.resume_position_ms > 0 && video) {
          video.currentTime = info.resume_position_ms / 1000;
        }
        applySubtitleDefault();
        positioned = true;
        checkSegments();
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
    paused = false;
    endReported = false;
    wake();
    // Playback starting inside a segment counts as reaching it (D22).
    checkSegments();
    if (previewMs !== null) return; // a preview reports nothing
    if (!reported) report('start');
    if (!progressTimer) {
      progressTimer = setInterval(() => report('progress'), 10_000);
    }
  }
  function onPause() {
    paused = true;
    report('progress');
  }
  function onEnded() {
    endPlayback();
  }
  /** Playback reached the end of the media — by playing there, or by a
   *  skip that ended it (D22): one stopped report per time it gets
   *  there, even when the browser's own `ended` follows a skip. */
  function endPlayback() {
    paused = true;
    if (endReported) return;
    endReported = true;
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
        toggleFullscreen();
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
      skipper = new Skipper(p.data.segments, playerSkipMode(p.data.segment_skip, previewMs !== null));
      if (previewMs !== null) {
        // Straight to the preview time: no resume prompt.
        started = true;
        await tick();
        await attach();
        video?.play().catch(() => {});
      } else if (p.data.resume_position_ms <= 0) {
        resumeChoice = 'restart';
        started = true;
        // The <video> only exists once `info` has rendered.
        await tick();
        await attach();
      }
    } else {
      error = p.error ?? 'Playback is not available for this item.';
    }
  });

  onDestroy(() => {
    if (progressTimer) clearInterval(progressTimer);
    if (noticeTimer) clearTimeout(noticeTimer);
    if (idleTimer) clearTimeout(idleTimer);
    if (reported) report('stopped');
    hls?.destroy();
  });
</script>

<svelte:window
  onkeydown={onKey}
  onpointermove={onActivity}
  onpointerdown={onActivity}
  onfocusin={onActivity}
/>
<svelte:document onfullscreenchange={onFullscreenChange} />

{#if error}
  <p class="error" role="alert">{error}</p>
{:else if !detail || !info}
  <p class="dim">Loading…</p>
{:else}
  <h1>{detail.title}</h1>
  {#if previewMs !== null}
    <p class="preview" role="status">
      <strong>Preview</strong> from {formatSegmentTime(previewMs)} — nothing you watch here is
      saved to watch history.
      <a class="btn small" href={`/item/${detail.id}#segments`}>Back to segments</a>
    </p>
  {/if}
  <p class="dim plan">
    {info.action === 'DirectPlay' ? 'Direct play' : 'Remuxed for your browser'}
    · {info.reason}
    {#each info.warnings as w (w)}<br /><span class="warn">{w}</span>{/each}
  </p>

  {#if stoppedByAdmin}
    <p class="error" role="alert">
      Playback was stopped by an administrator.
      <button class="btn small" onclick={() => location.reload()}>Play again</button>
    </p>
  {/if}

  <div
    class="player-wrap"
    class:idle={idle && !paused}
    class:own-fullscreen={canFullscreen}
    bind:this={wrap}
  >
    <video
      bind:this={video}
      controls
      controlslist={canFullscreen ? 'nofullscreen' : undefined}
      playsinline
      crossorigin="anonymous"
      onplay={onPlay}
      onpause={onPause}
      onended={onEnded}
      ontimeupdate={checkSegments}
      onseeked={checkSegments}
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
    <div class="skip-layer">
      <p class="skipped" class:shown={skippedNotice !== null} role="status">
        {skippedNotice ?? ''}
      </p>
      {#if offer}
        <button type="button" class="btn primary skip" onclick={() => offer && skipSegment(offer)}>
          {SKIP_LABELS[offer.kind]}
        </button>
      {/if}
    </div>
    {#if canFullscreen}
      <button
        type="button"
        class="fs"
        onclick={toggleFullscreen}
        aria-label={fullscreen ? 'Leave full screen' : 'Full screen'}
        title={fullscreen ? 'Leave full screen (F)' : 'Full screen (F)'}
      >
        <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
          {#if fullscreen}
            <path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5" />
          {:else}
            <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
          {/if}
        </svg>
      </button>
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
  .player-wrap:fullscreen {
    border-radius: 0;
    display: flex;
    align-items: center;
  }
  .player-wrap.idle:fullscreen {
    cursor: none;
  }
  video {
    width: 100%;
    max-height: 78vh;
    display: block;
  }
  .player-wrap:fullscreen video {
    height: 100%;
    max-height: none;
  }
  /* Chromium's own full-screen button (greyed out by `controlslist`). */
  .player-wrap.own-fullscreen video::-webkit-media-controls-fullscreen-button {
    display: none;
  }
  /* Above the browser's control bar, so neither covers the other. */
  .skip-layer {
    position: absolute;
    right: 1.25rem;
    bottom: 4.5rem;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 0.5rem;
    pointer-events: none;
  }
  .skip-layer > * {
    pointer-events: auto;
  }
  .skip {
    font-size: 1.05rem;
    padding: 0.65rem 1.3rem;
    box-shadow: 0 0.3rem 1.2rem rgba(0, 0, 0, 0.55);
  }
  .skipped {
    margin: 0;
    opacity: 0;
    transition: opacity 0.25s;
  }
  .skipped.shown {
    opacity: 1;
    background: rgba(11, 14, 20, 0.86);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.45rem 0.9rem;
  }
  .fs {
    position: absolute;
    top: 0.75rem;
    right: 0.75rem;
    display: grid;
    place-items: center;
    width: 2.6rem;
    height: 2.6rem;
    padding: 0;
    color: #fff;
    background: rgba(11, 14, 20, 0.6);
    border: 1px solid rgba(255, 255, 255, 0.18);
    border-radius: 0.6rem;
    cursor: pointer;
    transition: opacity 0.3s;
  }
  .fs:hover {
    border-color: var(--accent);
  }
  .fs svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .player-wrap.idle .fs:not(:focus-visible) {
    opacity: 0;
    pointer-events: none;
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
  .preview {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    flex-wrap: wrap;
    border: 1px solid var(--accent);
    border-radius: 0.6rem;
    padding: 0.5rem 0.8rem;
  }
  .keys {
    font-size: 0.85rem;
  }
</style>
