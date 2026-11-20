// The still-watching prompt (SEG7, D24): how many episodes in a row the
// next-episode countdown has rolled into with nobody at the player, when
// its card asks "Still watching?" instead of counting down, and what
// counts as somebody at the player. The count and the rules for media
// events are kept apart from the page like `autoplay.ts`;
// `watchForViewer` attaches them to the page, and the player page
// (`routes/watch/[id]`) wires it all up.

/** The card asks once the countdown has rolled this many times in a row
 *  with no interaction (D24): the viewer gets the episode they started
 *  plus three autoplayed ones, and the card at the end of the fourth
 *  asks. Fixed: there is no preference for it. */
export const STILL_WATCHING_AFTER = 3;

/** One player page's count (D24). Only rolls the countdown fired count;
 *  any interaction starts the count over — Play now, Keep watching and
 *  Stop included, so their rolls don't count. Once the count reaches
 *  `STILL_WATCHING_AFTER` the card asks instead of counting down, and it
 *  goes on asking until somebody answers. An answer starts the count
 *  over, so it asks again only after that many more rolls. The count
 *  lives as long as the page: a roll keeps it, while a reload, Back /
 *  Forward or another item starts it over. */
export class StillWatching {
  /** This page may ask at all: a preview (D21) never asks. */
  readonly enabled: boolean;
  private rolls = 0;

  constructor(enabled: boolean) {
    this.enabled = enabled;
  }

  /** The card asks "Still watching?" instead of counting down. */
  get asking(): boolean {
    return this.enabled && this.rolls >= STILL_WATCHING_AFTER;
  }

  /** The countdown ran out, and the player rolls into the next episode. */
  countdownFired(): void {
    this.rolls += 1;
  }

  /** Somebody is at the player: the count starts over. Returns whether
   *  that answered the card (it was asking), so the player can turn the
   *  card back into the countdown. */
  interaction(): boolean {
    const answered = this.asking;
    this.rolls = 0;
    return answered;
  }
}

/** The `<video>` events `OwnActions` sorts. */
export type MediaEventType = 'play' | 'pause' | 'seeking' | 'seeked' | 'volumechange';

/** What `OwnActions` reads off the media element. */
export interface MediaFlags {
  readonly paused: boolean;
  readonly ended: boolean;
  readonly seeking: boolean;
}

/** The media element as `OwnActions.adopt` wraps it. */
export interface OwnableMedia extends MediaFlags {
  currentTime: number;
  src: string;
  play(): Promise<void>;
  pause(): void;
  load(): void;
}

/** Tells the player's own plays, pauses and seeks from everybody else's
 *  (D24). The player plays, pauses and seeks the video itself — the
 *  roll's pause and play, auto-skip, the resume seek — and so does
 *  hls.js, which seeks at the start of a stream and over holes in the
 *  buffer, announcing only some of it. So `adopt` routes every play(),
 *  pause() and `currentTime` change made from this page's script through
 *  here, and each one is expected to cause its event: `play` when it
 *  starts paused media, `pause` when it pauses playing media, `seeking`
 *  when it starts a seek (once the media has loaded). An event nobody
 *  here expected came from outside the page's script — the browser's own
 *  controls, media keys, picture-in-picture — so it is somebody at the
 *  player. The one exception is the pause the browser makes by itself
 *  when the media plays to its end. A new source (`src`, load()) makes
 *  the browser drop the events it still had queued for the element, so
 *  whatever was expected of them is dropped then too. */
export class OwnActions {
  private plays = 0;
  private pauses = 0;
  private seeks = 0;

  /** Route `media`'s play(), pause(), load() and `currentTime` and `src`
   *  setters through here, for every caller in this page's script.
   *  Returns a function that puts them back. */
  adopt(media: OwnableMedia): () => void {
    const play = media.play;
    const pause = media.pause;
    const load = media.load;
    const time = accessor(media, 'currentTime');
    const src = accessor(media, 'src');
    // The browser drops the element's queued events as the new source
    // starts to load — before anything the page does after this call.
    const forget = () => {
      this.plays = 0;
      this.pauses = 0;
      this.seeks = 0;
    };
    media.load = () => {
      forget();
      load.call(media);
    };
    if (src) {
      Object.defineProperty(media, 'src', {
        configurable: true,
        enumerable: true,
        get: () => src.get.call(media),
        set: (value: string) => {
          forget();
          src.set.call(media, value);
        },
      });
    }
    media.play = () => {
      const wasPaused = media.paused;
      const wasEnded = media.ended;
      const result = play.call(media);
      // A play the browser refuses (autoplay rules) changes nothing.
      if (wasPaused && !media.paused) {
        this.plays += 1;
        // Playing from the end seeks back to the start first.
        if (wasEnded && media.seeking) this.seeks += 1;
      }
      return result;
    };
    media.pause = () => {
      const wasPlaying = !media.paused;
      pause.call(media);
      if (wasPlaying && media.paused) this.pauses += 1;
    };
    if (time) {
      Object.defineProperty(media, 'currentTime', {
        configurable: true,
        enumerable: true,
        get: () => time.get.call(media),
        set: (value: number) => {
          time.set.call(media, value);
          // A seek that starts says so at once. Before the media has
          // loaded, setting the time only says where to start: nothing
          // seeks, and no event comes.
          if (media.seeking) this.seeks += 1;
        },
      });
    }
    return () => {
      for (const name of ['play', 'pause', 'load', 'currentTime', 'src']) {
        Reflect.deleteProperty(media, name);
      }
    };
  }

