//! AB11 (D28/D29): an audiobooks library's own settings, mirroring the
//! library settings Audiobookshelf keeps for the same things. Spec:
//! `docs/AUDIOBOOKS-PODCASTS.md`, "Aligning with Audiobookshelf
//! (AB11)". Admins only; an audiobooks library only, as the metadata
//! precedence (AB6) is.
//!
//! - `GET | PUT /api/v1/admin/libraries/{id}/audiobook-settings` —
//!   `{"parse_subtitles", "finish_remaining_seconds", "finish_percent"}`:
//!   - `parse_subtitles` (default false): a title folder named
//!     `Title - Subtitle` gives the subtitle after ` - `. Takes effect
//!     at the next scan, which merges every book of the library again;
//!   - `finish_remaining_seconds` (default 10, 0 = off): a progress
//!     write leaving less than this much of the book marks it finished;
//!   - `finish_percent` (default null = off, else 1–100): finish at
//!     this share of the book instead of the time rule.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

/// ABS's default for the time rule, in seconds.
pub const DEFAULT_FINISH_REMAINING_SECONDS: i32 = 10;
/// The largest time rule accepted: a day.
pub const MAX_FINISH_REMAINING_SECONDS: i32 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudiobookSettings {
    pub parse_subtitles: bool,
    pub finish_remaining_seconds: i32,
    pub finish_percent: Option<i32>,
}

impl Default for AudiobookSettings {
    fn default() -> Self {
        AudiobookSettings {
            parse_subtitles: false,
            finish_remaining_seconds: DEFAULT_FINISH_REMAINING_SECONDS,
            finish_percent: None,
        }
    }
}

impl AudiobookSettings {
    /// What an admin sent, checked.
    pub fn validate(&self) -> Result<(), String> {
        if !(0..=MAX_FINISH_REMAINING_SECONDS).contains(&self.finish_remaining_seconds) {
            return Err(format!(
                "finish_remaining_seconds must be 0 to {MAX_FINISH_REMAINING_SECONDS}"
            ));
        }
        if self.finish_percent.is_some_and(|p| !(1..=100).contains(&p)) {
            return Err("finish_percent must be 1 to 100, or null for off".into());
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct SettingsDto {
    library_id: Uuid,
    #[serde(flatten)]
    settings: AudiobookSettings,
    /// ABS's defaults.
    default: AudiobookSettings,
}

fn parse_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such library".into()))
}

async fn load(state: &AppState, id: Uuid) -> Result<AudiobookSettings, AppError> {
    let row: Option<(String, bool, i32, Option<i32>)> = sqlx::query_as(
        "SELECT kind, parse_subtitles, finish_remaining_seconds, finish_percent
         FROM libraries WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;
    match row {
        None => Err(AppError::NotFound("no such library".into())),
        Some((kind, ..)) if kind != "audiobooks" => Err(AppError::BadRequest(
            "only an audiobooks library has audiobook settings".into(),
        )),
        Some((_, parse_subtitles, finish_remaining_seconds, finish_percent)) => {
            Ok(AudiobookSettings {
                parse_subtitles,
                finish_remaining_seconds,
                finish_percent,
            })
        }
    }
}

fn dto(library_id: Uuid, settings: AudiobookSettings) -> SettingsDto {
    SettingsDto {
        library_id,
        settings,
        default: AudiobookSettings::default(),
    }
}

async fn get_settings(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<SettingsDto>, AppError> {
    let library_id = parse_id(&id)?;
    Ok(Json(dto(library_id, load(&state, library_id).await?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsBody {
    parse_subtitles: bool,
    finish_remaining_seconds: i32,
    finish_percent: Option<i32>,
}

/// Set all three at once. A change of `parse_subtitles` re-merges the
/// library's books at the next scan (it is part of each book's sources
/// fingerprint); the finish rules apply to the next progress write.
async fn put_settings(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
    Json(body): Json<SettingsBody>,
) -> Result<Json<SettingsDto>, AppError> {
    let library_id = parse_id(&id)?;
    load(&state, library_id).await?;
    let settings = AudiobookSettings {
        parse_subtitles: body.parse_subtitles,
        finish_remaining_seconds: body.finish_remaining_seconds,
        finish_percent: body.finish_percent,
    };
    settings.validate().map_err(AppError::BadRequest)?;
    sqlx::query(
        "UPDATE libraries SET parse_subtitles = $2, finish_remaining_seconds = $3,
                finish_percent = $4
         WHERE id = $1",
    )
    .bind(library_id)
    .bind(settings.parse_subtitles)
    .bind(settings.finish_remaining_seconds)
    .bind(settings.finish_percent)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(dto(library_id, settings)))
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/admin/libraries/{id}/audiobook-settings",
        get(get_settings).put(put_settings),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_audiobookshelfs_and_are_checked() {
        let d = AudiobookSettings::default();
        assert!(!d.parse_subtitles);
        assert_eq!(d.finish_remaining_seconds, 10);
        assert_eq!(d.finish_percent, None);
        assert!(d.validate().is_ok());
        let with = |s: i32, p: Option<i32>| AudiobookSettings {
            parse_subtitles: true,
            finish_remaining_seconds: s,
            finish_percent: p,
        };
        assert!(
            with(0, None).validate().is_ok(),
            "0 turns the time rule off"
        );
        assert!(with(-1, None).validate().is_err());
        assert!(with(86_401, None).validate().is_err());
        assert!(with(10, Some(95)).validate().is_ok());
        assert!(with(10, Some(100)).validate().is_ok());
        assert!(with(10, Some(0)).validate().is_err());
        assert!(with(10, Some(101)).validate().is_err());
    }
}
