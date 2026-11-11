//! A4: invite-only sign-up. An admin creates an invite; the raw
//! token is shown exactly once, only its hash is stored. There is
//! deliberately NO open registration route — accepting an invite
//! is the only way a new account comes to exist (besides the
//! one-time env bootstrap).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::{create_user, new_token, token_hash, AdminUser, UserDto};
use crate::error::AppError;
use crate::http::AppState;

const INVITE_TTL_DAYS: i64 = 7;

#[derive(Serialize, FromRow)]
struct InviteDto {
    id: Uuid,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    used_at: Option<DateTime<Utc>>,
    used_by: Option<Uuid>,
}

#[derive(Serialize)]
struct CreatedInvite {
    id: Uuid,
    /// The raw token — returned once, never stored.
    token: String,
    invite_path: String,
    expires_at: DateTime<Utc>,
}

async fn create_invite(
    State(state): State<AppState>,
    admin: AdminUser,
) -> Result<(StatusCode, Json<CreatedInvite>), AppError> {
    let (raw, hash) = new_token();
    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO invites (token_hash, created_by, expires_at)
         VALUES ($1, $2, now() + ($3 || ' days')::interval)
         RETURNING id, expires_at",
    )
    .bind(&hash)
    .bind(admin.0.user_id)
    .bind(INVITE_TTL_DAYS.to_string())
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedInvite {
            id: row.0,
            invite_path: format!("/invite/{raw}"),
            token: raw,
            expires_at: row.1,
        }),
    ))
}

async fn list_invites(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<InviteDto>>, AppError> {
    let rows = sqlx::query_as::<_, InviteDto>(
        "SELECT id, created_at, expires_at, used_at, used_by FROM invites ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(rows))
}

async fn revoke_invite(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("no such invite".into()))?;
    let done = sqlx::query("DELETE FROM invites WHERE id = $1 AND used_at IS NULL")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such invite".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct InviteStatus {
    valid: bool,
}

/// Lets the invite page check a link before showing the form.
/// Checking never consumes the invite.
async fn invite_status(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<InviteStatus>, AppError> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM invites
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(token_hash(&token))
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(InviteStatus {
        valid: count.0 == 1,
    }))
}

#[derive(Deserialize)]
struct AcceptInvite {
    token: String,
    username: String,
    password: String,
}

#[derive(Serialize)]
struct AcceptResponse {
    user: UserDto,
}

/// Accept an invite: creates the account (and default profile)
/// and marks the invite used, atomically. The new user then
/// signs in normally — no session is minted here, so the invite
/// link itself never becomes a login.
async fn accept_invite(
    State(state): State<AppState>,
    Json(body): Json<AcceptInvite>,
) -> Result<(StatusCode, Json<AcceptResponse>), AppError> {
    let invite: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM invites
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(token_hash(&body.token))
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;
    let Some((invite_id,)) = invite else {
        return Err(AppError::BadRequest(
            "this invite is invalid, expired, or already used".into(),
        ));
    };
    // Create the account first (a taken username must NOT consume
    // the invite — the invitee retries with another name), then
    // claim the invite with a conditional update.
    let user_id = create_user(&state.pool, &body.username, &body.password, false).await?;
    let done = sqlx::query(
        "UPDATE invites SET used_at = now(), used_by = $2 WHERE id = $1 AND used_at IS NULL",
    )
    .bind(invite_id)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        // Lost a race: the invite was consumed between check and
        // create. The account exists now; say so plainly.
        return Err(AppError::Conflict(
            "this invite was just used by someone else".into(),
        ));
    }
    let user: (String, bool) = sqlx::query_as("SELECT username, is_admin FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(AppError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(AcceptResponse {
            user: UserDto {
                id: user_id,
                username: user.0,
                is_admin: user.1,
            },
        }),
    ))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/admin/invites",
            post(create_invite).get(list_invites),
        )
        .route("/api/v1/admin/invites/{id}", delete(revoke_invite))
        .route("/api/v1/invites/{token}", get(invite_status))
        .route("/api/v1/invites/accept", post(accept_invite))
}
