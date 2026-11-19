//! Segment editor (SEG4, D21) and the segment read path.
//!
//! - `GET /api/v1/admin/items/{id}/segments` — every stored
//!   intro / credits / recap row of the item, each with what it means
//!   for playback right now, plus the playback file the times are in
//! - `PUT /api/v1/admin/items/{id}/segments/{kind}`
//!   `{start_ms, end_ms}` — add or change the admin's own times for a
//!   kind: the item's `manual` row (confidence 1.0)
//! - `DELETE /api/v1/admin/items/{id}/segments/{kind}/{source}` —
//!   delete one listed row, per D21:
//!   - the manual row — times or a marker — is removed, and the kind
//!     is back under detection;
//!   - a detected row (chapters, fingerprint, heuristic) stays, because
//!     detection owns it and would write it again; instead a manual
//!     "no segment of this kind here" marker is stored, which beats
//!     detection now and after every later run. If the admin's own
//!     times already override detection for that kind, the request is
//!     refused (409) rather than throwing those times away.
//!
//! Both edits answer with the editor view, like GET.
//!
//! D21's limits: manual times are checked only for start before end
//! and both inside the playback file (0 ≤ start < end ≤ its length).
//! The detectors' length and position rules (D17/D18) do not apply,
//! and kinds may overlap. Every route is admin-only (404 for everyone
//! else, A7) and, like the other admin item routes, not limited to the
//! libraries the admin's active profile can see.
//!
//! [`effective_for`] is what playback-info serves. It reads the same
//! rows with the same rule (`parlour_playback::effective_segments`), so
//! the editor's "used by the player" and the player always agree.

use axum::extract::{Path, State};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use parlour_core::{MediaSegment, SegmentKind, SegmentSource};
use parlour_playback::{effective_segments, PLAYBACK_CONFIDENCE_FLOOR};

use crate::auth::AdminUser;
use crate::error::AppError;
use crate::http::AppState;

/// Confidence of every row the editor writes (SEG1: manual rows are
/// written at 1.0; D18: they win whatever it is).
const MANUAL_CONFIDENCE: f32 = 1.0;

// ---------------------------------------------------------------- rows

#[derive(FromRow)]
struct StoredRow {
    kind: String,
    source: String,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    confidence: f32,
    updated_at: DateTime<Utc>,
}

/// One stored row, parsed: a span, or — manual rows only — the admin's
/// "no segment of this kind here" marker (D21).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Row {
    kind: SegmentKind,
    source: SegmentSource,
    /// `None` is the marker.
    span: Option<(i64, i64)>,
    confidence: f32,
    updated_at: DateTime<Utc>,
}

/// Parse stored rows. A row this binary cannot read — a kind or source
/// from a newer schema, or a time-less row the schema forbids — is
/// skipped rather than failing playback or the editor.
fn parse_rows(stored: Vec<StoredRow>) -> Vec<Row> {
    stored
        .into_iter()
        .filter_map(|r| {
            let kind = SegmentKind::parse(&r.kind)?;
            let source = SegmentSource::parse(&r.source)?;
            let span = match (r.start_ms, r.end_ms) {
                (Some(start), Some(end)) => Some((start, end)),
                (None, None) if source.is_manual() => None,
                _ => return None,
            };
            Some(Row {
                kind,
                source,
                span,
                confidence: r.confidence,
                updated_at: r.updated_at,
            })
        })
        .collect()
}

