// Shared API types (mirror the server DTOs).

export interface User {
  id: string;
  username: string;
  is_admin: boolean;
}

export interface Profile {
  id: string;
  name: string;
  has_pin: boolean;
  avatar_color: string | null;
  initial: string;
  audio_lang: string | null;
  subtitle_lang: string | null;
  subtitle_mode: string;
  remote_bitrate_cap_kbps: number | null;
  /** Home Screen rails in display order (UI7). */
  home_rails: HomeRail[];
  /** Inside an intro, recap or credits segment (SEG5, D22). */
  segment_skip: SegmentSkip;
  /** Next-episode countdown (SEG6). */
  autoplay_next: boolean;
}

export interface HomeRail {
  id: string;
  visible: boolean;
}

export interface Me {
  user: User;
  active_profile: Profile | null;
}

export interface Library {
  id: string;
  name: string;
  kind: string;
  created_at: string;
}

export interface LibraryDetail extends Library {
  paths: string[];
}

export interface Item {
  id: string;
  library_id: string;
  parent_id: string | null;
  kind: string;
  title: string;
  sort_title: string;
  year: number | null;
  season_number: number | null;
  episode_number: number | null;
  overview: string | null;
  content_rating: string | null;
  genres: string[];
  vote_average: number | null;
  metadata_status: string;
  metadata_provider: string | null;
  has_poster: boolean;
  available: boolean;
  created_at: string;
  updated_at: string;
}

export interface StreamInfo {
  stream_index: number;
  kind: string;
  codec: string | null;
  width: number | null;
  height: number | null;
  hdr_format: string | null;
  language: string | null;
  title: string | null;
  channels: number | null;
  is_default: boolean;
}

export interface SubtitleInfo {
  stream_index: number | null;
  source: string;
  path: string | null;
  format: string | null;
  language: string | null;
  title: string | null;
  is_forced: boolean;
  is_default: boolean;
  is_sdh: boolean;
}

export interface FileInfo {
  id: string;
  path: string;
  relative_path: string;
  size_bytes: number;
  container: string | null;
  duration_ms: number | null;
  probe_error: string | null;
  available: boolean;
  streams: StreamInfo[];
  subtitles: SubtitleInfo[];
}

export interface ImageInfo {
  kind: string;
  variant: string;
  path: string;
  source_url: string | null;
}

export interface CastMember {
  name: string;
  character_name: string | null;
  sort_order: number;
}

export interface ItemDetail extends Item {
  children: Item[];
  files: FileInfo[];
  images: ImageInfo[];
  cast: CastMember[];
  similar: Item[];
  user_state: UserState | null;
}

/** Home rail rows that can be episodes (UI11): the series title,
 *  and which item's poster to show — the series' when it has one,
 *  else the item's own, null when neither has art. */
export interface RailItem extends Item {
  series_title?: string | null;
  poster_item_id?: string | null;
}

export interface ContinueItem extends RailItem {
  position_ms: number;
  duration_ms: number | null;
}

/** A tile in Home's library row (UI8). */
export interface LibraryTile {
  id: string;
  name: string;
  kind: string;
  item_count: number;
  /** Item whose backdrop the tile shows; null → a plain tile. */
  art_item_id: string | null;
}

/** One per-library Latest rail on Home (UI8). */
export interface LatestRail {
  library_id: string;
  library_name: string;
  library_kind: string;
  items: Item[];
}

export interface HomeData {
  continue_watching: ContinueItem[];
  recently_added: Item[];
  next_up: RailItem[];
  libraries: LibraryTile[];
  latest: LatestRail[];
}

export interface UserState {
  played: boolean;
  is_favorite: boolean;
  position_ms: number;
  play_count: number;
}

export interface SubtitleTrackInfo {
  id: string;
  language: string | null;
  title: string | null;
  is_forced: boolean;
  is_default: boolean;
  is_sdh: boolean;
  source: string;
  text: boolean;
  url: string | null;
}

export interface AudioStreamInfo {
  stream_index: number;
  codec: string | null;
  language: string | null;
  is_default: boolean;
}

export interface PlaybackInfo {
  item_id: string;
  file_id: string;
  action: string;
  reason: string;
  stream_kind: string;
  stream_url: string;
  resume_position_ms: number;
  played: boolean;
  subtitles: SubtitleTrackInfo[];
  default_subtitle_id: string | null;
  audio_streams: AudioStreamInfo[];
  /** SEG1: at most one per kind, start-ordered; manual wins over auto.
   *  Automatic rows below confidence 0.6 are never sent (D17), so a
   *  `heuristic` guess (stored at 0.5) does not appear here today. */
  segments: MediaSegment[];
  /** The active profile's choice for those segments (SEG5), read
   *  fresh with every playback. */
  segment_skip: SegmentSkip;
  /** The episode after this one in series order (SEG6, D23) — watched
   *  or not, skipping any with no available file — shaped like a Next
   *  Up row. Null for a movie and for the last episode. */
  next_episode: RailItem | null;
  /** Whether the active profile wants it played after the countdown
   *  card (SEG6, Preferences → Playback), read fresh with every
   *  playback. */
  autoplay_next: boolean;
  warnings: string[];
}

