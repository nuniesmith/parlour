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

export interface ContinueItem extends Item {
  position_ms: number;
  duration_ms: number | null;
}

export interface HomeData {
  continue_watching: ContinueItem[];
  recently_added: Item[];
  next_up: Item[];
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
  warnings: string[];
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
  result: Record<string, number> | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
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

export function posterUrl(item: Item, variant = 'w300'): string {
  return `/api/v1/items/${item.id}/artwork/poster?v=${variant}`;
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

export function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  return `${(n / 1e3).toFixed(0)} KB`;
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
