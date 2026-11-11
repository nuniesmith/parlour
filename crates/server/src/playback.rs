//! Playback surface (Milestone 1.6).
//!
//! - `GET /api/v1/items/{id}/playback-info` — the plan (P1)
//! - `GET /api/v1/stream/{file_id}` — Range streaming (P2)
//! - `GET /api/v1/stream/{file_id}/hls/{asset}` — remux (P3)
//! - `POST /api/v1/playback/{start,progress,stopped}` — reporting
//!   into `user_data` + the live-session registry (P4, P8)
//! - `POST /api/v1/items/{id}/{played,favorite}` — marks (P5)
//! - `GET /api/v1/subtitles/{track_id}` — WebVTT extraction (P6)
//!
//! Media URLs carry a short-lived HMAC stream token (file +
//! profile + expiry) so a leaked URL is neither permanent nor a
//! skeleton key; a valid session cookie + library access also
//! works for the same URLs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::FromRow;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use parlour_core::{ClientKind, NetworkKind};
use parlour_playback::{decide, pick_subtitle, FileFacts, SubtitleChoice};

use crate::auth::{AdminUser, AuthUser};

/// Optional-auth extractor: media URLs are authorised by stream
/// token *or* session cookie, so a missing/invalid session must
/// not reject the request before the token is checked. (axum's
/// `Option<T>` blanket impl doesn't cover stateful extractors.)
pub struct MaybeUser(pub Option<AuthUser>);

impl axum::extract::FromRequestParts<AppState> for MaybeUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(AuthUser::from_request_parts(parts, state).await.ok()))
    }
}
use crate::error::AppError;
use crate::http::AppState;

type HmacSha256 = Hmac<Sha256>;

const TOKEN_TTL_SECS: i64 = 6 * 3600;
const TEXT_SUBTITLE_FORMATS: &[&str] = &[
    "srt", "subrip", "ass", "ssa", "vtt", "webvtt", "mov_text", "text", "sub",
];

// ---------------------------------------------------------------------
// Shared playback state (lives on AppState)

#[derive(Clone)]
pub struct SessionInfo {
    pub profile_id: Uuid,
    pub item_id: Uuid,
    pub file_id: Option<Uuid>,
    pub started_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub position_ms: i64,
}

#[derive(Clone, PartialEq)]
enum RemuxStatus {
    Running,
    Ready,
    Failed(String),
}

pub struct PlaybackState {
    secret: Vec<u8>,
    pub cache_dir: PathBuf,
    sessions: Mutex<HashMap<Uuid, SessionInfo>>,
    remux: std::sync::Arc<Mutex<HashMap<Uuid, (PathBuf, RemuxStatus)>>>,
}

