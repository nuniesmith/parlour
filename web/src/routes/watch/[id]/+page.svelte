<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import Hls from 'hls.js';
  import { api } from '$lib/api';
  import { Autoplay } from '$lib/autoplay';
  import NextEpisodeCard from '$lib/NextEpisodeCard.svelte';
  import { formatSegmentTime, previewParam } from '$lib/segments';
  import {
    SKIPPED_NOTICE_MS,
    SKIPPED_NOTICES,
    SKIP_LABELS,
    Skipper,
    playerSkipMode,
    runsToEnd,
    skipLanding,
  } from '$lib/skip';
  import { carryView, releaseView } from '$lib/viewKey.svelte';
  import {
    deviceLabel,
    episodeLabel,
    formatDurationMs,
    type ItemDetail,
    type MediaSegment,
    type PlaybackInfo,
    type RailItem,
    type SegmentKind,
  } from '$lib/types';

  let detail = $state<ItemDetail | null>(null);
  let info = $state<PlaybackInfo | null>(null);
  let error = $state<string | null>(null);
  let video = $state<HTMLVideoElement | null>(null);
  let hls: Hls | null = null;
  let started = $state(false);
  let progressTimer: ReturnType<typeof setInterval> | null = null;
  // The start report for the episode playing now has gone out.
  let reported = false;

  // The item this page opened on. A roll (SEG6) swaps later episodes
  // into this same page, so after the first load everything reads
  // `info` and `detail` instead.
  const itemId = page.params.id ?? '';
  // The path the root layout mounted this page under (`$lib/viewKey`).
  const mountPath = page.url.pathname;

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

  // SEG6 autoplay (D23): the countdown card at the credits or the end
  // of the file, and the roll into the next episode. `nextCard` is the
  // time the card has left while it is up; a timer keeps checking the
  // playhead meanwhile, since a paused or ended video sends no
  // timeupdate.
  let autoplay: Autoplay | null = null;
  let nextCard = $state<number | null>(null);
  let countdownTimer: ReturnType<typeof setInterval> | null = null;
  // A roll is under way — the old episode has been stopped, the next one
  // is loading — and where to; or why it could not start.
  let rolling = $state(false);
  let rollingTo = $state.raw<RailItem | null>(null);
  let rollError = $state<string | null>(null);
  let destroyed = false;

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

  /** Report on the episode playing now; resolves once the server has
   *  answered, so a roll can send the next start report after this
   *  episode's stopped one (D23). `positionMs` stands in for the
   *  playhead: a roll out of credits that run to the end of the file
   *  reports the end. Progress only ever follows a start report. Never
   *  rejects: a report that can't reach the server is lost, as before,
   *  and must not stop a roll. */
  function report(kind: 'start' | 'progress' | 'stopped', positionMs?: number): Promise<void> {
    if (previewMs !== null) return Promise.resolve();
    if (!info || !video || stoppedByAdmin) return Promise.resolve();
    if (kind === 'progress' && !reported) return Promise.resolve();
    const sentFor = info.item_id;
    const body: Record<string, unknown> = {
      item_id: info.item_id,
      file_id: info.file_id,
      position_ms: positionMs ?? Math.round((video.currentTime || 0) * 1000),
      duration_ms: video.duration ? Math.round(video.duration * 1000) : undefined,
    };
    if (kind === 'start') {
      reported = true;
      body.client_name = 'Parlour Web';
      body.device_name = deviceLabel(navigator.userAgent);
    } else if (sessionId) {
      body.session_id = sessionId;
    }
    return api<{ session_id: string }>(`/api/v1/playback/${kind}`, {
      method: 'POST',
      body: JSON.stringify(body),
      keepalive: kind === 'stopped',
    })
      .then(({ status, data }) => {
        // Answers about an episode the player has since rolled on from
        // are stale.
        if (info?.item_id !== sentFor) return;
        if (kind === 'start' && data) sessionId = data.session_id;
        // 409: the server stopped this session and revoked its stream.
        if (kind === 'progress' && status === 409) onStoppedByAdmin();
      })
      .catch(() => {});
  }

  /** The start report goes out once the episode is at its opening
   *  position and playing — so a resume, or a roll into a partly
   *  watched episode, reports where it really starts — then progress
   *  every 10 s. A preview reports nothing. */
  function startReports() {
    if (previewMs !== null || rolling || !positioned || !video || video.paused) return;
    if (!reported) void report('start');
    if (!progressTimer) progressTimer = setInterval(() => void report('progress'), 10_000);
  }

  function onStoppedByAdmin() {
    stoppedByAdmin = true;
    offer = null;
    setCard(null);
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

  /** The media is at its end: ended, or the playhead is there — Chrome
   *  leaves `ended` false after a seek to the end made while paused. */
  function atMediaEnd(): boolean {
    if (!video) return false;
    const durationMs = mediaDurationMs();
    return video.ended || (durationMs !== null && video.currentTime * 1000 >= durationMs);
  }

  /** Where the playhead is now → the countdown card (SEG6), the Skip
   *  button to show, or (auto mode, first time there) the segment to
   *  skip. Runs on every timeupdate, seek, play, pause and end, and on a
   *  timer while the card is up. While autoplay is on, the card stands in
   *  for Skip Credits and the automatic credits skip; after Cancel the
   *  credits get the button whatever the mode (D23). Once the media has
   *  ended no Skip button is offered: a segment's end can lie a little
   *  past the browser's end. */
  function checkPlayhead() {
    if (!video || !skipper || !autoplay || !positioned || stoppedByAdmin || rolling) {
      offer = null;
      setCard(null);
      return;
    }
    const ms = video.currentTime * 1000;
    const durationMs = mediaDurationMs();
    const atEnd = atMediaEnd();
    const card = autoplay.step(performance.now(), {
      ms,
      playing: !video.paused && !atEnd,
      atEnd,
      durationMs,
    });
    if (card.fire) {
      void rollToNext();
      return;
    }
    setCard(card.remainingMs);
    if (atEnd) {
      offer = null;
      return;
    }
    const step = skipper.step(ms, !video.paused, durationMs, autoplay.creditsRule());
    if (step.skip) {
      skipSegment(step.skip);
      showSkipped(step.skip.kind);
    }
    offer = step.offer;
  }

  /** Put the countdown card up with this much time left, or take it
   *  down (null). While it is up a timer keeps the count going. */
  function setCard(remainingMs: number | null) {
    nextCard = remainingMs;
    if (remainingMs !== null && countdownTimer === null) {
      countdownTimer = setInterval(checkPlayhead, 250);
    } else if (remainingMs === null && countdownTimer !== null) {
      clearInterval(countdownTimer);
      countdownTimer = null;
    }
  }

  function playNextNow() {
    if (autoplay?.playNow()) void rollToNext();
  }

  /** Cancel (D23): no card again in this playback, and the credits get
   *  the Skip button whatever the profile's mode. */
  function cancelAutoplay() {
    autoplay?.cancel();
    checkPlayhead();
  }

  /** "S1:E3 · Title" for the next episode, or its title alone. */
  function nextLabel(next: RailItem): string {
    return episodeLabel(next.season_number, next.episode_number, next.title) ?? next.title;
  }

  /** The roll (SEG6, D23), on Play now or when the countdown runs out.
   *  This episode's stopped report goes first: at the end of the file
   *  when the credits run there — as D22's Skip Credits reports it, so
   *  the episode counts as played — otherwise where the playhead is.
   *  Then the next episode loads into this same player, so full screen
   *  stays: a partly watched one from where the viewer left off, with no
   *  resume prompt, a played one from the beginning. Its start report
   *  goes out once it plays from there. The URL moves to it, replacing
   *  this history entry, without remounting the page. */
  async function rollToNext() {
    const next = info?.next_episode;
    if (!video || !info || !next || rolling || destroyed) return;
    rolling = true;
    rollingTo = next;
    rollError = null;
    offer = null;
    skippedNotice = null;
    setCard(null);
    if (progressTimer) clearInterval(progressTimer);
    progressTimer = null;
    video.pause();

    const durationMs = mediaDurationMs();
    const credits = info.segments.find((s) => s.kind === 'credits' && s.end_ms > s.start_ms);
    const inCreditsToEnd =
      credits !== undefined &&
      video.currentTime * 1000 >= credits.start_ms &&
      runsToEnd(credits, durationMs);
    if (durationMs !== null && (atMediaEnd() || inCreditsToEnd)) {
      if (!endReported) {
        endReported = true;
        await report('stopped', Math.round(durationMs));
      }
    } else if (reported) {
      await report('stopped');
    }
    reported = false;
    sessionId = null;
    if (destroyed) return;

    // The next episode, fresh from the server: its stream, its segments,
    // its own next episode, and the profile's choices as they are now.
    // If that fails — the server refuses, or can't be reached — this
    // episode stays, paused, and the overlay says why.
    const loaded = await Promise.all([
      api<ItemDetail>(`/api/v1/items/${next.id}`),
      api<PlaybackInfo>(`/api/v1/items/${next.id}/playback-info?client=web&network=lan`),
    ]).then(
      ([d, p]) =>
        d.data && p.data
          ? { detail: d.data, info: p.data }
          : (p.error ?? d.error ?? 'It could not be loaded.'),
      (e: unknown) => (e instanceof Error ? e.message : String(e)),
    );
    if (destroyed) return;
    if (typeof loaded === 'string') {
      rolling = false;
      rollError = loaded;
      return;
    }
    const p = loaded.info;
    hls?.destroy();
    hls = null;
    detail = loaded.detail;
    info = p;
    skipper = new Skipper(p.segments, playerSkipMode(p.segment_skip, false));
    autoplay = new Autoplay(p.segments, p.autoplay_next && p.next_episode !== null);
    positioned = false;
    endReported = false;
    const path = `/watch/${p.item_id}`;
    carryView(mountPath, path);
    void goto(path, { replaceState: true, noScroll: true, keepFocus: true });
    await tick(); // the next episode's subtitle <track>s
    try {
      await attach(p.resume_position_ms > 0 ? p.resume_position_ms : null);
    } catch (e) {
      if (destroyed) return;
      // The page is the next episode's now; nothing of the old one may
      // go on playing under its name.
      video.removeAttribute('src');
      video.load();
      rolling = false;
      rollError = e instanceof Error ? e.message : String(e);
      return;
    }
    if (destroyed) return;
    rolling = false;
    rollingTo = null;
    video.play().catch(() => {});
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
    for (let i = 0; i < 45 && !destroyed; i++) {
      const res = await fetch(url, { credentials: 'same-origin' });
      if (res.ok) return;
      await new Promise((r) => setTimeout(r, 2000));
    }
    if (destroyed) return;
    throw new Error('The remux is taking too long — try again in a bit.');
  }

  /** Load `info`'s stream into the player. Once its metadata is in, seek
   *  to `startMs` (a resume point or preview time; null for the
   *  beginning), pick the subtitle, and let skips, the card and the
   *  start report go. */
  async function attach(startMs: number | null) {
    if (!video || !info) return;
    const url = info.stream_url;
    if (info.stream_kind === 'hls') {
      await waitForPlaylist(url);
      if (destroyed || !video) return;
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
        if (startMs !== null && startMs > 0 && video) video.currentTime = startMs / 1000;
        applySubtitleDefault();
        positioned = true;
        checkPlayhead();
        startReports();
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
    started = true;
    const startMs = choice === 'resume' && info ? info.resume_position_ms : null;
    void attach(startMs).then(() => video?.play().catch(() => {}));
  }

  function onPlay() {
    paused = false;
    endReported = false;
    wake();
    // Playback starting inside a segment counts as reaching it (D22).
    checkPlayhead();
    startReports();
  }
  function onPause() {
    paused = true;
    // The countdown waits while the video is paused (D23).
    checkPlayhead();
    if (!rolling) void report('progress');
  }
  function onEnded() {
    if (rolling) return;
    endPlayback();
    // ...and keeps running once it has ended.
    checkPlayhead();
  }
  /** Playback reached the end of the media — by playing there, or by a
   *  skip that ended it (D22): one stopped report per time it gets
   *  there, even when the browser's own `ended` follows a skip. */
  function endPlayback() {
    paused = true;
    if (endReported) return;
    endReported = true;
    void report('stopped');
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
        // Mid-roll the old episode is done and the next is loading.
        if (rolling) break;
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
      // A preview gets no card and no autoplay (D23).
      autoplay = new Autoplay(
        p.data.segments,
        previewMs === null && p.data.autoplay_next && p.data.next_episode !== null,
      );
      if (previewMs !== null) {
        // Straight to the preview time: no resume prompt.
        started = true;
        await tick();
        await attach(previewMs);
        video?.play().catch(() => {});
      } else if (p.data.resume_position_ms <= 0) {
        started = true;
        // The <video> only exists once `info` has rendered.
        await tick();
        await attach(null);
      }
    } else {
      error = p.error ?? 'Playback is not available for this item.';
    }
  });

  onDestroy(() => {
    destroyed = true;
    if (progressTimer) clearInterval(progressTimer);
    if (countdownTimer) clearInterval(countdownTimer);
    if (noticeTimer) clearTimeout(noticeTimer);
    if (idleTimer) clearTimeout(idleTimer);
    if (reported) void report('stopped');
    hls?.destroy();
    releaseView(mountPath);
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
      ontimeupdate={checkPlayhead}
      onseeked={checkPlayhead}
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
    {#if rolling && rollingTo}
      <div class="rolling" role="status">
        <p>Starting {nextLabel(rollingTo)}…</p>
      </div>
    {:else if rollError && rollingTo}
      <div class="rolling" role="alert">
        <p>The next episode, {nextLabel(rollingTo)}, didn't start: {rollError}</p>
        <div class="row">
          <a class="btn" href={`/item/${rollingTo.id}`}>Open it</a>
          <button type="button" class="btn" onclick={() => (rollError = null)}>Close</button>
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
      {#if nextCard !== null && info.next_episode}
        <NextEpisodeCard
          next={info.next_episode}
          remainingMs={nextCard}
          onplaynow={playNextNow}
          oncancel={cancelAutoplay}
        />
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
  /* Above the browser's control bar, so neither covers the other. It
     spans the player, so the next-episode card can size itself to it;
     what it holds sits at the right. */
  .skip-layer {
    position: absolute;
    left: 1.25rem;
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
  .resume,
  .rolling {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    align-content: center;
    gap: 0.8rem;
    background: rgba(0, 0, 0, 0.72);
    text-align: center;
  }
  .rolling {
    padding: 1rem;
  }
  .rolling p {
    margin: 0;
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
