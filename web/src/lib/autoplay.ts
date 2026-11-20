// Autoplay next episode (SEG6, D23): when the countdown card is up, how
// long it has left, when it fires, and what that leaves the credits.
// While the card asks "Still watching?" instead (SEG7, `stillWatching.ts`)
// it is held: up, but not counting. Kept free of the DOM like `skip.ts`;
// the player page (`routes/watch/[id]`) wires it to the <video> and does
// the roll.

import { runsToEnd, type CreditsRule } from './skip';
import type { MediaSegment } from './types';

/** How long the card counts down before the next episode plays (D15). */
export const COUNTDOWN_MS = 10_000;

/** The playhead as the card needs it. */
export interface Playhead {
  /** Where the playhead is, in ms. */
  ms: number;
  /** The media is playing: not paused, not at its end. */
  playing: boolean;
  /** The media is at its end — ended, or the playhead is there (Chrome
   *  leaves `ended` false after a paused seek to the end). */
  atEnd: boolean;
  /** The media's length as the browser has it (null while unknown). */
  durationMs: number | null;
}

/** The card after a step. */
export interface CountdownStep {
  /** The card is up with this many ms left; null when it isn't. */
  remainingMs: number | null;
  /** The countdown ran out: play the next episode now. */
  fire: boolean;
}

/** One episode's autoplay (D23). With autoplay on and a next episode to
 *  play, the card is up while the playhead is in the credits — from
 *  their start to their end, or on to the end of the file when they run
 *  there — and once the media has ended. It counts down while the video
 *  plays and once it has ended, and waits while the video is paused.
 *  Seeking back out of that stretch takes the card down; coming back
 *  starts the count afresh. Play now, or the count running out, fires
 *  once. Cancel turns autoplay off for the rest of this playback: the
 *  card never comes back, and the credits get the Skip button whatever
 *  the profile's mode. */
export class Autoplay {
  /** Autoplay applies to this playback at all: the profile has it on,
   *  there is a next episode, and this is not a preview. */
  readonly enabled: boolean;
  private readonly credits: MediaSegment | null;
  private cancelledFlag = false;
  private firedFlag = false;
  private remaining: number | null = null;
  private lastAt = 0;
  private lastRunning = false;

  constructor(segments: MediaSegment[], enabled: boolean) {
    this.credits = segments.find((s) => s.kind === 'credits' && s.end_ms > s.start_ms) ?? null;
    this.enabled = enabled;
  }

  /** Cancel was pressed in this playback. */
  get cancelled(): boolean {
    return this.cancelledFlag;
  }

  /** What the credits get now (D23): while autoplay is on, the card
   *  stands in for Skip Credits and the automatic skip; after Cancel,
   *  the Skip button. */
  creditsRule(): CreditsRule {
    if (!this.enabled || this.firedFlag) return 'mode';
    return this.cancelledFlag ? 'button' : 'card';
  }

  /** Whether the playhead is where the card belongs. */
  private inStretch(at: Playhead): boolean {
    if (at.atEnd) return true;
    const c = this.credits;
    if (c === null || at.ms < c.start_ms) return false;
    return at.ms < c.end_ms || runsToEnd(c, at.durationMs);
  }

  /** Advance to `now` (ms, a monotonic clock) with the playhead as it
   *  is now. The time since the last step counts only if the video was
   *  running then — playing, or at its end — so call this on every
   *  play, pause, seek and end as well as on a timer while the card is
   *  up. `hold`: the card is asking "Still watching?" (SEG7, D24) — it is
   *  up at the same moments, but nothing counts and nothing fires, for as
   *  long as it takes; once the hold ends, the count starts from the full
   *  10 s. Play now and Cancel work as usual meanwhile. */
  step(now: number, at: Playhead, hold = false): CountdownStep {
    if (this.creditsRule() !== 'card' || !this.inStretch(at)) {
      this.remaining = null;
      return { remainingMs: null, fire: false };
    }
    if (this.remaining === null || hold) {
      this.remaining = COUNTDOWN_MS;
    } else if (this.lastRunning) {
      this.remaining = Math.max(0, this.remaining - Math.max(0, now - this.lastAt));
    }
    this.lastAt = now;
    this.lastRunning = !hold && (at.playing || at.atEnd);
    if (this.remaining <= 0) {
      this.firedFlag = true;
      this.remaining = null;
      return { remainingMs: null, fire: true };
    }
    return { remainingMs: this.remaining, fire: false };
  }

  /** Play now: fires if the card is up. */
  playNow(): boolean {
    if (this.creditsRule() !== 'card' || this.remaining === null) return false;
    this.firedFlag = true;
    this.remaining = null;
    return true;
  }

  /** Cancel: no card again in this playback. */
  cancel(): void {
    if (!this.enabled || this.firedFlag) return;
    this.cancelledFlag = true;
    this.remaining = null;
  }
}