impl PlaybackState {
    pub fn new() -> Self {
        use rand::RngCore;
        let mut secret = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut secret);
        Self {
            secret,
            cache_dir: PathBuf::from("cache"),
            sessions: Mutex::new(HashMap::new()),
            remux: std::sync::Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn configure(&mut self, cache_dir: PathBuf, secret: Option<String>) {
        self.cache_dir = cache_dir;
        if let Some(s) = secret {
            if !s.is_empty() {
                self.secret = s.into_bytes();
            }
        }
    }
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// Stream tokens

#[derive(Serialize, Deserialize)]
struct TokenClaims {
    fid: Uuid,
    pid: Uuid,
    exp: i64,
}

fn make_token(state: &PlaybackState, file_id: Uuid, profile_id: Uuid) -> String {
    let claims = TokenClaims {
        fid: file_id,
        pid: profile_id,
        exp: Utc::now().timestamp() + TOKEN_TTL_SECS,
    };
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap_or_default());
    let mut mac = HmacSha256::new_from_slice(&state.secret).expect("hmac key");
    mac.update(payload.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{payload}.{sig}")
}

fn verify_token(state: &PlaybackState, token: &str, file_id: Uuid) -> Option<Uuid> {
    let (payload, sig) = token.split_once('.')?;
    let mut mac = HmacSha256::new_from_slice(&state.secret).ok()?;
    mac.update(payload.as_bytes());
    let sig_bytes = URL_SAFE_NO_PAD.decode(sig).ok()?;
    mac.verify_slice(&sig_bytes).ok()?;
    let claims: TokenClaims =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    if claims.fid != file_id || claims.exp < Utc::now().timestamp() {
        return None;
    }
    Some(claims.pid)
}

/// Authorise a media request: a valid stream token for this
/// file, or a signed-in session whose active profile can access
/// the file's library.
async fn authorise_media(
    app: &AppState,
    file_id: Uuid,
    token: Option<&str>,
    session_user: Option<&AuthUser>,
) -> Result<Uuid, AppError> {
    if let Some(token) = token {
        if let Some(pid) = verify_token(&app.playback, token, file_id) {
            return Ok(pid);
        }
    }
    if let Some(user) = session_user {
        if let Some(profile_id) = user.active_profile_id {
            let ok: Option<(bool,)> = sqlx::query_as(
                "SELECT true FROM media_files f
                 JOIN library_access a ON a.library_id = f.library_id AND a.profile_id = $2
                 WHERE f.id = $1",
            )
            .bind(file_id)
            .bind(profile_id)
            .fetch_optional(&app.pool)
            .await
            .map_err(AppError::from)?;
            if ok.is_some() {
                return Ok(profile_id);
            }
        }
    }
    Err(AppError::Unauthorized("media access denied".into()))
}

// ---------------------------------------------------------------------
// File rows

#[derive(FromRow)]
#[allow(dead_code)]
struct FileRow {
    id: Uuid,
    library_id: Uuid,
    item_id: Uuid,
    path: String,
    size_bytes: i64,
    container: Option<String>,
    duration_ms: Option<i64>,
}

async fn load_file(app: &AppState, file_id: Uuid) -> Result<FileRow, AppError> {
    sqlx::query_as::<_, FileRow>(
        "SELECT id, library_id, item_id, path, size_bytes, container, duration_ms
         FROM media_files WHERE id = $1 AND available",
    )
    .bind(file_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| AppError::NotFound("file".into()))
}

fn content_type_for(container: Option<&str>, path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "ts" | "m2ts" => "video/mp2t",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        _ => match container.unwrap_or("") {
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            _ => "application/octet-stream",
        },
    }
}

// ---------------------------------------------------------------------
// P1: playback info

#[derive(Deserialize)]
struct PlaybackInfoQuery {
    client: Option<String>,
    network: Option<String>,
}

#[derive(Serialize, FromRow)]
struct SubTrackRow {
    id: Uuid,
    stream_index: Option<i32>,
    source: String,
    format: Option<String>,
    language: Option<String>,
    title: Option<String>,
    is_forced: bool,
    is_default: bool,
    is_sdh: bool,
}

#[derive(Serialize)]
struct SubtitleDto {
    id: Uuid,
    language: Option<String>,
    title: Option<String>,
    is_forced: bool,
    is_default: bool,
    is_sdh: bool,
    source: String,
    text: bool,
    url: Option<String>,
}

#[derive(Serialize, FromRow)]
struct AudioDto {
    stream_index: i32,
    codec: Option<String>,
    language: Option<String>,
    is_default: bool,
}

#[derive(Serialize)]
struct PlaybackInfo {
    item_id: Uuid,
    file_id: Uuid,
    action: String,
    reason: String,
    stream_kind: String,
    stream_url: String,
    resume_position_ms: i64,
    played: bool,
    subtitles: Vec<SubtitleDto>,
    default_subtitle_id: Option<Uuid>,
    audio_streams: Vec<AudioDto>,
    warnings: Vec<String>,
}

fn is_text_sub(format: Option<&str>) -> bool {
    format
        .map(|f| TEXT_SUBTITLE_FORMATS.contains(&f.to_lowercase().as_str()))
        .unwrap_or(false)
}

async fn playback_info(
    State(app): State<AppState>,
    user: AuthUser,
    Path(item_id): Path<Uuid>,
    Query(q): Query<PlaybackInfoQuery>,
) -> Result<Json<PlaybackInfo>, AppError> {
    let Some(profile_id) = user.active_profile_id else {
        return Err(AppError::BadRequest("select a profile first".into()));
    };
    // Access-checked item load (no access = 404).
    let item: Option<(Uuid,)> = sqlx::query_as(
        "SELECT i.library_id FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $2
         WHERE i.id = $1",
    )
    .bind(item_id)
    .bind(profile_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    if item.is_none() {
        return Err(AppError::NotFound("not found".into()));
    }

    let file = sqlx::query_as::<_, FileRow>(
        "SELECT id, library_id, item_id, path, size_bytes, container, duration_ms
         FROM media_files WHERE item_id = $1 AND available ORDER BY relative_path LIMIT 1",
    )
    .bind(item_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| AppError::BadRequest("this item has no playable file yet".into()))?;

    let streams = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT kind, codec FROM media_streams WHERE file_id = $1 ORDER BY stream_index",
    )
    .bind(file.id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    let video_codec = streams
        .iter()
        .find(|(k, _)| k == "video")
        .and_then(|(_, c)| c.clone());
    let audio = sqlx::query_as::<_, AudioDto>(
        "SELECT stream_index, codec, language, is_default FROM media_streams
         WHERE file_id = $1 AND kind = 'audio' ORDER BY stream_index",
    )
    .bind(file.id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    let audio_codec = audio.first().and_then(|a| a.codec.clone());
    let bitrate_kbps = match (file.duration_ms, file.size_bytes) {
        (Some(d), s) if d > 0 => Some(s * 8 / d),
        _ => None,
    };

    let profile: (Option<String>, Option<String>, String, Option<i32>) = sqlx::query_as(
        "SELECT audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps
         FROM profiles WHERE id = $1",
    )
    .bind(profile_id)
    .fetch_one(&app.pool)
    .await
    .map_err(AppError::from)?;

    let client = match q.client.as_deref() {
        Some("android") => ClientKind::AndroidPhone,
        Some("androidtv") | Some("firetv") => ClientKind::AndroidTv,
        Some("ios") => ClientKind::Ios,
        Some("tvos") => ClientKind::TvOs,
        _ => ClientKind::Web,
    };
    let network = match q.network.as_deref() {
        Some("tailnet") => NetworkKind::Tailnet,
        Some("public") => NetworkKind::Public,
        _ => NetworkKind::Lan,
    };
    let plan = decide(
        &FileFacts {
            container: file.container.clone(),
            video_codec,
            audio_codec,
            bitrate_kbps,
        },
        client,
        network,
        profile.3,
    );

    let token = make_token(&app.playback, file.id, profile_id);
    let (stream_kind, stream_url) = match plan.action {
        parlour_core::PlaybackAction::Remux => {
            ensure_remux(&app, &file).await;
            (
                "hls",
                format!("/api/v1/stream/{}/hls/playlist.m3u8?token={token}", file.id),
            )
        }
        _ => ("file", format!("/api/v1/stream/{}?token={token}", file.id)),
    };

    let mut warnings = Vec::new();
    if plan.reason == parlour_core::PlaybackReason::RemoteBandwidth {
        warnings.push(
            "This file is over the profile's remote bitrate cap; Phase 1 plays the original \
             (adaptive transcoding arrives in Phase 2)."
                .to_string(),
        );
    }
    if plan.reason == parlour_core::PlaybackReason::CodecUnsupported {
        warnings.push(
            "This file uses a codec the web player may not decode; full transcoding arrives \
             in Phase 2."
                .to_string(),
        );
    }

    // Subtitles for this file + auto-pick.
    let track_rows = sqlx::query_as::<_, SubTrackRow>(
        "SELECT id, stream_index, source, format, language, title, is_forced, is_default, is_sdh
         FROM subtitle_tracks WHERE file_id = $1 ORDER BY source, stream_index",
    )
    .bind(file.id)
    .fetch_all(&app.pool)
    .await
    .map_err(AppError::from)?;
    let choices: Vec<SubtitleChoice> = track_rows
        .iter()
        .map(|t| SubtitleChoice {
            language: t.language.clone(),
            is_forced: t.is_forced,
            is_default: t.is_default,
            is_text: is_text_sub(t.format.as_deref()),
        })
        .collect();
    let default_idx = pick_subtitle(
        &choices,
        &profile.2,
        profile.1.as_deref(),
        profile.0.as_deref(),
    );
    let default_subtitle_id = default_idx.map(|i| track_rows[i].id);
    let subtitles: Vec<SubtitleDto> = track_rows
        .iter()
        .map(|t| {
            let text = is_text_sub(t.format.as_deref());
            SubtitleDto {
                id: t.id,
                language: t.language.clone(),
                title: t.title.clone(),
                is_forced: t.is_forced,
                is_default: t.is_default,
                is_sdh: t.is_sdh,
                source: t.source.clone(),
                text,
                url: text.then(|| format!("/api/v1/subtitles/{}?token={token}", t.id)),
            }
        })
        .collect();

    let user_data: Option<(i64, bool)> = sqlx::query_as(
        "SELECT position_ms, played FROM user_data WHERE profile_id = $1 AND item_id = $2",
    )
    .bind(profile_id)
    .bind(item_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    let (resume_position_ms, played) = match user_data {
        Some((pos, played)) => (if played { 0 } else { pos }, played),
        None => (0, false),
    };

    Ok(Json(PlaybackInfo {
        item_id,
        file_id: file.id,
        action: format!("{:?}", plan.action),
        reason: format!("{:?}", plan.reason),
        stream_kind: stream_kind.to_string(),
        stream_url,
        resume_position_ms,
        played,
        subtitles,
        default_subtitle_id,
        audio_streams: audio,
        warnings,
    }))
}

// ---------------------------------------------------------------------
// P2: file streaming with Range

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

async fn stream_file(
    State(app): State<AppState>,
    headers: HeaderMap,
    user: MaybeUser,
    Path(file_id): Path<Uuid>,
    Query(q): Query<TokenQuery>,
) -> Result<Response, AppError> {
    authorise_media(&app, file_id, q.token.as_deref(), user.0.as_ref()).await?;
    let file = load_file(&app, file_id).await?;
    serve_range(
        &file.path,
        file.size_bytes,
        &headers,
        content_type_for(file.container.as_deref(), &file.path),
    )
    .await
}

async fn serve_range(
    path: &str,
    size: i64,
    headers: &HeaderMap,
    content_type: &'static str,
) -> Result<Response, AppError> {
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::NotFound("file".into()))?;
    let total = size.max(0) as u64;
    let (start, end, status) = match parse_range(headers, total) {
        RangeResult::Full => (0, total.saturating_sub(1), StatusCode::OK),
        RangeResult::Partial(s, e) => (s, e, StatusCode::PARTIAL_CONTENT),
        RangeResult::Unsatisfiable => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            )
                .into_response());
        }
    };
    let len = if total == 0 { 0 } else { end - start + 1 };
    f.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let stream = tokio_util::io::ReaderStream::new(f.take(len));
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(header::ACCEPT_RANGES, "bytes");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    builder
        .body(Body::from_stream(stream))
        .map_err(|e| AppError::Internal(e.to_string()))
}