  /** Whether this event on `media` is somebody at the player: the page's
   *  script didn't cause it. Call it for every event of these types, in
   *  the order they arrive. */
  byViewer(type: MediaEventType, media: MediaFlags): boolean {
    switch (type) {
      case 'play':
        if (this.plays === 0) return true;
        this.plays -= 1;
        return false;
      case 'pause':
        // Playing to the end pauses the media, which has ended by then.
        if (media.ended) return false;
        if (this.pauses === 0) return true;
        this.pauses -= 1;
        return false;
      case 'seeking':
        if (this.seeks === 0) return true;
        this.seeks -= 1;
        return false;
      case 'seeked':
        // With no seek left running, every `seeking` has arrived: a seek
        // still expected never started (nowhere to seek to yet).
        if (!media.seeking) this.seeks = 0;
        return false;
      case 'volumechange':
        // The player never changes the volume by itself (M is a key
        // press, which counts on its own).
        return true;
    }
  }
}

/** The getter and setter of `name` on `obj`'s prototype chain. */
function accessor(
  obj: object,
  name: string,
): { get: () => unknown; set: (value: unknown) => void } | null {
  for (let o = Object.getPrototypeOf(obj); o !== null; o = Object.getPrototypeOf(o)) {
    const d = Object.getOwnPropertyDescriptor(o, name);
    if (d) return d.get && d.set ? { get: d.get, set: d.set } : null;
  }
  return null;
}

/** The attribute on the next-episode card's buttons
 *  (`NextEpisodeCard.svelte`). A press on one is that button's own
 *  business: it counts as an interaction when the button acts (D24), not
 *  when the pointer goes down, so an asking card doesn't turn back into
 *  the countdown under the pointer before the click lands. */
export const CARD_BUTTON = 'data-card-button';

/** Calls `onViewer` whenever somebody at the player acts (D24): a key
 *  press, click, tap or wheel turn anywhere on the page — the browser's
 *  own controls and the player's buttons included; a play, pause, seek
 *  or volume change the page's script didn't make (the browser's
 *  controls, media keys, picture-in-picture); picture-in-picture opening
 *  or closing; or full screen changing, since the browser keeps the Esc
 *  that leaves full screen from the page. Moving the pointer doesn't
 *  count: on a TV whose remote moves an on-screen cursor, only a click
 *  does. Returns a function that stops watching. */
export function watchForViewer(video: HTMLVideoElement, onViewer: () => void): () => void {
  const own = new OwnActions();
  const restore = own.adopt(video);
  const onInput = (event: Event) => {
    // On one of the card's buttons a pointer press, a click or Enter
    // makes the button act, and it counts then. Anything else there —
    // Space plays and pauses, a wheel turn — counts now.
    if (
      event.target instanceof Element &&
      event.target.closest(`[${CARD_BUTTON}]`) !== null &&
      (event.type === 'pointerdown' ||
        event.type === 'click' ||
        (event instanceof KeyboardEvent && event.key === 'Enter'))
    ) {
      return;
    }
    onViewer();
  };
  const onMedia = (event: Event) => {
    if (own.byViewer(event.type as MediaEventType, video)) onViewer();
  };
  const onAlways = () => onViewer();
  const inputs = ['keydown', 'pointerdown', 'click', 'wheel'];
  const media: MediaEventType[] = ['play', 'pause', 'seeking', 'seeked', 'volumechange'];
  const pip = ['enterpictureinpicture', 'leavepictureinpicture'];
  // Capture, so a handler that stops an event can't hide it from here.
  const options = { capture: true, passive: true };
  for (const type of inputs) window.addEventListener(type, onInput, options);
  for (const type of media) video.addEventListener(type, onMedia);
  for (const type of pip) video.addEventListener(type, onAlways);
  document.addEventListener('fullscreenchange', onAlways);
  return () => {
    for (const type of inputs) window.removeEventListener(type, onInput, options);
    for (const type of media) video.removeEventListener(type, onMedia);
    for (const type of pip) video.removeEventListener(type, onAlways);
    document.removeEventListener('fullscreenchange', onAlways);
    restore();
  };
}