async fn load_rows(pool: &parlour_db::PgPool, item_id: Uuid) -> Result<Vec<Row>, AppError> {
    let stored = sqlx::query_as::<_, StoredRow>(
        "SELECT kind, source, start_ms, end_ms, confidence, updated_at
         FROM media_segments WHERE item_id = $1",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::from)?;
    Ok(parse_rows(stored))
}

/// What the player gets from these rows: the spans, minus every kind an
/// admin marked as having none.
fn effective(rows: &[Row]) -> Vec<MediaSegment> {
    let spans: Vec<MediaSegment> = rows
        .iter()
        .filter_map(|r| {
            r.span.map(|(start_ms, end_ms)| MediaSegment {
                kind: r.kind,
                start_ms,
                end_ms,
                source: r.source,
                confidence: r.confidence,
            })
        })
        .collect();
    let marked_none: Vec<SegmentKind> = rows
        .iter()
        .filter(|r| r.span.is_none())
        .map(|r| r.kind)
        .collect();
    effective_segments(&spans, &marked_none)
}

/// The item's segments as playback-info hands them to the player:
/// at most one per kind, start-ordered (SEG1, D17, D21).
pub async fn effective_for(
    pool: &parlour_db::PgPool,
    item_id: Uuid,
) -> Result<Vec<MediaSegment>, AppError> {
    Ok(effective(&load_rows(pool, item_id).await?))
}

/// What one stored row means for playback right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RowStatus {
    /// The player gets this segment.
    Used,
    /// The admin's "no segment of this kind here" marker: the player
    /// gets no segment of this kind, whatever detection finds (D21).
    NoSegment,
    /// Detected, but the admin's own times for this kind are used
    /// instead (D15).
    Overridden,
    /// Detected, but the admin's marker says this kind has none (D21).
    Hidden,
    /// Detected below the playback floor (D17): never served.
    BelowFloor,
    /// Detected; another detected row of this kind is used instead.
    Outranked,
}

/// The status of each row, in the rows' order.
fn statuses(rows: &[Row]) -> Vec<RowStatus> {
    let used = effective(rows);
    rows.iter()
        .map(|row| {
            let manual_of_kind = rows
                .iter()
                .find(|r| r.kind == row.kind && r.source.is_manual());
            if row.span.is_none() {
                RowStatus::NoSegment
            } else if used
                .iter()
                .any(|u| u.kind == row.kind && u.source == row.source)
            {
                RowStatus::Used
            } else if row.source.is_manual() {
                // Unreachable with the schema's checks (a manual span
                // always wins); described rather than hidden.
                RowStatus::Outranked
            } else {
                match manual_of_kind {
                    Some(m) if m.span.is_none() => RowStatus::Hidden,
                    Some(_) => RowStatus::Overridden,
                    None if row.confidence < PLAYBACK_CONFIDENCE_FLOOR => RowStatus::BelowFloor,
                    None => RowStatus::Outranked,
                }
            }
        })
        .collect()
}

/// D17's detector order, the admin's row first: how the editor lists a
/// kind's rows.
fn source_order(source: SegmentSource) -> u8 {
    match source {
        SegmentSource::Manual => 0,
        SegmentSource::Chapters => 1,
        SegmentSource::Fingerprint => 2,
        SegmentSource::Heuristic => 3,
    }
}

// ------------------------------------------------------------ the rules

/// "1:02:03.450" or "2:03.450", for messages.
fn clock(ms: i64) -> String {
    let (hours, minutes, seconds, millis) = (
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1_000 % 60,
        ms % 1_000,
    );
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes}:{seconds:02}.{millis:03}")
    }
}

/// D21: manual times are checked only for start before end and both
/// inside the file — `0 <= start < end <= length`. Nothing else: the
/// detectors' length and position rules (D17/D18) don't apply to an
/// admin's times, and kinds may overlap. A file whose length is
/// unknown can't be checked, so it can't take manual times.
fn check_span(start_ms: i64, end_ms: i64, file_ms: Option<i64>) -> Result<(), String> {
    let Some(file_ms) = file_ms.filter(|d| *d > 0) else {
        return Err(
            "this file's length is unknown, so segment times can't be checked against it \
             — rescan the library and try again"
                .into(),
        );
    };
    if start_ms < 0 {
        return Err("a segment can't start before the beginning of the file".into());
    }
    if end_ms <= start_ms {
        return Err("a segment must start before it ends".into());
    }
    if end_ms > file_ms {
        return Err(format!(
            "the end ({}) is past the end of the file ({})",
            clock(end_ms),
            clock(file_ms)
        ));
    }
    Ok(())
}

