//! Playback surface (Milestone 1.6).
//!
//! - `GET /api/v1/items/{id}/playback-info` — the plan (P1)
//! - `GET /api/v1/stream/{file_id}` — Range streaming (P2)
//! - `GET /api/v1/stream/{file_id}/hls/{asset}` — remux (P3)
//! - `POST /api/v1/playback/{start,progress,stopped}` — reporting
//!   into `user_data` + the live-session registry (P4, P8)
//! - `POST /api/v1/items/{id}/{played,favorite}` — marks (P5)
//! - `GET /api/v1/subtitles/{track_id}` — WebVTT extraction (P6)
//! - `GET /api/v1/admin/sessions` — Now Playing (P8, UI3): each
//!   live session with the plan it was actually given, the
//!   client, and where the request came from
//! - `POST /api/v1/admin/sessions/{id}/stop` — admin stop (UI3):
//!   revokes the session's stream tokens and marks it stopped
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
use axum::http::{header, HeaderMap, StatusCode, Version};
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

use crate::auth::{AdminUser, AuthUser, ClientIp};

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
    pub duration_ms: Option<i64>,
    /// The plan playback-info handed this profile for this file
    /// (UI3). `None` when the client never asked for one.
    pub plan: Option<StreamPlan>,
    pub client: ClientInfo,
    pub remote: RemoteInfo,
    /// Set by an admin stop: the session is hidden from Now
    /// Playing and its next progress report is answered 409.
    pub stopped: bool,
}

/// First video stream: codec, width, height, HDR format.
type VideoFacts = (Option<String>, Option<i32>, Option<i32>, Option<String>);
/// First audio stream: codec, language, channels, default flag.
type AudioFacts = (Option<String>, Option<String>, Option<i32>, bool);

/// One source stream and what the server does with it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct VideoPlan {
    pub codec: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub hdr_format: Option<String>,
    /// "direct_play" | "copy" | "transcode"
    pub mode: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AudioPlan {
    pub codec: Option<String>,
    pub language: Option<String>,
    pub channels: Option<i32>,
    pub is_default: bool,
    /// "direct_play" | "copy" | "transcode"
    pub mode: &'static str,
}

/// The stream decision as the Now Playing card shows it (UI3):
/// source → what the server is doing, with the engine's reason.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct StreamPlan {
    /// `PlaybackAction` / `PlaybackReason` names, as playback-info
    /// reports them.
    pub action: String,
    pub reason: String,
    /// "file" (the original bytes) or "hls" (remux output).
    pub delivery: &'static str,
    /// Client kind and network the plan was decided for.
    pub client: &'static str,
    pub network: &'static str,
    pub container: Option<String>,
    pub bitrate_kbps: Option<i64>,
    pub video: Option<VideoPlan>,
    pub audio: Option<AudioPlan>,
    #[serde(skip)]
    decided_at: DateTime<Utc>,
}

/// Per-stream mode for an overall action. A remux copies every
/// stream into the new container untouched.
fn stream_mode(action: parlour_core::PlaybackAction) -> &'static str {
    match action {
        parlour_core::PlaybackAction::DirectPlay => "direct_play",
        parlour_core::PlaybackAction::Remux => "copy",
        parlour_core::PlaybackAction::Transcode => "transcode",
    }
}

/// What the client said about itself at playback start, plus its
/// User-Agent. Display only — capabilities never come from here.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: Option<String>,
    pub version: Option<String>,
    pub device: Option<String>,
    pub user_agent: Option<String>,
}

/// Where the start report came from. `ip` is the socket peer —
/// behind a reverse proxy that is the proxy, so the proxy's
/// `X-Forwarded-For` claim is carried separately, unverified.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct RemoteInfo {
    pub ip: Option<String>,
    pub forwarded_for: Option<String>,
    pub protocol: Option<String>,
}

fn protocol_name(version: Version) -> Option<String> {
    let name = match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => return None,
    };
    Some(name.to_string())
}

