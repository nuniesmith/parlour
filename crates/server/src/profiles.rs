//! A5: profiles under a user — CRUD, optional PIN, avatar colour,
//! and per-profile defaults (languages, subtitle mode, remote
//! bitrate cap). A profile always belongs to exactly one user;
//! asking for someone else's profile is a 404, never a 403.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::{hash_password, validate_pin, verify_password, AuthUser};
use crate::error::AppError;
use crate::http::AppState;

#[derive(Debug, Clone, FromRow)]
pub struct ProfileRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub pin_hash: Option<String>,
    pub avatar_color: Option<String>,
    pub audio_lang: Option<String>,
    pub subtitle_lang: Option<String>,
    pub subtitle_mode: String,
    pub remote_bitrate_cap_kbps: Option<i32>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ProfileDto {
    pub id: Uuid,
    pub name: String,
    pub has_pin: bool,
    pub avatar_color: String,
    /// First letter of the name, for avatar rendering in clients.
    pub initial: String,
    pub audio_lang: Option<String>,
    pub subtitle_lang: Option<String>,
    pub subtitle_mode: String,
    pub remote_bitrate_cap_kbps: Option<i32>,
}

impl From<ProfileRow> for ProfileDto {
    fn from(p: ProfileRow) -> Self {
        Self {
            id: p.id,
            initial: p
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default(),
            name: p.name,
            has_pin: p.pin_hash.is_some(),
            avatar_color: p.avatar_color.unwrap_or_else(|| "#7c5cbf".to_string()),
            audio_lang: p.audio_lang,
            subtitle_lang: p.subtitle_lang,
            subtitle_mode: p.subtitle_mode,
            remote_bitrate_cap_kbps: p.remote_bitrate_cap_kbps,
        }
    }
}

pub async fn list_profiles_for(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Vec<ProfileDto>, AppError> {
    let rows = sqlx::query_as::<_, ProfileRow>(
        "SELECT id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps FROM profiles WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::from)?;
    Ok(rows.into_iter().map(ProfileDto::from).collect())
}

pub async fn get_profile_for(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    profile_id: Uuid,
) -> Result<Option<ProfileDto>, AppError> {
    let row = sqlx::query_as::<_, ProfileRow>(
        "SELECT id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps FROM profiles WHERE id = $1 AND user_id = $2",
    )
    .bind(profile_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)?;
    Ok(row.map(ProfileDto::from))
}

async fn fetch_row(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    profile_id: Uuid,
) -> Result<ProfileRow, AppError> {
    sqlx::query_as::<_, ProfileRow>(
        "SELECT id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps FROM profiles WHERE id = $1 AND user_id = $2",
    )
    .bind(profile_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| AppError::NotFound("no such profile".into()))
}

fn parse_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such profile".into()))
}

fn validate_name(name: &str) -> Result<(), AppError> {
    if name.trim().is_empty() || name.len() > 40 {
        return Err(AppError::BadRequest(
            "profile name must be 1-40 characters".into(),
        ));
    }
    Ok(())
}

fn validate_color(color: &str) -> Result<(), AppError> {
    let ok = color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "avatar_color must look like #RRGGBB".into(),
        ))
    }
}

fn validate_mode(mode: &str) -> Result<(), AppError> {
    if matches!(mode, "auto" | "always" | "never") {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "subtitle_mode must be auto, always or never".into(),
        ))
    }
}

#[derive(Deserialize)]
struct CreateProfile {
    name: String,
    pin: Option<String>,
    avatar_color: Option<String>,
    audio_lang: Option<String>,
    subtitle_lang: Option<String>,
    subtitle_mode: Option<String>,
    remote_bitrate_cap_kbps: Option<i32>,
}

async fn list_profiles(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<ProfileDto>>, AppError> {
    Ok(Json(list_profiles_for(&state.pool, user.user_id).await?))
}

async fn create_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<CreateProfile>,
) -> Result<(StatusCode, Json<ProfileDto>), AppError> {
    validate_name(&body.name)?;
    if let Some(color) = &body.avatar_color {
        validate_color(color)?;
    }
    if let Some(mode) = &body.subtitle_mode {
        validate_mode(mode)?;
    }
    let pin_hash = match &body.pin {
        Some(pin) => {
            validate_pin(pin)?;
            Some(hash_password(pin)?)
        }
        None => None,
    };
    let row = sqlx::query_as::<_, ProfileRow>(
        "INSERT INTO profiles (user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps) VALUES ($1, $2, $3, COALESCE($4, '#7c5cbf'), $5, $6, COALESCE($7, 'auto'), $8) RETURNING id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps",
    )
    .bind(user.user_id)
    .bind(body.name.trim())
    .bind(pin_hash)
    .bind(&body.avatar_color)
    .bind(&body.audio_lang)
    .bind(&body.subtitle_lang)
    .bind(&body.subtitle_mode)
    .bind(body.remote_bitrate_cap_kbps)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("you already have a profile with that name".into())
        }
        _ => AppError::from(e),
    })?;
    Ok((StatusCode::CREATED, Json(ProfileDto::from(row))))
}

