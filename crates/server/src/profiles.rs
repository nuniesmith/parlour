//! A5: profiles under a user — CRUD, optional PIN, avatar colour,
//! and per-profile defaults (languages, subtitle mode, remote
//! bitrate cap). A profile always belongs to exactly one user;
//! asking for someone else's profile is a 404, never a 403.
//!
//! UI7 added the preferences the web Preferences pages edit:
//! Home Screen rail order/visibility and the Playback choices
//! Phase 9 reads (segment skip mode, autoplay next episode).

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
    pub home_rail_order: Vec<String>,
    pub home_rails_hidden: Vec<String>,
    pub segment_skip: String,
    pub autoplay_next: bool,
}

/// The Home Screen rails, in the default order. A rail id is
/// what clients render against; adding one here makes it appear,
/// visible, at the end of every profile's list until the profile
/// moves it. `libraries` is the library tile row and `latest` the
/// group of per-library Latest rails (UI8) — one id for the whole
/// group, so the set of ids does not depend on which libraries
/// exist.
pub const HOME_RAILS: &[&str] = &[
    "libraries",
    "continue_watching",
    "next_up",
    "recently_added",
    "latest",
];

/// One rail in a profile's Home Screen, in display order.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct HomeRail {
    pub id: String,
    pub visible: bool,
}

/// Every known rail, in the profile's chosen order, with its
/// visibility. Stored ids the server no longer knows are dropped;
/// known rails missing from the stored order go last, in default
/// order — so the answer is always a full permutation of
/// [`HOME_RAILS`].
pub fn home_rails(order: &[String], hidden: &[String]) -> Vec<HomeRail> {
    let mut ids: Vec<&str> = Vec::with_capacity(HOME_RAILS.len());
    for id in order {
        if let Some(known) = HOME_RAILS.iter().find(|k| **k == id.as_str()) {
            if !ids.contains(known) {
                ids.push(known);
            }
        }
    }
    for known in HOME_RAILS {
        if !ids.contains(known) {
            ids.push(known);
        }
    }
    ids.into_iter()
        .map(|id| HomeRail {
            id: id.to_string(),
            visible: !hidden.iter().any(|h| h == id),
        })
        .collect()
}

/// Validate a client's rail list and split it into what is
/// stored: the order, and the hidden set. Unknown or repeated ids
/// are a 400; a partial list is fine (the rest keep default
/// order after it, visible).
fn split_home_rails(rails: &[HomeRail]) -> Result<(Vec<String>, Vec<String>), AppError> {
    let mut order = Vec::with_capacity(rails.len());
    let mut hidden = Vec::new();
    for rail in rails {
        if !HOME_RAILS.contains(&rail.id.as_str()) {
            return Err(AppError::BadRequest(format!(
                "unknown home rail '{}'",
                rail.id
            )));
        }
        if order.contains(&rail.id) {
            return Err(AppError::BadRequest(format!(
                "home rail '{}' listed twice",
                rail.id
            )));
        }
        order.push(rail.id.clone());
        if !rail.visible {
            hidden.push(rail.id.clone());
        }
    }
    Ok((order, hidden))
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
    /// Home Screen rails in display order, each with visibility.
    pub home_rails: Vec<HomeRail>,
    /// Inside an intro/credits segment: `button`, `auto` or `off`.
    pub segment_skip: String,
    /// Next-episode countdown at credits / end of file.
    pub autoplay_next: bool,
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
            home_rails: home_rails(&p.home_rail_order, &p.home_rails_hidden),
            segment_skip: p.segment_skip,
            autoplay_next: p.autoplay_next,
        }
    }
}