/** What the player does inside a segment (SEG5, D22): offer a Skip
 *  button, skip it automatically, or neither. */
export type SegmentSkip = 'button' | 'auto' | 'off';

export type SegmentKind = 'intro' | 'credits' | 'recap';
export type SegmentSource = 'chapters' | 'fingerprint' | 'manual' | 'heuristic';

export interface MediaSegment {
  kind: SegmentKind;
  start_ms: number;
  end_ms: number;
  source: SegmentSource;
  confidence: number;
}

/** The SEG4 editor's view of one item (`GET /api/v1/admin/items/{id}/segments`). */
export interface SegmentEditorView {
  item_id: string;
  /** The file playback serves — the times are into it. Null when the
   *  item has no playable file: nothing can be added then. */
  file: { id: string; relative_path: string; duration_ms: number | null } | null;
  /** Every stored row, kind by kind (recap, intro, credits), each
   *  kind's manual row first, then the detectors. */
  rows: StoredSegment[];
}

/** One stored `media_segments` row and what it means for playback now. */
export interface StoredSegment {
  kind: SegmentKind;
  source: SegmentSource;
  /** Both null on a manual "no segment of this kind here" marker (D21). */
  start_ms: number | null;
  end_ms: number | null;
  confidence: number;
  status: 'used' | 'no_segment' | 'overridden' | 'hidden' | 'below_floor' | 'outranked';
  updated_at: string;
}

