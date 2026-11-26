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
  /** Sidecar opt-in (D14, XFER5): may Parlour write next to the
   *  media? Off by default; set only by its own endpoint. */
  write_sidecars: boolean;
  /** Whether Parlour can write in each root folder right now. */
  folders: FolderAccess[];
}

export interface FolderAccess {
  path: string;
  writable: boolean;
  problem: string | null;
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
  /** 'aac': AB10's audio fallback copy of a book track. */
  delivery: 'file' | 'hls' | 'aac';
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
  /** XFER7: nightly backups (`folder` null = off). D39: set on
   *  Server → Import/Export; `source` says whether an admin saved it
   *  or it is still the environment's default. */
  backups: {
    folder: string | null;
    keep: number;
    include_hashes: boolean;
    source: 'saved' | 'environment';
  };
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
  /** D37: imported without a password; an admin must set one. */
  password_locked: boolean;
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

/** XFER8 / D39: the nightly backup setting (`GET /api/v1/admin/backup`). */
export interface BackupSettingsView {
  /** `saved` = set on Server → Import/Export; `environment` = nothing
   *  saved yet, so PARLOUR_BACKUP_DIR / _KEEP apply. */
  source: 'saved' | 'environment';
  /** null = nightly backups are off. */
  folder: string | null;
  keep: number;
  include_hashes: boolean;
  /** Why the folder can't take a bundle right now; null when it can. */
  folder_problem: string | null;
  updated_at: string | null;
  updated_by: string | null;
  environment: { folder: string | null; keep: number };
}

/** XFER8: one library's sidecar status (`GET /api/v1/admin/sidecars`),
 *  from the ledgers XFER4–XFER6 already keep. */
export interface LibrarySidecars {
  id: string;
  name: string;
  kind: string;
  supported: boolean;
  write_sidecars: boolean;
  folders: FolderAccess[];
  unwritable_notice: boolean;
  items: number;
  sidecars: number;
  sidecars_overridden: number;
  nfos_read: number;
  nfos_written: number;
  artwork_written: number;
}

/** XFER8: an uploaded bundle waiting for its dry run and apply. */
export interface PendingUpload {
  id: string;
  bytes: number;
  uploaded_at: string | null;
}

/** XFER3's import report (`import::Report`): the dry run's answer and
 *  the apply job's result. */
export interface ImportTally {
  created: number;
  updated: number;
  existing: number;
  skipped: number;
}

export interface ImportPath {
  path: string;
  exists_here: boolean;
}

export interface ImportReport {
  dry_run: boolean;
  manifest: {
    format: string;
    format_version: number;
    server_version: string;
    exported_at: string;
    hashes_included: boolean;
    artwork_included: boolean;
  };
  users: {
    username: string;
    is_admin: boolean;
    action: 'create' | 'existing' | 'adopt' | 'conflict';
    sign_in: 'from_bundle' | 'locked' | 'kept' | 'none';
    existing_username: string | null;
    note: string | null;
  }[];
  libraries: {
    name: string;
    kind: string;
    action: 'create' | 'existing' | 'conflict';
    bundle_paths: ImportPath[];
    paths_added: ImportPath[];
    note: string | null;
  }[];
  conflicts: { what: string; key: string; reason: string }[];
  conflicts_total: number;
  profiles: ImportTally;
  access: ImportTally;
  items: ImportTally;
  items_matched_by_provider_id: number;
  people: ImportTally;
  credits: ImportTally;
  segments: ImportTally;
  images: ImportTally;
  artwork_files: ImportTally;
  watch_state: ImportTally;
  recent_searches: ImportTally;
  activity: ImportTally;
  notes: string[];
}

/** An import job's result is the report; anything else isn't. */
export function asImportReport(value: unknown): ImportReport | null {
  const r = value as Partial<ImportReport> | null;
  return r && typeof r === 'object' && r.manifest && Array.isArray(r.users)
    ? (r as ImportReport)
    : null;
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
  import_bundle: 'Server import',
};

export function jobKindLabel(kind: string): string {
  return JOB_KIND_LABELS[kind] ?? kind;
}

/** XFER7: a nightly backup is an `export_bundle` job whose payload
 *  says `trigger: "nightly"`; its bundle lives in the backup folder. */
export function isNightlyBackup(job: Job): boolean {
  const payload = job.payload as { trigger?: unknown } | null;
  return job.kind === 'export_bundle' && payload?.trigger === 'nightly';
}

/** A job's name in lists: the kind's label, or "Nightly backup". */
export function jobLabel(job: Job): string {
  return isNightlyBackup(job) ? 'Nightly backup' : jobKindLabel(job.kind);
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
  /** AB11: hidden from Continue Listening until the position next moves. */
  hidden: boolean;
  /** AB11: when the last writer says it made the update (its own clock). */
  client_updated_at: string | null;
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
  // AB6: the book-only fields, merged from the book's sources and any
  // match, and the match on record.
  subtitle: string | null;
  publisher: string | null;
  isbn: string | null;
  asin: string | null;
  language: string | null;
  google_books_id: string | null;
  open_library_id: string | null;
  match: BookMatch | null;
  // AB7: authors with their author-page ids (same order as
  // `authors`), and the series the book sits in, in its own order.
  author_refs: AuthorRef[];
  series: BookSeriesRef[];
}

/** AB7: an author of a book, and the id of their page. */
export interface AuthorRef {
  id: string;
  name: string;
}

/** AB7 (D28): a series a book sits in, and its place there. */
export interface BookSeriesRef {
  id: string;
  name: string;
  sequence: string | null;
}

/** AB7: one series, as the series list gives it. */
export interface Series {
  id: string;
  library_id: string;
  name: string;
  sort_name: string;
  book_count: number;
  finished_count: number;
  cover_book_id: string | null;
}

/** AB7: a series page — its books in sequence order. */
export interface SeriesDetail extends Series {
  books: Book[];
}

/** AB7: one author, as the authors list gives it. */
export interface Author {
  id: string;
  name: string;
  book_count: number;
}

/** AB7: a series one author's books sit in. */
export interface AuthorSeries {
  id: string;
  library_id: string;
  name: string;
  book_count: number;
}

/** AB7: an author page — their books by title, and their series. */
export interface AuthorDetail extends Author {
  books: Book[];
  series: AuthorSeries[];
}

/** AB7: a book's sequence in one series, as a label ("Book 2"). */
export function sequenceLabel(book: Book, seriesId: string): string | null {
  const seq = book.series.find((s) => s.id === seriesId)?.sequence;
  return seq ? `Book ${seq}` : null;
}

/** AB6: what an admin's match left on a book. */
export interface BookMatch {
  provider: string;
  provider_id: string;
  mode: MatchMode;
  matched_at: string;
}

/** AB6 (D30): how a match lays a provider's values over a book. */
export type MatchMode = 'overwrite' | 'fill_gaps';

/** AB6: the book providers, in D30's order (Google Books is the
 *  default). Audnexus is deliberately absent (its terms). */
export const BOOK_PROVIDERS: { id: string; label: string }[] = [
  { id: 'google', label: 'Google Books' },
  { id: 'openlibrary', label: 'Open Library' },
];

/** AB6: one provider search result. */
export interface BookCandidate {
  provider: string;
  provider_id: string;
  title: string;
  subtitle: string | null;
  authors: string[];
  narrators: string[];
  publisher: string | null;
  year: number | null;
  description: string | null;
  genres: string[];
  isbn: string | null;
  asin: string | null;
  language: string | null;
  cover_url: string | null;
  confidence: number;
}

export interface BookSearchResult {
  provider: string;
  title: string;
  author: string | null;
  results: BookCandidate[];
}

/** AB6: what applying a match did. */
export interface BookMatchReport {
  provider: string;
  provider_id: string;
  mode: MatchMode;
  changed: boolean;
  cover: 'applied' | 'kept' | 'none' | 'failed';
  sidecar_written: boolean;
  cover_written: boolean;
  left_alone: string[];
  errors: string[];
}

/** AB6: an audiobooks library's metadata precedence. */
export interface MetadataPrecedence {
  library_id: string;
  sources: string[];
  default: string[];
  available: string[];
}

/** AB11: an audiobooks library's own settings (Audiobookshelf's). */
export interface AudiobookSettingsValues {
  parse_subtitles: boolean;
  finish_remaining_seconds: number;
  finish_percent: number | null;
}

export interface AudiobookSettings extends AudiobookSettingsValues {
  library_id: string;
  default: AudiobookSettingsValues;
}

/** AB6: what each metadata source is, for the precedence editor. */
export const METADATA_SOURCES: Record<string, string> = {
  folder: 'Folder names (title, author folder)',
  tags: "Audio tags (the first track's)",
  nfo: '.nfo file (Audiobookshelf-style "Key: value")',
  txt: 'desc.txt and reader.txt',
  opf: '.opf file',
  abs_json: "Audiobookshelf's metadata.json",
  sidecar: "Parlour's book.parlour.json, or a match",
};

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
  /** AB10: the decision for the profile the page named (`?client=web`);
   *  null when it named none. */
  plan: BookTrackPlan | null;
}

