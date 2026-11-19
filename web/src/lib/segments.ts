// Segment editor helpers (SEG4, D21): reading and writing segment
// times, the "Preview from here" link, and the words the editor uses
// for kinds, sources and what each stored row means for playback.

import type { SegmentKind, SegmentSource, StoredSegment } from './types';

/** In the order they play in an episode, as the editor lists them. */
export const SEGMENT_KINDS: SegmentKind[] = ['recap', 'intro', 'credits'];

export const KIND_LABELS: Record<SegmentKind, string> = {
  recap: 'Recap',
  intro: 'Intro',
  credits: 'Credits',
};

export const SOURCE_LABELS: Record<SegmentSource, string> = {
  manual: 'Manual',
  chapters: 'Chapters',
  fingerprint: 'Fingerprint',
  heuristic: 'Black frames + silence',
};

/** "1:02:03.45", "2:03", "0:00" — exact to the millisecond, with
 *  trailing zeros dropped, so it reads back as the same time. */
export function formatSegmentTime(ms: number): string {
  const total = Math.max(0, Math.round(ms));
  const h = Math.floor(total / 3_600_000);
  const m = Math.floor((total % 3_600_000) / 60_000);
  const s = Math.floor((total % 60_000) / 1000);
  const frac = total % 1000;
  const sec =
    String(s).padStart(2, '0') +
    (frac ? `.${String(frac).padStart(3, '0').replace(/0+$/, '')}` : '');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

/** A segment's length: "45 s", "12.5 s", or "1:30" from a minute up. */
export function formatSegmentLength(ms: number): string {
  if (ms >= 60_000) return formatSegmentTime(ms);
  const seconds = Math.round(ms / 100) / 10;
  return `${seconds} s`;
}

/** Read a time an admin typed: "90", "90.5", "1:30", "1:30.25",
 *  "1:02:03.5". Minutes and seconds that follow a larger unit must be
 *  under 60, so "1:75" is a typo, not 2:15. Whole milliseconds, or
 *  null for anything else. */
export function parseSegmentTime(text: string): number | null {
  const t = text.trim();
  if (!/^\d+(:\d{1,2}){0,2}(\.\d+)?$/.test(t)) return null;
  const [whole, frac = ''] = t.split('.');
  const parts = whole.split(':').map(Number);
  if (parts.slice(1).some((p) => p >= 60)) return null;
  const seconds = parts.reduce((acc, p) => acc * 60 + p, 0);
  return seconds * 1000 + Math.round(Number(`0.${frac || '0'}`) * 1000);
}

/** "Preview from here" (D21): the player, opened at that time, in a
 *  mode that reports nothing — no watch state, no play count. */
export function previewHref(itemId: string, ms: number): string {
  return `/watch/${itemId}?preview=${Math.max(0, Math.round(ms))}`;
}

/** The player's `?preview=` value in milliseconds, or null for
 *  ordinary playback (absent or not a whole number). */
export function previewParam(value: string | null): number | null {
  if (value === null || !/^\d{1,10}$/.test(value)) return null;
  return Number(value);
}

/** One line on what a stored row means for playback right now. */
export function statusText(row: StoredSegment): string {
  const kind = KIND_LABELS[row.kind].toLowerCase();
  switch (row.status) {
    case 'used':
      return 'Used by the player';
    case 'no_segment':
      return `No ${kind} — the player gets none, whatever detection finds`;
    case 'overridden':
      return 'Not used — the manual times win';
    case 'hidden':
      return `Not used — this item is marked as having no ${kind}`;
    case 'below_floor':
      return 'Not used — detection isn’t confident enough';
    case 'outranked':
      return `Not used — another detected ${kind} is used`;
  }
}