export interface Job {
  id: string;
  kind: string;
  library_id: string | null;
  payload: unknown;
  status: string;
  progress_done: number;
  progress_total: number;
  errors: { path: string | null; message: string }[];
  result: Record<string, unknown> | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

export interface StreamPlan {
  action: string;
  reason: string;
  delivery: 'file' | 'hls';
  client: string;
  network: string;
  container: string | null;
  bitrate_kbps: number | null;
  video: {
    codec: string | null;
    width: number | null;
    height: number | null;
    hdr_format: string | null;
    mode: string;
  } | null;
  audio: {
    codec: string | null;
    language: string | null;
    channels: number | null;
    is_default: boolean;
    mode: string;
  } | null;
}

export interface PlaybackSession {
  session_id: string;
  username: string;
  profile_name: string;
  item_id: string;
  item_title: string;
  item_kind: string;
  series_title: string | null;
  season_number: number | null;
  episode_number: number | null;
  poster_item_id: string | null;
  started_at: string;
  last_seen: string;
  position_ms: number;
  duration_ms: number | null;
  client: {
    name: string | null;
    version: string | null;
    device: string | null;
    user_agent: string | null;
  };
  remote: { ip: string | null; forwarded_for: string | null; protocol: string | null };
  plan: StreamPlan | null;
}

export interface ActivityEntry {
  id: string;
  created_at: string;
  kind: string;
  user_id: string | null;
  profile_id: string | null;
  item_id: string | null;
  actor: string | null;
  summary: string;
  detail: Record<string, unknown>;
}

export interface ActivityPage {
  entries: ActivityEntry[];
  next_before: string | null;
}

export interface Notice {
  id: string;
  kind: string;
  severity: 'info' | 'warning' | 'error';
  summary: string;
  detail: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface ServerInfo {
  name: string;
  version: string;
  port: number;
  db: 'ok' | 'unreachable';
  urls: { lan: string | null; tailnet: string | null; public: string | null };
  jobs: {
    queued: number;
    running: number;
    failed_recent: number;
    succeeded_recent: number;
  } | null;
  actions: { restart: boolean; shutdown: boolean };
}

export interface Candidate {
  provider: string;
  provider_id: number;
  title: string;
  year: number | null;
  overview: string | null;
  poster_url: string | null;
  confidence: number;
}

export interface ReviewItem {
  id: string;
  library_id: string;
  kind: string;
  title: string;
  year: number | null;
  candidates: Candidate[];
}

export interface AdminUser {
  id: string;
  username: string;
  is_admin: boolean;
  created_at: string;
  profile_count: number;
}

export interface Invite {
  id: string;
  created_at: string;
  expires_at: string;
  used_at: string | null;
  used_by: string | null;
}

export function posterUrl(itemId: string, variant = 'w300'): string {
  return `/api/v1/items/${itemId}/artwork/poster?v=${variant}`;
}

export function backdropUrl(item: Item): string {
  return `/api/v1/items/${item.id}/artwork/backdrop`;
}

export function formatRuntime(min: number | null | undefined): string {
  if (!min) return '';
  const h = Math.floor(min / 60);
  const m = min % 60;
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

/** XFER2: what opting into artwork would add to an export now
 *  (`GET /api/v1/admin/export/estimate`). */
export interface ExportEstimate {
  artwork_files: number;
  artwork_bytes: number;
  /** Cached images whose file is gone or can't be named; they
   *  travel as provider references only. */
  artwork_unavailable: number;
}

export function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  return `${(n / 1e3).toFixed(0)} KB`;
}

/** "S1:E2 · Pilot" — the episode line on posters and the Now
 *  Playing card. Null when neither number nor title is known. */
export function episodeLabel(
  season: number | null,
  episode: number | null,
  title: string | null,
): string | null {
  const where = season !== null && episode !== null ? `S${season}:E${episode}` : null;
  return [where, title].filter(Boolean).join(' · ') || null;
}

/** One timestamp style everywhere on the admin pages:
 *  "Nov 15, 2026, 3:55 AM" in the viewer's locale. */
export function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** Readable names for job kinds (Dashboard + Server → Jobs). */
export const JOB_KIND_LABELS: Record<string, string> = {
  scan_library: 'Library scan',
  fetch_metadata: 'Metadata match',
  download_artwork: 'Artwork download',
  import_emby: 'Emby import',
  purge_activity: 'Activity clean-up',
  detect_segments: 'Segment detection',
  export_bundle: 'Server export',
};

export function jobKindLabel(kind: string): string {
  return JOB_KIND_LABELS[kind] ?? kind;
}

/** Container names as people know them. The server stores
 *  ffprobe's first format name, which is the family, not the
 *  file type: `mov` covers both MP4 and MOV files. */
const CONTAINER_LABELS: Record<string, string> = {
  matroska: 'MKV',
  mov: 'MP4/MOV',
  mpegts: 'TS',
  avi: 'AVI',
  webm: 'WebM',
};

export function containerLabel(name: string | null): string {
  if (!name) return 'Unknown';
  return CONTAINER_LABELS[name.toLowerCase()] ?? name.toUpperCase();
}

export function formatDurationMs(ms: number | null): string {
  if (!ms) return '';
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
    : `${m}:${String(s).padStart(2, '0')}`;
}

/** A short "Browser on OS" label for the Now Playing card. Display
 *  only — the server never derives capabilities from it. */
export function deviceLabel(ua: string): string {
  const browser = /Silk\//.test(ua)
    ? 'Silk'
    : /Edg\//.test(ua)
      ? 'Edge'
      : /Firefox\//.test(ua)
        ? 'Firefox'
        : /Chrome\//.test(ua)
          ? 'Chrome'
          : /Safari\//.test(ua)
            ? 'Safari'
            : 'Browser';
  const os = /Android/.test(ua)
    ? 'Android'
    : /iPhone|iPad|iPod/.test(ua)
      ? 'iOS'
      : /Windows/.test(ua)
        ? 'Windows'
        : /Mac OS X/.test(ua)
          ? 'macOS'
          : /Linux/.test(ua)
            ? 'Linux'
            : '';
  return os ? `${browser} on ${os}` : browser;
}

// ------------------------------------------------------------ audiobooks
// Phase 10 (AB3/AB4): `/api/v1/audiobooks`. Positions are
// book-global milliseconds from the start of track 0.

export interface BookProgress {
  position_ms: number;
  duration_ms: number | null;
  finished: boolean;
  finished_at: string | null;
  last_listened_at: string | null;
  updated_at: string | null;
}

export interface Book {
  id: string;
  library_id: string;
  title: string;
  sort_title: string;
  year: number | null;
  overview: string | null;
  genres: string[];
  authors: string[];
  narrators: string[];
  duration_ms: number;
  track_count: number;
  chapter_count: number;
  has_cover: boolean;
  available: boolean;
  created_at: string;
  progress: BookProgress;
}

export interface BookTrack {
  index: number;
  file_id: string;
  title: string;
  start_ms: number;
  duration_ms: number | null;
  container: string | null;
  codec: string | null;
  content_type: string;
  size_bytes: number;
  stream_url: string;
}

export interface BookChapter {
  index: number;
  title: string;
  start_ms: number;
  end_ms: number;
}

export interface BookDetail extends Book {
  tracks: BookTrack[];
  chapters: BookChapter[];
}

/** Where an item opens: books have their own page. */
export function itemHref(item: { id: string; kind: string }): string {
  return item.kind === 'audiobook' ? `/book/${item.id}` : `/item/${item.id}`;
}

/** Which track holds a book-global position, and how far into it:
 *  the last track starting at or before it (positions past the end
 *  land at the end of the last track). */
export function locateInBook(
  tracks: BookTrack[],
  positionMs: number,
): { track: number; offsetMs: number } {
  if (tracks.length === 0) return { track: 0, offsetMs: 0 };
  let i = 0;
  while (i + 1 < tracks.length && tracks[i + 1].start_ms <= positionMs) i++;
  const len = tracks[i].duration_ms ?? Infinity;
  return { track: i, offsetMs: Math.max(0, Math.min(positionMs - tracks[i].start_ms, len)) };
}

/** The chapter playing at a book-global position, or -1. */
export function chapterAt(chapters: BookChapter[], positionMs: number): number {
  let found = -1;
  for (let i = 0; i < chapters.length; i++) {
    if (chapters[i].start_ms <= positionMs) found = i;
    else break;
  }
  return found;
}