pub async fn list_profiles_for(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Vec<ProfileDto>, AppError> {
    let rows = sqlx::query_as::<_, ProfileRow>(
        "SELECT id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps, home_rail_order, home_rails_hidden, segment_skip, autoplay_next FROM profiles WHERE user_id = $1 ORDER BY created_at",
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
        "SELECT id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps, home_rail_order, home_rails_hidden, segment_skip, autoplay_next FROM profiles WHERE id = $1 AND user_id = $2",
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
        "SELECT id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps, home_rail_order, home_rails_hidden, segment_skip, autoplay_next FROM profiles WHERE id = $1 AND user_id = $2",
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

fn validate_segment_skip(mode: &str) -> Result<(), AppError> {
    if matches!(mode, "button" | "auto" | "off") {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "segment_skip must be button, auto or off".into(),
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
        "INSERT INTO profiles (user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps) VALUES ($1, $2, $3, COALESCE($4, '#7c5cbf'), $5, $6, COALESCE($7, 'auto'), $8) RETURNING id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps, home_rail_order, home_rails_hidden, segment_skip, autoplay_next",
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
    /// Back to "no preference" — `null` alone can't say that,
    /// since an absent field means "leave it".
    #[serde(default)]
    clear_audio_lang: bool,
    #[serde(default)]
    clear_subtitle_lang: bool,
    home_rails: Option<Vec<HomeRail>>,
    segment_skip: Option<String>,
    autoplay_next: Option<bool>,
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
    if let Some(mode) = &body.segment_skip {
        validate_segment_skip(mode)?;
    }
    let rails = body
        .home_rails
        .as_deref()
        .map(split_home_rails)
        .transpose()?;
    let (rail_order, rails_hidden) = match rails {
        Some((order, hidden)) => (Some(order), Some(hidden)),
        None => (None, None),
    };
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
        "UPDATE profiles SET name = COALESCE($3, name), pin_hash = CASE WHEN $4 THEN $5 ELSE pin_hash END, avatar_color = COALESCE($6, avatar_color), audio_lang = CASE WHEN $16 THEN NULL ELSE COALESCE($7, audio_lang) END, subtitle_lang = CASE WHEN $17 THEN NULL ELSE COALESCE($8, subtitle_lang) END, subtitle_mode = COALESCE($9, subtitle_mode), remote_bitrate_cap_kbps = CASE WHEN $10 THEN $11 ELSE remote_bitrate_cap_kbps END, home_rail_order = COALESCE($12, home_rail_order), home_rails_hidden = COALESCE($13, home_rails_hidden), segment_skip = COALESCE($14, segment_skip), autoplay_next = COALESCE($15, autoplay_next) WHERE id = $1 AND user_id = $2 RETURNING id, user_id, name, pin_hash, avatar_color, audio_lang, subtitle_lang, subtitle_mode, remote_bitrate_cap_kbps, home_rail_order, home_rails_hidden, segment_skip, autoplay_next",
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
    .bind(rail_order)
    .bind(rails_hidden)
    .bind(&body.segment_skip)
    .bind(body.autoplay_next)
    .bind(body.clear_audio_lang)
    .bind(body.clear_subtitle_lang)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(rails: &[HomeRail]) -> Vec<&str> {
        rails.iter().map(|r| r.id.as_str()).collect()
    }

    fn strings(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_rails_are_all_visible_in_default_order() {
        let rails = home_rails(&[], &[]);
        assert_eq!(ids(&rails), HOME_RAILS);
        assert!(rails.iter().all(|r| r.visible));
    }

    #[test]
    fn stored_order_wins_and_missing_rails_go_last() {
        let rails = home_rails(&strings(&["recently_added"]), &strings(&["next_up"]));
        assert_eq!(
            ids(&rails),
            [
                "recently_added",
                "libraries",
                "continue_watching",
                "next_up",
                "latest"
            ]
        );
        let hidden: Vec<&str> = rails
            .iter()
            .filter(|r| !r.visible)
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(hidden, ["next_up"]);
    }

    #[test]
    fn unknown_and_repeated_stored_ids_are_dropped() {
        let rails = home_rails(
            &strings(&["gone_rail", "next_up", "next_up", "continue_watching"]),
            &strings(&["gone_rail"]),
        );
        assert_eq!(
            ids(&rails),
            [
                "next_up",
                "continue_watching",
                "libraries",
                "recently_added",
                "latest"
            ]
        );
        assert!(rails.iter().all(|r| r.visible));
    }

    #[test]
    fn split_round_trips_through_home_rails() {
        let wanted = vec![
            HomeRail {
                id: "next_up".into(),
                visible: true,
            },
            HomeRail {
                id: "recently_added".into(),
                visible: false,
            },
            HomeRail {
                id: "latest".into(),
                visible: false,
            },
            HomeRail {
                id: "continue_watching".into(),
                visible: true,
            },
            HomeRail {
                id: "libraries".into(),
                visible: true,
            },
        ];
        let (order, hidden) = split_home_rails(&wanted).unwrap();
        assert_eq!(hidden, strings(&["recently_added", "latest"]));
        assert_eq!(home_rails(&order, &hidden), wanted);
    }

    #[test]
    fn split_rejects_unknown_and_duplicate_ids() {
        let unknown = [HomeRail {
            id: "live_tv".into(),
            visible: true,
        }];
        assert!(split_home_rails(&unknown).is_err());
        let twice = [
            HomeRail {
                id: "next_up".into(),
                visible: true,
            },
            HomeRail {
                id: "next_up".into(),
                visible: false,
            },
        ];
        assert!(split_home_rails(&twice).is_err());
    }

    #[test]
    fn segment_skip_modes() {
        for ok in ["button", "auto", "off"] {
            assert!(validate_segment_skip(ok).is_ok());
        }
        assert!(validate_segment_skip("always").is_err());
    }
}