/// Trim, drop control characters, cap the length; blank is `None`.
fn clean_label(raw: Option<&str>, max: usize) -> Option<String> {
    let cleaned: String = raw?
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Sessions with no report for this long are not "now playing".
const SESSION_LIVE_SECS: i64 = 90;
/// ...and are forgotten entirely after this long.
const SESSION_FORGET_SECS: i64 = 3600;

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
    /// Latest plan handed out per (profile, file) by playback-info,
    /// picked up by the start report (UI3).
    plans: Mutex<HashMap<(Uuid, Uuid), StreamPlan>>,
    /// Admin stop (UI3): stream tokens for (profile, file) issued
    /// at or before this instant (ms) are refused.
    revoked: Mutex<HashMap<(Uuid, Uuid), i64>>,
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
            plans: Mutex::new(HashMap::new()),
            revoked: Mutex::new(HashMap::new()),
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
    /// Issued-at, milliseconds. Tokens minted before this field
    /// existed read as 0 and so fall to any later revocation.
    #[serde(default)]
    iat: i64,
}

enum TokenCheck {
    Valid(Uuid),
    /// Genuine token, but an admin stopped the session it fed.
    Revoked,
    Invalid,
}

fn make_token(state: &PlaybackState, file_id: Uuid, profile_id: Uuid) -> String {
    let claims = TokenClaims {
        fid: file_id,
        pid: profile_id,
        exp: Utc::now().timestamp() + TOKEN_TTL_SECS,
        iat: Utc::now().timestamp_millis(),
    };
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap_or_default());
    let mut mac = HmacSha256::new_from_slice(&state.secret).expect("hmac key");
    mac.update(payload.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{payload}.{sig}")
}