enum RangeResult {
    Full,
    Partial(u64, u64),
    Unsatisfiable,
}

/// Single-range parser (`bytes=a-b`, `bytes=a-`, `bytes=-b`).
fn parse_range(headers: &HeaderMap, total: u64) -> RangeResult {
    let Some(value) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return RangeResult::Full;
    };
    let Some(spec) = value.strip_prefix("bytes=") else {
        return RangeResult::Full;
    };
    if spec.contains(',') || total == 0 {
        return RangeResult::Unsatisfiable;
    }
    let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
    if a.is_empty() {
        // Suffix range: last b bytes.
        let Ok(n) = b.parse::<u64>() else {
            return RangeResult::Unsatisfiable;
        };
        if n == 0 {
            return RangeResult::Unsatisfiable;
        }
        let start = total.saturating_sub(n);
        return RangeResult::Partial(start, total - 1);
    }
    let Ok(start) = a.parse::<u64>() else {
        return RangeResult::Unsatisfiable;
    };
    if start >= total {
        return RangeResult::Unsatisfiable;
    }
    let end = if b.is_empty() {
        total - 1
    } else {
        match b.parse::<u64>() {
            Ok(e) => e.min(total - 1),
            Err(_) => return RangeResult::Unsatisfiable,
        }
    };
    if end < start {
        return RangeResult::Unsatisfiable;
    }
    RangeResult::Partial(start, end)
}