fn parse_id(id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(id).map_err(|_| AppError::NotFound("no such item".into()))
}

fn parse_kind(kind: &str) -> Result<SegmentKind, AppError> {
    SegmentKind::parse(kind).ok_or_else(|| {
        AppError::BadRequest(format!(
            "unknown segment kind \"{kind}\" (intro, credits or recap)"
        ))
    })
}

fn parse_source(source: &str) -> Result<SegmentSource, AppError> {
    SegmentSource::parse(source).ok_or_else(|| {
        AppError::BadRequest(format!(
            "unknown segment source \"{source}\" (manual, chapters, fingerprint or heuristic)"
        ))
    })
}

// ------------------------------------------------------------- the view

/// The file playback serves for the item — first available by relative
/// path, then path, as playback-info and detection pick it — so its
/// times are the ones the segments are in.
#[derive(Debug, Serialize, FromRow)]
struct PlaybackFileDto {
    id: Uuid,
    relative_path: String,
    /// Null when ffprobe could not tell; manual times can't be added then.
    duration_ms: Option<i64>,
}

#[derive(Serialize)]
struct RowDto {
    kind: &'static str,
    source: &'static str,
    /// Both null on the admin's "no segment of this kind here" marker.
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    confidence: f32,
    status: RowStatus,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct EditorDto {
    item_id: Uuid,
    /// Null when the item has no playable file (a series, a season, an
    /// item whose files are all unavailable): nothing can be added then.
    file: Option<PlaybackFileDto>,
    /// Every stored row, kind by kind (recap, intro, credits), each
    /// kind's manual row first, then the detectors in D17's order.
    rows: Vec<RowDto>,
}

async fn require_item(state: &AppState, item_id: Uuid) -> Result<(), AppError> {
    let found: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM media_items WHERE id = $1")
        .bind(item_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?;
    found
        .map(|_| ())
        .ok_or_else(|| AppError::NotFound("no such item".into()))
}

async fn playback_file(
    state: &AppState,
    item_id: Uuid,
) -> Result<Option<PlaybackFileDto>, AppError> {
    sqlx::query_as::<_, PlaybackFileDto>(
        "SELECT id, relative_path, duration_ms FROM media_files
         WHERE item_id = $1 AND available
         ORDER BY relative_path, path LIMIT 1",
    )
    .bind(item_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)
}

async fn editor_view(state: &AppState, item_id: Uuid) -> Result<EditorDto, AppError> {
    let file = playback_file(state, item_id).await?;
    let mut rows = load_rows(&state.pool, item_id).await?;
    rows.sort_by_key(|r| (r.kind, source_order(r.source)));
    let rows = rows
        .iter()
        .zip(statuses(&rows))
        .map(|(r, status)| RowDto {
            kind: r.kind.as_str(),
            source: r.source.as_str(),
            start_ms: r.span.map(|s| s.0),
            end_ms: r.span.map(|s| s.1),
            confidence: r.confidence,
            status,
            updated_at: r.updated_at,
        })
        .collect();
    Ok(EditorDto {
        item_id,
        file,
        rows,
    })
}

// ------------------------------------------------------------- handlers

async fn get_segments(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<Json<EditorDto>, AppError> {
    let item_id = parse_id(&id)?;
    require_item(&state, item_id).await?;
    Ok(Json(editor_view(&state, item_id).await?))
}

#[derive(Deserialize)]
struct TimesBody {
    start_ms: i64,
    end_ms: i64,
}

/// Add or change the admin's times for one kind. Replaces the kind's
/// marker if it had one: the admin now says where the segment is.
async fn set_times(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path((id, kind)): Path<(String, String)>,
    Json(body): Json<TimesBody>,
) -> Result<Json<EditorDto>, AppError> {
    let item_id = parse_id(&id)?;
    let kind = parse_kind(&kind)?;
    require_item(&state, item_id).await?;
    let file = playback_file(&state, item_id).await?.ok_or_else(|| {
        AppError::BadRequest("this item has no playable file, so it has no segments".into())
    })?;
    check_span(body.start_ms, body.end_ms, file.duration_ms).map_err(AppError::BadRequest)?;
    sqlx::query(
        "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
         VALUES ($1, $2, $3, $4, 'manual', $5)
         ON CONFLICT (item_id, kind, source) DO UPDATE
         SET start_ms = EXCLUDED.start_ms, end_ms = EXCLUDED.end_ms,
             confidence = EXCLUDED.confidence, updated_at = now()",
    )
    .bind(item_id)
    .bind(kind.as_str())
    .bind(body.start_ms)
    .bind(body.end_ms)
    .bind(MANUAL_CONFIDENCE)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(editor_view(&state, item_id).await?))
}

/// Delete one listed row, per D21 (see the module docs).
async fn delete_segment(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path((id, kind, source)): Path<(String, String, String)>,
) -> Result<Json<EditorDto>, AppError> {
    let item_id = parse_id(&id)?;
    let kind = parse_kind(&kind)?;
    let source = parse_source(&source)?;
    require_item(&state, item_id).await?;
    if source.is_manual() {
        // The admin's own row, times or marker: gone, and detection
        // decides this kind again.
        let done = sqlx::query(
            "DELETE FROM media_segments
             WHERE item_id = $1 AND kind = $2 AND source = 'manual'",
        )
        .bind(item_id)
        .bind(kind.as_str())
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
        if done.rows_affected() == 0 {
            return Err(AppError::NotFound("no such segment".into()));
        }
        return Ok(Json(editor_view(&state, item_id).await?));
    }

    // A detected row: store the kind's "none" marker instead.
    let mut tx = state.pool.begin().await.map_err(AppError::from)?;
    let detected: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM media_segments WHERE item_id = $1 AND kind = $2 AND source = $3",
    )
    .bind(item_id)
    .bind(kind.as_str())
    .bind(source.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::from)?;
    if detected.is_none() {
        return Err(AppError::NotFound("no such segment".into()));
    }
    let manual: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT start_ms FROM media_segments
         WHERE item_id = $1 AND kind = $2 AND source = 'manual'
         FOR UPDATE",
    )
    .bind(item_id)
    .bind(kind.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(AppError::from)?;
    match manual {
        // Already marked: nothing more to store.
        Some((None,)) => {}
        Some((Some(_),)) => {
            return Err(AppError::Conflict(format!(
                "this item's {} already uses times an admin set, not the detected ones \
                 — change or delete those instead",
                kind.as_str()
            )));
        }
        None => {
            sqlx::query(
                "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
                 VALUES ($1, $2, NULL, NULL, 'manual', $3)
                 ON CONFLICT (item_id, kind, source) DO NOTHING",
            )
            .bind(item_id)
            .bind(kind.as_str())
            .bind(MANUAL_CONFIDENCE)
            .execute(&mut *tx)
            .await
            .map_err(AppError::from)?;
        }
    }
    tx.commit().await.map_err(AppError::from)?;
    Ok(Json(editor_view(&state, item_id).await?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/items/{id}/segments", get(get_segments))
        .route("/api/v1/admin/items/{id}/segments/{kind}", put(set_times))
        .route(
            "/api/v1/admin/items/{id}/segments/{kind}/{source}",
            delete(delete_segment),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use parlour_core::SegmentKind::*;
    use parlour_core::SegmentSource::*;

    fn row(kind: SegmentKind, source: SegmentSource, span: Option<(i64, i64)>, c: f32) -> Row {
        Row {
            kind,
            source,
            span,
            confidence: c,
            updated_at: Utc::now(),
        }
    }

    fn stored(kind: &str, source: &str, start: Option<i64>, end: Option<i64>) -> StoredRow {
        StoredRow {
            kind: kind.into(),
            source: source.into(),
            start_ms: start,
            end_ms: end,
            confidence: 1.0,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn manual_times_are_checked_only_against_the_file() {
        let file = Some(60_000);
        assert!(check_span(0, 60_000, file).is_ok(), "the whole file");
        assert!(check_span(59_999, 60_000, file).is_ok(), "1 ms at the end");
        // D17/D18 do not apply: a 1 s "intro" 50 s in is fine.
        assert!(check_span(50_000, 51_000, file).is_ok());
        assert!(check_span(-1, 1_000, file).is_err(), "before the start");
        assert!(check_span(5_000, 5_000, file).is_err(), "empty");
        assert!(check_span(6_000, 5_000, file).is_err(), "backwards");
        let err = check_span(0, 60_001, file).unwrap_err();
        assert!(
            err.contains("1:00.001") && err.contains("1:00.000"),
            "{err}"
        );
        // A file of unknown length can't be checked.
        assert!(check_span(0, 1_000, None).is_err());
        assert!(check_span(0, 1_000, Some(0)).is_err());
    }

    #[test]
    fn clock_reads_like_a_player() {
        assert_eq!(clock(0), "0:00.000");
        assert_eq!(clock(90_250), "1:30.250");
        assert_eq!(clock(3_723_450), "1:02:03.450");
    }

    #[test]
    fn rows_a_newer_schema_wrote_are_skipped() {
        let rows = parse_rows(vec![
            stored("intro", "manual", None, None),
            stored("credits", "chapters", Some(1_000), Some(2_000)),
            stored("outro", "manual", Some(0), Some(1_000)),
            stored("intro", "guess", Some(0), Some(1_000)),
            // Only a manual row may be a marker, and only with no
            // times at all.
            stored("intro", "chapters", None, None),
            stored("recap", "manual", Some(0), None),
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].kind, rows[0].span), (Intro, None));
        assert_eq!(
            (rows[1].kind, rows[1].span),
            (Credits, Some((1_000, 2_000)))
        );
    }

    #[test]
    fn a_marker_hides_its_kind_from_the_player() {
        let rows = vec![
            row(Intro, Manual, None, 1.0),
            row(Intro, Chapters, Some((5_000, 20_000)), 1.0),
            row(Credits, Fingerprint, Some((40_000, 60_000)), 0.9),
        ];
        let served = effective(&rows);
        assert_eq!(served.len(), 1);
        assert_eq!((served[0].kind, served[0].source), (Credits, Fingerprint));
        assert_eq!(
            statuses(&rows),
            vec![RowStatus::NoSegment, RowStatus::Hidden, RowStatus::Used]
        );
    }

    #[test]
    fn every_row_says_why_it_is_or_is_not_used() {
        let rows = vec![
            // Intro: the admin's times beat both detectors.
            row(Intro, Manual, Some((6_000, 19_000)), 1.0),
            row(Intro, Chapters, Some((5_000, 20_000)), 1.0),
            row(Intro, Heuristic, Some((4_000, 21_000)), 0.5),
            // Credits: fingerprint outranks chapters on confidence; the
            // heuristic guess is under the floor.
            row(Credits, Chapters, Some((40_000, 60_000)), 0.7),
            row(Credits, Fingerprint, Some((41_000, 60_000)), 0.9),
            row(Credits, Heuristic, Some((42_000, 60_000)), 0.5),
            // Recap: only a guess — nothing is used.
            row(Recap, Heuristic, Some((0, 3_000)), 0.5),
        ];
        assert_eq!(
            statuses(&rows),
            vec![
                RowStatus::Used,
                RowStatus::Overridden,
                RowStatus::Overridden,
                RowStatus::Outranked,
                RowStatus::Used,
                RowStatus::BelowFloor,
                RowStatus::BelowFloor,
            ]
        );
        // The admin's row lists first, then D17's order.
        let mut sorted = rows.clone();
        sorted.reverse();
        sorted.sort_by_key(|r| (r.kind, source_order(r.source)));
        let order: Vec<_> = sorted.iter().map(|r| (r.kind, r.source)).collect();
        assert_eq!(
            order,
            vec![
                (Recap, Heuristic),
                (Intro, Manual),
                (Intro, Chapters),
                (Intro, Heuristic),
                (Credits, Chapters),
                (Credits, Fingerprint),
                (Credits, Heuristic),
            ]
        );
    }
}
