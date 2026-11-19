// Skip in the player (SEG5, D22): which segment the playhead is in,
// what the player offers or does there under the profile's mode, where
// a skip lands, and the words it uses. Kept free of the DOM so the
// rules read in one place; the player page (`routes/watch/[id]`) wires
// them to the <video>.

import type { MediaSegment, SegmentKind, SegmentSkip } from './types';

/** The Skip button, per kind. */
export const SKIP_LABELS: Record<SegmentKind, string> = {
  recap: 'Skip Recap',
  intro: 'Skip Intro',
  credits: 'Skip Credits',
};

/** The short notice after an automatic skip (D22). */
export const SKIPPED_NOTICES: Record<SegmentKind, string> = {
  recap: 'Skipped the recap',
  intro: 'Skipped the intro',
  credits: 'Skipped the credits',
};

/** How long that notice stays up. */
export const SKIPPED_NOTICE_MS = 4000;

/** A segment that ends this close to the end of the media runs to the
 *  end of the file. Segment times are on ffprobe's clock and the
 *  browser measures the media itself, so the two ends can differ by a
 *  fraction of a second. */
export const END_OF_FILE_SLACK_MS = 1000;

/** The profile's mode as the player applies it. A preview (D21) never
 *  skips by itself and offers the Skip button whatever the profile's
 *  mode (D22). A value this player doesn't know gets the default, the
 *  button. */
export function playerSkipMode(mode: string | null | undefined, preview: boolean): SegmentSkip {
  if (preview) return 'button';
  return mode === 'auto' || mode === 'off' ? mode : 'button';
}

/** The segment the playhead is in: start ≤ ms < end, or null. Kinds
 *  may overlap (D21); then the one that started first is the one at the
 *  playhead, and skipping it lands in the next, which is then offered
 *  or skipped in its turn. */
export function segmentAt(segments: MediaSegment[], ms: number): MediaSegment | null {
  let found: MediaSegment | null = null;
  for (const s of segments) {
    if (s.start_ms <= ms && ms < s.end_ms && (found === null || s.start_ms < found.start_ms)) {
      found = s;
    }
  }
  return found;
}

/** Where a skip lands (D22): the segment's end — or, when the segment
 *  runs to the end of the file, the end of the media, so playback ends
 *  there and the usual stopped report goes out. `durationMs` is the
 *  media's length as the browser has it (null while unknown). */
export function skipLanding(
  segment: MediaSegment,
  durationMs: number | null,
): { ms: number; endsPlayback: boolean } {
  if (
    durationMs !== null &&
    Number.isFinite(durationMs) &&
    segment.end_ms >= durationMs - END_OF_FILE_SLACK_MS
  ) {
    return { ms: durationMs, endsPlayback: true };
  }
  return { ms: segment.end_ms, endsPlayback: false };
}

/** What the player does with the playhead where it is now. */
export interface SkipStep {
  /** Skip this segment now (auto mode, the first time playback reaches it). */
  skip: MediaSegment | null;
  /** Show the Skip button for this segment. */
  offer: MediaSegment | null;
}

/** One playback's skip state (D22). `button` offers the Skip button
 *  while the playhead is inside a segment; `off` offers nothing. `auto`
 *  skips a segment the first time playback reaches it — including when
 *  playback starts inside it — and never again in this playback: back
 *  inside a segment it already skipped, the segment plays and the
 *  button is offered instead. Paused inside a segment it hasn't reached
 *  yet, auto mode waits for playback to start there. */
export class Skipper {
  readonly mode: SegmentSkip;
  private readonly segments: MediaSegment[];
  private readonly skipped = new Set<MediaSegment>();

  constructor(segments: MediaSegment[], mode: SegmentSkip) {
    this.segments = segments.filter((s) => s.end_ms > s.start_ms);
    this.mode = mode;
  }

  /** `ms`: the playhead. `playing`: the media is playing (not paused).
   *  `durationMs`: the media's length as the browser has it (null while
   *  unknown). At the end of the media there is nothing left to skip,
   *  even when a segment's end — on ffprobe's clock — lies a little past
   *  it, as it can when credits run to the end of the file. */
  step(ms: number, playing: boolean, durationMs: number | null = null): SkipStep {
    if (durationMs !== null && Number.isFinite(durationMs) && ms >= durationMs) {
      return { skip: null, offer: null };
    }
    const segment = segmentAt(this.segments, ms);
    if (segment === null || this.mode === 'off') return { skip: null, offer: null };
    if (this.mode === 'auto' && !this.skipped.has(segment)) {
      if (!playing) return { skip: null, offer: null };
      this.skipped.add(segment);
      return { skip: segment, offer: null };
    }
    return { skip: null, offer: segment };
  }
}