// ---------------------------------------------------------------------
// P3: remux to HLS/fMP4

async fn ensure_remux(app: &AppState, file: &FileRow) {
    let dir = app
        .playback
        .cache_dir
        .join("remux")
        .join(file.id.to_string());
    if dir.join(".ready").exists() {
        return; // finished in a previous run; cache is on disk
    }
    {
        let mut map = app.playback.remux.lock().unwrap();
        if let Some((_, status)) = map.get(&file.id) {
            if *status == RemuxStatus::Ready || *status == RemuxStatus::Running {
                return;
            }
        }
        map.insert(file.id, (dir.clone(), RemuxStatus::Running));
    }
    let map = app.playback.remux.clone();
    let file_id = file.id;
    let src = file.path.clone();
    let ffmpeg = std::env::var("PARLOUR_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string());
    tokio::spawn(async move {
        let _ = tokio::fs::create_dir_all(&dir).await;
        let _ = tokio::fs::remove_file(dir.join(".failed")).await;
        // Bare filenames + current_dir: the HLS muxer fails to
        // open the fMP4 init segment when given an absolute path
        // for -hls_fmp4_init_filename.
        let result = tokio::process::Command::new(ffmpeg)
            .current_dir(&dir)
            .args([
                "-y",
                "-v",
                "error",
                "-i",
                &src,
                // Video + first audio only: subtitles travel as
                // separate WebVTT tracks, and the HLS muxer
                // rejects copied subtitle streams outright.
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c",
                "copy",
                "-f",
                "hls",
                "-hls_time",
                "6",
                "-hls_segment_type",
                "fmp4",
                "-hls_fmp4_init_filename",
                "init.mp4",
                "-hls_segment_filename",
                "seg-%05d.m4s",
                "-hls_playlist_type",
                "vod",
                "playlist.m3u8",
            ])
            .status()
            .await;
        let status = match result {
            Ok(s) if s.success() => RemuxStatus::Ready,
            Ok(s) => RemuxStatus::Failed(format!("ffmpeg exited {s}")),
            Err(e) => RemuxStatus::Failed(e.to_string()),
        };
        match &status {
            RemuxStatus::Ready => {
                let _ = tokio::fs::write(dir.join(".ready"), b"").await;
            }
            RemuxStatus::Failed(msg) => {
                let _ = tokio::fs::write(dir.join(".failed"), msg).await;
            }
            RemuxStatus::Running => {}
        }
        map.lock().unwrap().insert(file_id, (dir, status));
    });
}

fn remux_status(app: &AppState, file_id: Uuid, dir: &std::path::Path) -> RemuxStatus {
    if dir.join(".ready").exists() {
        return RemuxStatus::Ready;
    }
    if let Ok(msg) = std::fs::read_to_string(dir.join(".failed")) {
        return RemuxStatus::Failed(msg);
    }
    let map = app.playback.remux.lock().unwrap();
    map.get(&file_id)
        .map(|(_, s)| s.clone())
        .unwrap_or(RemuxStatus::Running)
}

async fn stream_hls(
    State(app): State<AppState>,
    user: MaybeUser,
    Path((file_id, asset)): Path<(Uuid, String)>,
    Query(q): Query<TokenQuery>,
) -> Result<Response, AppError> {
    authorise_media(&app, file_id, q.token.as_deref(), user.0.as_ref()).await?;
    if asset.contains('/') || asset.contains('\\') || asset.starts_with('.') {
        return Err(AppError::NotFound("not found".into()));
    }
    let file = load_file(&app, file_id).await?;
    let dir = app
        .playback
        .cache_dir
        .join("remux")
        .join(file_id.to_string());
    ensure_remux(&app, &file).await;
    if asset == "playlist.m3u8" {
        match remux_status(&app, file_id, &dir) {
            RemuxStatus::Ready => {}
            RemuxStatus::Failed(msg) => return Err(AppError::Internal(msg)),
            RemuxStatus::Running => {
                return Ok((
                    StatusCode::SERVICE_UNAVAILABLE,
                    [(header::RETRY_AFTER, "2")],
                    "remux in progress",
                )
                    .into_response());
            }
        }
    }
    let path = dir.join(&asset);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound("file".into()))?;
    let content_type = if asset.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if asset.ends_with(".m4s") {
        "video/iso.segment"
    } else {
        "video/mp4"
    };
    Ok(([(header::CONTENT_TYPE, content_type)], bytes).into_response())
}