#[derive(Deserialize)]
struct UpdateProfile {
    name: Option<String>,
    pin: Option<String>,
    #[serde(default)]
    clear_pin: bool,
    avatar_color: Option<String>,
    audio_lang: Option<String>,
    subtitle_lang: Option<String>,
    subtitle_mode: Option<String>,
    remote_bitrate_cap_kbps: Option<i32>,
    #[serde(default)]
    clear_bitrate_cap: bool,
}

async fn update_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateProfile>,
) -> Result<Json<ProfileDto>, AppError> {
    let id = parse_id(&id)?;
    if let Some(name) = &body.name {
        validate_name(name)?;
    }
    if let Some(color) = &body.avatar_color {
        validate_color(color)?;
    }
    if let Some(mode) = &body.subtitle_mode {
        validate_mode(mode)?;
    }
    let pin_hash: Option<Option<String>> = if body.clear_pin {
        Some(None)
    } else if let Some(pin) = &body.pin {
        validate_pin(pin)?;
        Some(Some(hash_password(pin)?))
    } else {
        None
    };
    let cap: Option<Option<i32>> = if body.clear_bitrate_cap {
        Some(None)
    } else {
        body.remote_bitrate_cap_kbps.map(Some)
    };
    let row = sqlx::query_as::<_, ProfileRow>(
        "UPDATE profiles SET name = COALESCE($3, name), pin_hash = CASE WHEN $4 THEN $5 ELSE pin_hash END, avatar_color = COALESCE($6, avatar_color), audio_lang = COALESCE($7, audio_lang), subtitle_lang = COALESCE($8, subtitle_lang), subtitle_mode = COALESCE($9, subtitle_mode), remote_bitrate_cap_kbps = CASE WHEN $10 THEN $11 ELSE remote_bitrate_cap_kbps END WHERE id = $1 AND user_id = $2 RETURNING id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps",
    )
    .bind(id)
    .bind(user.user_id)
    .bind(body.name.as_deref().map(str::trim))
    .bind(pin_hash.is_some())
    .bind(pin_hash.flatten())
    .bind(&body.avatar_color)
    .bind(&body.audio_lang)
    .bind(&body.subtitle_lang)
    .bind(&body.subtitle_mode)
    .bind(cap.is_some())
    .bind(cap.flatten())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("you already have a profile with that name".into())
        }
        _ => AppError::from(e),
    })?
    .ok_or_else(|| AppError::NotFound("no such profile".into()))?;
    Ok(Json(ProfileDto::from(row)))
}

async fn delete_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id = parse_id(&id)?;
    let done = sqlx::query("DELETE FROM profiles WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.user_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such profile".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SelectProfile {
    pin: Option<String>,
}

/// Select the active profile for this session. A profile with a
/// PIN requires it — that is what keeps a kids' profile (and an
/// adults' one) honest on a shared TV account.
async fn select_profile(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<SelectProfile>,
) -> Result<Json<ProfileDto>, AppError> {
    let id = parse_id(&id)?;
    let row = fetch_row(&state.pool, user.user_id, id).await?;
    if let Some(pin_hash) = &row.pin_hash {
        let pin_ok = body
            .pin
            .as_deref()
            .map(|pin| verify_password(pin, pin_hash))
            .unwrap_or(false);
        if !pin_ok {
            return Err(AppError::Unauthorized("wrong PIN".into()));
        }
    }
    sqlx::query("UPDATE sessions SET active_profile_id = $1 WHERE id = $2")
        .bind(id)
        .bind(user.session_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    Ok(Json(ProfileDto::from(row)))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/profiles", get(list_profiles).post(create_profile))
        .route(
            "/api/v1/profiles/{id}",
            axum::routing::patch(update_profile).delete(delete_profile),
        )
        .route("/api/v1/profiles/{id}/select", post(select_profile))
}