fn decode_token(state: &PlaybackState, token: &str) -> Option<TokenClaims> {
    let (payload, sig) = token.split_once('.')?;
    let mut mac = HmacSha256::new_from_slice(&state.secret).ok()?;
    mac.update(payload.as_bytes());
    let sig_bytes = URL_SAFE_NO_PAD.decode(sig).ok()?;
    mac.verify_slice(&sig_bytes).ok()?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

fn verify_token(state: &PlaybackState, token: &str, file_id: Uuid) -> TokenCheck {
    let Some(claims) = decode_token(state, token) else {
        return TokenCheck::Invalid;
    };
    if claims.fid != file_id || claims.exp < Utc::now().timestamp() {
        return TokenCheck::Invalid;
    }
    let revoked_at = state
        .revoked
        .lock()
        .unwrap()
        .get(&(claims.pid, claims.fid))
        .copied();
    match revoked_at {
        Some(at) if claims.iat <= at => TokenCheck::Revoked,
        _ => TokenCheck::Valid(claims.pid),
    }
}

/// Refuse every stream token already issued to this profile for
/// this file. Tokens minted afterwards (a deliberate new
/// playback-info) work again.
fn revoke_tokens(state: &PlaybackState, profile_id: Uuid, file_id: Uuid) {
    let now = Utc::now().timestamp_millis();
    let mut revoked = state.revoked.lock().unwrap();
    // A revocation outlives every token it could apply to after
    // one token lifetime; drop those.
    revoked.retain(|_, at| now - *at <= TOKEN_TTL_SECS * 1000);
    revoked.insert((profile_id, file_id), now);
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
        match verify_token(&app.playback, token, file_id) {
            TokenCheck::Valid(pid) => return Ok(pid),
            // A stopped stream stays stopped: the session cookie
            // must not quietly re-authorise the same URL.
            TokenCheck::Revoked => {
                return Err(AppError::Unauthorized("this stream was stopped".into()))
            }
            TokenCheck::Invalid => {}
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
    let video_facts: Option<VideoFacts> = sqlx::query_as(
        "SELECT codec, width, height, hdr_format FROM media_streams
             WHERE file_id = $1 AND kind = 'video' ORDER BY stream_index LIMIT 1",
    )
    .bind(file.id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
    let audio_facts: Option<AudioFacts> = sqlx::query_as(
        "SELECT codec, language, channels, is_default FROM media_streams
         WHERE file_id = $1 AND kind = 'audio' ORDER BY stream_index LIMIT 1",
    )
    .bind(file.id)
    .fetch_optional(&app.pool)
    .await
    .map_err(AppError::from)?;
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

    // UI3: remember the plan this profile was just handed for this
    // file, so the start report can attach it to the session.
    {
        let mode = stream_mode(plan.action);
        let now = Utc::now();
        let mut plans = app.playback.plans.lock().unwrap();
        plans.retain(|_, p| (now - p.decided_at).num_seconds() <= TOKEN_TTL_SECS);
        plans.insert(
            (profile_id, file.id),
            StreamPlan {
                action: format!("{:?}", plan.action),
                reason: format!("{:?}", plan.reason),
                delivery: stream_kind,
                client: match client {
                    ClientKind::Web => "web",
                    ClientKind::AndroidPhone => "android",
                    ClientKind::AndroidTv => "androidtv",
                    ClientKind::Ios => "ios",
                    ClientKind::TvOs => "tvos",
                },
                network: match network {
                    NetworkKind::Lan => "lan",
                    NetworkKind::Tailnet => "tailnet",
                    NetworkKind::Public => "public",
                },
                container: file.container.clone(),
                bitrate_kbps,
                video: video_facts.map(|(codec, width, height, hdr_format)| VideoPlan {
                    codec,
                    width,
                    height,
                    hdr_format,
                    mode,
                }),
                audio: audio_facts.map(|(codec, language, channels, is_default)| AudioPlan {
                    codec,
                    language,
                    channels,
                    is_default,
                    mode,
                }),
                decided_at: now,
            },
        );
    }

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
    /// The id `start` returned. Optional: without it the session
    /// is matched by profile + item.
    session_id: Option<Uuid>,
    /// Display-only self-description (start report).
    client_name: Option<String>,
    client_version: Option<String>,
    device_name: Option<String>,
}

/// Find the session a report belongs to: by id when the client
/// sent one, else this profile's live session for the item (a
/// stopped one only if nothing live matches).
fn find_session<'a>(
    sessions: &'a mut HashMap<Uuid, SessionInfo>,
    profile_id: Uuid,
    body: &ReportBody,
) -> Option<&'a mut SessionInfo> {
    let id = match body.session_id {
        Some(id) => sessions
            .get(&id)
            .filter(|s| s.profile_id == profile_id && s.item_id == body.item_id)
            .map(|_| id),
        None => {
            let mut matches: Vec<(&Uuid, &SessionInfo)> = sessions
                .iter()
                .filter(|(_, s)| s.profile_id == profile_id && s.item_id == body.item_id)
                .collect();
            matches.sort_by_key(|(_, s)| (s.stopped, std::cmp::Reverse(s.last_seen)));
            matches.first().map(|(id, _)| **id)
        }
    }?;
    sessions.get_mut(&id)
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
    ClientIp(ip): ClientIp,
    version: Version,
    headers: HeaderMap,
    Json(body): Json<ReportBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let profile_id = active_profile(&user)?;
    require_item_access(&app, profile_id, body.item_id).await?;
    upsert_position(&app, profile_id, &body, true).await?;
    let header_text = |name: header::HeaderName, max: usize| {
        clean_label(headers.get(name).and_then(|v| v.to_str().ok()), max)
    };
    let plan = body.file_id.and_then(|file_id| {
        app.playback
            .plans
            .lock()
            .unwrap()
            .get(&(profile_id, file_id))
            .cloned()
    });
    let now = Utc::now();
    let session_id = Uuid::new_v4();
    {
        let mut sessions = app.playback.sessions.lock().unwrap();
        sessions.retain(|_, s| (now - s.last_seen).num_seconds() <= SESSION_FORGET_SECS);
        sessions.insert(
            session_id,
            SessionInfo {
                profile_id,
                item_id: body.item_id,
                file_id: body.file_id,
                started_at: now,
                last_seen: now,
                position_ms: body.position_ms.unwrap_or(0),
                duration_ms: body.duration_ms.filter(|d| *d > 0),
                plan,
                client: ClientInfo {
                    name: clean_label(body.client_name.as_deref(), 80),
                    version: clean_label(body.client_version.as_deref(), 40),
                    device: clean_label(body.device_name.as_deref(), 80),
                    user_agent: header_text(header::USER_AGENT, 300),
                },
                remote: RemoteInfo {
                    ip: ip.map(|a| a.to_string()),
                    // First hop only: the address the proxy says it saw.
                    forwarded_for: header_text(
                        header::HeaderName::from_static("x-forwarded-for"),
                        200,
                    )
                    .and_then(|v| clean_label(v.split(',').next(), 64)),
                    protocol: protocol_name(version),
                },
                stopped: false,
            },
        );
    }
    let title = crate::activity::item_label(&app.pool, body.item_id).await;
    crate::activity::record(
        &app.pool,
        crate::activity::Entry::by_user(
            "playback.start",
            user.user_id,
            &user.username,
            format!("{} started playing {title}", user.username),
        )
        .profile(profile_id)
        .item(body.item_id)
        .detail(serde_json::json!({ "session_id": session_id })),
    )
    .await;
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
    if let Some(s) = find_session(&mut sessions, profile_id, &body) {
        if s.stopped {
            // The position above is still saved; the client is
            // told so it can stop instead of fighting 401s.
            return Err(AppError::Conflict(
                "this playback session was stopped by an administrator".into(),
            ));
        }
        s.last_seen = Utc::now();
        s.position_ms = body.position_ms.unwrap_or(s.position_ms);
        if let Some(d) = body.duration_ms.filter(|d| *d > 0) {
            s.duration_ms = Some(d);
        }
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
    // Whether a session that was still playing ended here. Players
    // can report `stopped` twice (ended, then page teardown), and a
    // session an admin stopped already has its own activity line.
    let ended_live = {
        let mut sessions = app.playback.sessions.lock().unwrap();
        let mine = |s: &SessionInfo| s.profile_id == profile_id && s.item_id == body.item_id;
        let ids: Vec<Uuid> = match body.session_id {
            // A client that names its session ends only that one.
            Some(id) => sessions
                .get(&id)
                .filter(|s| mine(s))
                .map(|_| vec![id])
                .unwrap_or_default(),
            None => sessions
                .iter()
                .filter(|(_, s)| mine(s))
                .map(|(id, _)| *id)
                .collect(),
        };
        let mut live = false;
        for id in ids {
            if let Some(s) = sessions.remove(&id) {
                live |= !s.stopped;
            }
        }
        live
    };
    if ended_live {
        let title = crate::activity::item_label(&app.pool, body.item_id).await;
        crate::activity::record(
            &app.pool,
            crate::activity::Entry::by_user(
                "playback.stop",
                user.user_id,
                &user.username,
                format!("{} stopped playing {title}", user.username),
            )
            .profile(profile_id)
            .item(body.item_id)
            .detail(serde_json::json!({
                "position_ms": body.position_ms,
                "duration_ms": body.duration_ms,
            })),
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct SessionDto {
    session_id: Uuid,
    username: String,
    profile_name: String,
    item_id: Uuid,
    item_title: String,
    item_kind: String,
    /// Episodes only: the show, and where the episode sits in it.
    series_title: Option<String>,
    season_number: Option<i32>,
    episode_number: Option<i32>,
    /// The item whose poster to show (the series for an episode),
    /// or null when neither has one.
    poster_item_id: Option<Uuid>,
    started_at: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    position_ms: i64,
    duration_ms: Option<i64>,
    client: ClientInfo,
    remote: RemoteInfo,
    plan: Option<StreamPlan>,
}

#[derive(FromRow)]
struct SessionRow {
    username: String,
    profile_name: String,
    item_title: String,
    item_kind: String,
    season_number: Option<i32>,
    episode_number: Option<i32>,
    series_title: Option<String>,
    poster_item_id: Option<Uuid>,
    file_duration_ms: Option<i64>,
}

async fn list_sessions(
    State(app): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<SessionDto>>, AppError> {
    let cutoff = Utc::now() - chrono::Duration::seconds(SESSION_LIVE_SECS);
    let mut active: Vec<(Uuid, SessionInfo)> = app
        .playback
        .sessions
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, s)| !s.stopped && s.last_seen > cutoff)
        .map(|(id, s)| (*id, s.clone()))
        .collect();
    active.sort_by_key(|(id, s)| (s.started_at, *id));
    let mut out = Vec::new();
    for (session_id, s) in active {
        // Episode -> season -> series; a movie has neither parent.
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT u.username, p.name AS profile_name, i.title AS item_title,
                    i.kind AS item_kind, i.season_number, i.episode_number,
                    sr.title AS series_title,
                    (SELECT c.id FROM media_items c
                      WHERE c.id IN (sr.id, i.id)
                        AND EXISTS (SELECT 1 FROM images im
                                     WHERE im.item_id = c.id AND im.kind = 'poster'
                                       AND im.path <> '' AND im.path NOT LIKE 'pending:%')
                      ORDER BY (c.id = sr.id) DESC LIMIT 1) AS poster_item_id,
                    (SELECT f.duration_ms FROM media_files f WHERE f.id = $3) AS file_duration_ms
             FROM profiles p
             JOIN users u ON u.id = p.user_id
             JOIN media_items i ON i.id = $2
             LEFT JOIN media_items se ON se.id = i.parent_id AND i.kind = 'episode'
             LEFT JOIN media_items sr ON sr.id = se.parent_id
             WHERE p.id = $1",
        )
        .bind(s.profile_id)
        .bind(s.item_id)
        .bind(s.file_id)
        .fetch_optional(&app.pool)
        .await
        .map_err(AppError::from)?;
        if let Some(row) = row {
            out.push(SessionDto {
                session_id,
                username: row.username,
                profile_name: row.profile_name,
                item_id: s.item_id,
                item_title: row.item_title,
                item_kind: row.item_kind,
                series_title: row.series_title,
                season_number: row.season_number,
                episode_number: row.episode_number,
                poster_item_id: row.poster_item_id,
                started_at: s.started_at,
                last_seen: s.last_seen,
                position_ms: s.position_ms,
                duration_ms: s.duration_ms.or(row.file_duration_ms),
                client: s.client,
                remote: s.remote,
                plan: s.plan,
            });
        }
    }
    Ok(Json(out))
}

/// UI3 admin stop. There is no channel to a client in v1, so
/// "stop" is the honest server-side version: revoke the stream
/// tokens this profile holds for the file (the next segment or
/// range request is refused) and mark the session stopped (its
/// next progress report is answered 409 so the player can say
/// why). A real remote pause needs client support.
async fn stop_session(
    State(app): State<AppState>,
    admin: AdminUser,
    Path(session_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let target = {
        let mut sessions = app.playback.sessions.lock().unwrap();
        match sessions.get_mut(&session_id) {
            Some(s) if !s.stopped => {
                s.stopped = true;
                Some((s.profile_id, s.file_id, s.item_id))
            }
            _ => None,
        }
    };
    let Some((profile_id, file_id, item_id)) = target else {
        return Err(AppError::NotFound("no such session".into()));
    };
    if let Some(file_id) = file_id {
        revoke_tokens(&app.playback, profile_id, file_id);
    }
    let viewer: Option<(String,)> = sqlx::query_as(
        "SELECT u.username FROM profiles p JOIN users u ON u.id = p.user_id WHERE p.id = $1",
    )
    .bind(profile_id)
    .fetch_optional(&app.pool)
    .await
    .unwrap_or(None);
    let viewer = viewer
        .map(|(name,)| name)
        .unwrap_or_else(|| "someone".into());
    let title = crate::activity::item_label(&app.pool, item_id).await;
    crate::activity::record(
        &app.pool,
        crate::activity::Entry::by_user(
            "playback.stopped_by_admin",
            admin.0.user_id,
            &admin.0.username,
            format!(
                "{} stopped {viewer}'s playback of {title}",
                admin.0.username
            ),
        )
        .profile(profile_id)
        .item(item_id)
        .detail(serde_json::json!({ "session_id": session_id })),
    )
    .await;
    tracing::info!(%session_id, admin = %admin.0.username, "playback session stopped by admin");
    Ok(StatusCode::NO_CONTENT)
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
        .route("/api/v1/admin/sessions/{id}/stop", post(stop_session))
}