/** AB10: one track's stream decision. Anything but DirectPlay means
 *  `stream_url` is the AAC fallback, which answers 503 + Retry-After
 *  until the server has made it. */
export interface BookTrackPlan {
  action: 'DirectPlay' | 'Remux' | 'Transcode';
  reason: 'Direct' | 'ContainerUnsupported' | 'CodecUnsupported';
  mode: 'direct_play' | 'copy' | 'transcode';
  delivery: 'file' | 'aac';
  client: string;
}

/** AB10: does this track play through the AAC fallback? */
export function usesFallback(t: BookTrack): boolean {
  return !!t.plan && t.plan.action !== 'DirectPlay';
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

// ------------------------------------------------------------ listening
// AB8 (D29/D35): listening sessions, their stats, and the audiobook
// Home shelves.

/** One listening session (`/api/v1/listening/sessions`, and the admin
 *  list at `/api/v1/admin/listening/sessions`). */
export interface ListeningSession {
  id: string;
  profile_id: string;
  username: string;
  profile_name: string;
  item_id: string;
  title: string;
  has_cover: boolean;
  authors: string[];
  started_at: string;
  last_seen_at: string;
  ended_at: string | null;
  /** 'stopped' | 'replaced' | 'timeout' | 'admin'; null while open. */
  end_reason: string | null;
  start_position_ms: number;
  position_ms: number;
  duration_ms: number | null;
  /** Time actually spent listening (pauses excluded). */
  listened_ms: number;
  client_name: string | null;
  client_version: string | null;
  device_name: string | null;
  /** Open and still reporting: the player is on it now. */
  live: boolean;
}

export interface ListeningSessionPage {
  sessions: ListeningSession[];
  next_before: string | null;
}

export interface ListeningStats {
  time_zone: string;
  total_ms: number;
  sessions: number;
  days_listened: number;
  today_ms: number;
  /** Days with listening, newest first (`YYYY-MM-DD`). */
  days: { date: string; listened_ms: number }[];
  /** Books by listening time, most first. */
  books: {
    item_id: string;
    title: string;
    has_cover: boolean;
    listened_ms: number;
    sessions: number;
    last_listened_at: string;
  }[];
}

/** `GET /api/v1/audiobooks/home`: Audiobookshelf's book shelves. */
export interface AudioShelves {
  continue_listening: Book[];
  continue_series: { series_id: string; series_name: string; book: Book }[];
  recently_added: Book[];
  recent_series: Series[];
  discover: Book[];
  listen_again: Book[];
  newest_authors: Author[];
}

/** The Home Screen rail ids of the book shelves, in default order. */
export const AUDIO_RAILS = [
  'continue_listening',
  'continue_series',
  'recently_added_books',
  'recent_series',
  'discover_books',
  'listen_again',
  'newest_authors',
] as const;
export type AudioRail = (typeof AUDIO_RAILS)[number];

export function isAudioRail(id: string): id is AudioRail {
  return (AUDIO_RAILS as readonly string[]).includes(id);
}

/** "18 min", "2 h 5 min", "45 s" — a listening time. */
export function formatListened(ms: number): string {
  const secs = Math.floor(Math.max(0, ms) / 1000);
  if (secs < 60) return `${secs} s`;
  const mins = Math.floor(secs / 60);
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}