// ---------------------------------------------------------------------
// P4/P8: reporting + session registry

#[derive(Deserialize)]
struct ReportBody {
    item_id: Uuid,
    file_id: Option<Uuid>,
    position_ms: Option<i64>,
    duration_ms: Option<i64>,
}

async fn require_item_access(
    app: &AppState,
    profile_id: Uuid,
    item_id: Uuid,
) -> Result<(), AppError> {
    let ok: Option<(bool,)> = sqlx::query_as(
        "SELECT true FROM media_items i
         JOIN library_access a ON a.library_id = i.library_id AND a.profile_id = $2
         WHERE i.id = $1",
    )
    .bind(item_id)
    .bind(profile_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    if ok.is_none() {
        return Err(AppError::NotFound("not found".into()));
    }
    Ok(())
}

async fn upsert_position(
    app: &AppState,
    profile_id: Uuid,
    body: &ReportBody,
    bump_count: bool,
) -> Result<(), AppError> {
    let position = body.position_ms.unwrap_or(0).max(0);
    let duration = body.duration_ms;
    let played = match duration {
        Some(d) if d > 0 => position * 100 >= d * 90,
        _ => false,
    };
    sqlx::query(
        "INSERT INTO user_data (profile_id, item_id, position_ms, duration_ms, played, last_played_at, play_count, updated_at)
         VALUES ($1, $2, $3, $4, $5, now(), $6, now())
         ON CONFLICT (profile_id, item_id) DO UPDATE SET
            position_ms = EXCLUDED.position_ms,
            duration_ms = COALESCE(EXCLUDED.duration_ms, user_data.duration_ms),
            played = user_data.played OR EXCLUDED.played,
            last_played_at = now(),
            play_count = user_data.play_count + $6,
            updated_at = now()",
    )
    .bind(profile_id)
    .bind(body.item_id)
    .bind(position)
    .bind(duration)
    .bind(played)
    .bind(if bump_count { 1 } else { 0 })
    .execute(&app.pool)
    .await
    .map_err(AppError::from)?;
    Ok(())
}

fn active_profile(user: &AuthUser) -> Result<Uuid, AppError> {
    user.active_profile_id
        .ok_or_else(|| AppError::BadRequest("select a profile first".into()))
}

async fn report_start(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<ReportBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let profile_id = active_profile(&user)?;
    require_item_access(&app, profile_id, body.item_id).await?;
    upsert_position(&app, profile_id, &body, true).await?;
    let session_id = Uuid::new_v4();
    app.playback.sessions.lock().unwrap().insert(
        session_id,
        SessionInfo {
            profile_id,
            item_id: body.item_id,
            file_id: body.file_id,
            started_at: Utc::now(),
            last_seen: Utc::now(),
            position_ms: body.position_ms.unwrap_or(0),
        },
    );
    Ok(Json(serde_json::json!({"session_id": session_id})))
}

async fn report_progress(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<ReportBody>,
) -> Result<StatusCode, AppError> {
    let profile_id = active_profile(&user)?;
    require_item_access(&app, profile_id, body.item_id).await?;
    upsert_position(&app, profile_id, &body, false).await?;
    let mut sessions = app.playback.sessions.lock().unwrap();
    if let Some(s) = sessions
        .values_mut()
        .find(|s| s.profile_id == profile_id && s.item_id == body.item_id)
    {
        s.last_seen = Utc::now();
        s.position_ms = body.position_ms.unwrap_or(s.position_ms);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn report_stopped(
    State(app): State<AppState>,
    user: AuthUser,
    Json(body): Json<ReportBody>,
) -> Result<StatusCode, AppError> {
    let profile_id = active_profile(&user)?;
    require_item_access(&app, profile_id, body.item_id).await?;
    upsert_position(&app, profile_id, &body, false).await?;
    app.playback
        .sessions
        .lock()
        .unwrap()
        .retain(|_, s| !(s.profile_id == profile_id && s.item_id == body.item_id));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct SessionDto {
    session_id: Uuid,
    username: String,
    profile_name: String,
    item_id: Uuid,
    item_title: String,
    started_at: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    position_ms: i64,
}

async fn list_sessions(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<SessionDto>>, AppError> {
    let cutoff = Utc::now() - chrono::Duration::seconds(90);
    let active: Vec<(Uuid, SessionInfo)> = app
        .playback
        .sessions
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, s)| s.last_seen > cutoff)
        .map(|(id, s)| (*id, s.clone()))
        .collect();
    let mut out = Vec::new();
    for (session_id, s) in active {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT u.username, p.name, i.title FROM profiles p
             JOIN users u ON u.id = p.user_id
             JOIN media_items i ON i.id = $2
             WHERE p.id = $1",
        )
        .bind(s.profile_id)
        .bind(s.item_id)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?;
        if let Some((username, profile_name, item_title)) = row {
            out.push(SessionDto {
                session_id,
                username,
                profile_name,
                item_id: s.item_id,
                item_title,
                started_at: s.started_at,
                last_seen: s.last_seen,
                position_ms: s.position_ms,
            });
        }
    }
    Ok(Json(out))
}

// ---------------------------------------------------------------------
// P5: played / favourite marks

#[derive(Deserialize)]
struct MarkBody {
    played: Option<bool>,
    favorite: Option<bool>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserStateDto {
    pub played: bool,
    pub is_favorite: bool,
    pub position_ms: i64,
    pub play_count: i32,
}

pub async fn load_user_state(
    pool: &parlour_db::PgPool,
    profile_id: Uuid,
    item_id: Uuid,
) -> Result<Option<UserStateDto>, AppError> {
    let row = sqlx::query_as::<_, UserStateDto>(
        "SELECT played, is_favorite, position_ms, play_count FROM user_data
         WHERE profile_id = $1 AND item_id = $2",
    )
    .bind(profile_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)?;
    Ok(row)
}

async fn mark(
    State(app): State<AppState>,
    user: AuthUser,
    Path(item_id): Path<Uuid>,
    Json(body): Json<MarkBody>,
) -> Result<Json<UserStateDto>, AppError> {
    let profile_id = active_profile(&user)?;
    require_item_access(&app, profile_id, item_id).await?;
    if let Some(played) = body.played {
        sqlx::query(
            "INSERT INTO user_data (profile_id, item_id, played, position_ms, updated_at)
             VALUES ($1, $2, $3, 0, now())
             ON CONFLICT (profile_id, item_id) DO UPDATE SET
                played = EXCLUDED.played,
                position_ms = CASE WHEN EXCLUDED.played THEN user_data.position_ms ELSE 0 END,
                updated_at = now()",
        )
        .bind(profile_id)
        .bind(item_id)
        .bind(played)
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    }
    if let Some(fav) = body.favorite {
        sqlx::query(
            "INSERT INTO user_data (profile_id, item_id, is_favorite, updated_at)
             VALUES ($1, $2, $3, now())
             ON CONFLICT (profile_id, item_id) DO UPDATE SET
                is_favorite = EXCLUDED.is_favorite, updated_at = now()",
        )
        .bind(profile_id)
        .bind(item_id)
        .bind(fav)
        .execute(&app.pool)
        .await
        .map_err(AppError::from)?;
    }
    let state = load_user_state(&app.pool, profile_id, item_id)
        .await?
        .unwrap_or(UserStateDto {
            played: false,
            is_favorite: false,
            position_ms: 0,
            play_count: 0,
        });
    Ok(Json(state))
}

// ---------------------------------------------------------------------
// P6: subtitle extraction to WebVTT

async fn subtitle_vtt(
    State(app): State<AppState>,
    user: MaybeUser,
    Path(track_id): Path<Uuid>,
    Query(q): Query<TokenQuery>,
) -> Result<Response, AppError> {
    #[derive(FromRow)]
    struct TrackFile {
        file_id: Uuid,
        source: String,
        stream_index: Option<i32>,
        sub_path: Option<String>,
        media_path: String,
    }
    let track = sqlx::query_as::<_, TrackFile>(
        "SELECT t.file_id, t.source, t.stream_index, t.path AS sub_path, f.path AS media_path
         FROM subtitle_tracks t JOIN media_files f ON f.id = t.file_id
         WHERE t.id = $1",
    )
    .bind(track_id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| AppError::NotFound("not found".into()))?;
    authorise_media(&app, track.file_id, q.token.as_deref(), user.0.as_ref()).await?;

    let cache_file = app
        .playback
        .cache_dir
        .join("subtitles")
        .join(format!("{track_id}.vtt"));
    if !cache_file.exists() {
        tokio::fs::create_dir_all(cache_file.parent().unwrap())
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let ffmpeg = std::env::var("PARLOUR_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string());
        let out = cache_file.to_str().unwrap_or_default().to_string();
        let status = match track.source.as_str() {
            "sidecar" => {
                let src = track
                    .sub_path
                    .clone()
                    .ok_or_else(|| AppError::NotFound("not found".into()))?;
                tokio::process::Command::new(&ffmpeg)
                    .args(["-y", "-v", "error", "-i", &src, "-f", "webvtt", &out])
                    .status()
                    .await
            }
            _ => {
                let idx = track
                    .stream_index
                    .ok_or_else(|| AppError::NotFound("not found".into()))?;
                tokio::process::Command::new(&ffmpeg)
                    .args([
                        "-y",
                        "-v",
                        "error",
                        "-i",
                        &track.media_path,
                        "-map",
                        &format!("0:{idx}"),
                        "-f",
                        "webvtt",
                        &out,
                    ])
                    .status()
                    .await
            }
        };
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return Err(AppError::Internal(format!(
                    "subtitle extraction exited {s}"
                )))
            }
            Err(e) => return Err(AppError::Internal(e.to_string())),
        }
    }
    let bytes = tokio::fs::read(&cache_file)
        .await
        .map_err(|_| AppError::NotFound("file".into()))?;
    Ok(([(header::CONTENT_TYPE, "text/vtt; charset=utf-8")], bytes).into_response())
}

// ---------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/items/{id}/playback-info", get(playback_info))
        .route("/api/v1/stream/{file_id}", get(stream_file))
        .route("/api/v1/stream/{file_id}/hls/{asset}", get(stream_hls))
        .route("/api/v1/playback/start", post(report_start))
        .route("/api/v1/playback/progress", post(report_progress))
        .route("/api/v1/playback/stopped", post(report_stopped))
        .route("/api/v1/items/{id}/mark", post(mark))
        .route("/api/v1/subtitles/{track_id}", get(subtitle_vtt))
        .route("/api/v1/admin/sessions", get(list_sessions))
}
