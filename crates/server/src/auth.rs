//! Authentication and sessions (A1–A3).
//!
//! Passwords are Argon2id hashes. Sessions are opaque random
//! tokens in an HttpOnly cookie; only the token's SHA-256 hash is
//! stored (D7), so a database leak does not leak live sessions.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::Instant;

use argon2::password_hash::{PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::header::{self, HeaderMap};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::config::Config;
use crate::error::AppError;
use crate::http::AppState;
use crate::profiles::{list_profiles_for, ProfileDto};

pub const SESSION_COOKIE: &str = "parlour_session";
const SESSION_TTL_DAYS: i64 = 30;

// ---------------------------------------------------------------------------
// Passwords and tokens
// ---------------------------------------------------------------------------

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let mut salt_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| AppError::Internal(format!("salt: {e}")))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("password hash: {e}")))
}

pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Verify against a real Argon2 hash of a throwaway password, so a
/// login for a nonexistent user costs the same as a real one.
fn verify_dummy(password: &str) -> bool {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash =
        DUMMY.get_or_init(|| hash_password("not-a-real-password").expect("dummy hash must build"));
    verify_password(password, hash)
}

/// A fresh opaque token: returns (raw value for the client,
/// SHA-256 hex hash for the database).
pub fn new_token() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let hash = token_hash(&raw);
    (raw, hash)
}

pub fn token_hash(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub fn validate_username(username: &str) -> Result<(), AppError> {
    let ok = (3..=32).contains(&username.len())
        && username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if ok {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "username must be 3-32 characters: letters, digits, _ . -".into(),
        ))
    }
}

pub fn validate_password(password: &str) -> Result<(), AppError> {
    if (8..=128).contains(&password.len()) {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "password must be 8-128 characters".into(),
        ))
    }
}

pub fn validate_pin(pin: &str) -> Result<(), AppError> {
    if (4..=8).contains(&pin.len()) && pin.chars().all(|c| c.is_ascii_digit()) {
        Ok(())
    } else {
        Err(AppError::BadRequest("PIN must be 4-8 digits".into()))
    }
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
}

#[derive(Debug, Serialize)]
pub struct UserDto {
    pub id: Uuid,
    pub username: String,
    pub is_admin: bool,
}

impl From<&UserRow> for UserDto {
    fn from(u: &UserRow) -> Self {
        Self {
            id: u.id,
            username: u.username.clone(),
            is_admin: u.is_admin,
        }
    }
}

/// Create a user plus their default profile (named after them).
/// Returns the new user's id. Username uniqueness is enforced
/// case-insensitively by the DB index; a clash is a 409.
pub async fn create_user(
    pool: &sqlx::PgPool,
    username: &str,
    password: &str,
    is_admin: bool,
) -> Result<Uuid, AppError> {
    validate_username(username)?;
    validate_password(password)?;
    let hash = hash_password(password)?;
    let mut tx = pool.begin().await.map_err(AppError::from)?;
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(username)
    .bind(&hash)
    .bind(is_admin)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict(format!("username '{username}' is taken"))
        }
        _ => AppError::from(e),
    })?;
    sqlx::query("INSERT INTO profiles (user_id, name) VALUES ($1, $2)")
        .bind(row.0)
        .bind(username)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    tx.commit().await.map_err(AppError::from)?;
    Ok(row.0)
}

/// A3: if the environment names a first admin and no users exist,
/// create that admin (once — later boots ignore the variables).
pub async fn bootstrap_admin(pool: &sqlx::PgPool, config: &Config) -> Result<(), AppError> {
    let (Some(username), Some(password)) = (&config.admin_user, &config.admin_password) else {
        if config.admin_user.is_some() != config.admin_password.is_some() {
            tracing::warn!(
                "PARLOUR_ADMIN_USER and PARLOUR_ADMIN_PASSWORD must be set together; ignoring"
            );
        }
        return Ok(());
    };
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(AppError::from)?;
    if count.0 > 0 {
        tracing::debug!("users exist; admin bootstrap variables ignored");
        return Ok(());
    }
    let id = create_user(pool, username, password, true).await?;
    tracing::info!(%id, username = %username, "bootstrapped first admin from environment");
    Ok(())
}

// ---------------------------------------------------------------------------
// Session cookie + request authentication
// ---------------------------------------------------------------------------

fn session_cookie_header(raw_token: &str, secure: bool) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE}={raw_token}; HttpOnly; Path=/; SameSite=Lax; Max-Age={}",
        SESSION_TTL_DAYS * 24 * 3600
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn clear_cookie_header(secure: bool) -> String {
    let mut cookie = format!("{SESSION_COOKIE}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

fn cookie_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in header.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// The client's IP address, if the server was started with
/// connect-info (it is in `main`); tests and other embedders may
/// not provide it, so this extractor never fails.
pub struct ClientIp(pub Option<IpAddr>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip()),
        ))
    }
}

/// The authenticated user behind a request, plus their session.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub username: String,
    pub is_admin: bool,
    pub session_id: Uuid,
    pub active_profile_id: Option<Uuid>,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = cookie_from_headers(&parts.headers, SESSION_COOKIE)
            .ok_or_else(|| AppError::Unauthorized("not signed in".into()))?;
        let row: Option<(Uuid, String, bool, Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT u.id, u.username, u.is_admin, s.id, s.active_profile_id
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now() AND u.disabled_at IS NULL",
        )
        .bind(token_hash(&raw))
        .fetch_optional(&state.pool)
        .await
        .map_err(AppError::from)?;
        let (user_id, username, is_admin, session_id, active_profile_id) =
            row.ok_or_else(|| AppError::Unauthorized("session expired".into()))?;
        // Keep last_seen fresh without writing on every request.
        let _ = sqlx::query(
            "UPDATE sessions SET last_seen_at = now()
             WHERE id = $1 AND last_seen_at < now() - interval '5 minutes'",
        )
        .bind(session_id)
        .execute(&state.pool)
        .await;
        Ok(Self {
            user_id,
            username,
            is_admin,
            session_id,
            active_profile_id,
        })
    }
}

/// A7: admin-only routes. A non-admin gets a 404 — admin surfaces
/// do not exist as far as they can tell, never a 403.
pub struct AdminUser(pub AuthUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if user.is_admin {
            Ok(Self(user))
        } else {
            Err(AppError::NotFound("no such route".into()))
        }
    }
}

async fn create_session(
    state: &AppState,
    user_id: Uuid,
    headers: &HeaderMap,
    ip: Option<IpAddr>,
) -> Result<String, AppError> {
    let (raw, hash) = new_token();
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(300).collect::<String>());
    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, expires_at, user_agent, ip)
         VALUES ($1, $2, now() + ($3 || ' days')::interval, $4, $5)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(SESSION_TTL_DAYS.to_string())
    .bind(user_agent)
    .bind(ip.map(|a| a.to_string()))
    .execute(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(raw)
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    user: UserDto,
    profiles: Vec<ProfileDto>,
}

async fn login(
    State(state): State<AppState>,
    connect: ClientIp,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let now = Instant::now();
    let ip_key = format!(
        "ip:{}",
        connect.0.map(|ip| ip.to_string()).unwrap_or_default()
    );
    let user_key = format!("user:{}", body.username.to_lowercase());
    for key in [&ip_key, &user_key] {
        if let Some(remaining) = state.limiter.locked_for(key, now) {
            return Err(AppError::RateLimited(remaining.as_secs().max(1)));
        }
    }

    let user: Option<UserRow> = sqlx::query_as(
        "SELECT id, username, password_hash, is_admin FROM users
         WHERE lower(username) = lower($1) AND disabled_at IS NULL",
    )
    .bind(&body.username)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::from)?;

    let verified = match &user {
        Some(u) => verify_password(&body.password, &u.password_hash),
        None => verify_dummy(&body.password),
    };
    let Some(user) = user.filter(|_| verified) else {
        state.limiter.record_failure(&ip_key, now);
        state.limiter.record_failure(&user_key, now);
        return Err(AppError::Unauthorized(
            "invalid username or password".into(),
        ));
    };

    state.limiter.record_success(&ip_key);
    state.limiter.record_success(&user_key);
    let raw = create_session(&state, user.id, &headers, connect.0).await?;
    let profiles = list_profiles_for(&state.pool, user.id).await?;
    let body = LoginResponse {
        user: UserDto::from(&user),
        profiles,
    };
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie_header(&raw, state.cookie_secure)
            .parse()
            .expect("cookie header is valid"),
    );
    Ok(response)
}

async fn logout(State(state): State<AppState>, user: AuthUser) -> Result<Response, AppError> {
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(user.session_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    let mut response = (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        clear_cookie_header(state.cookie_secure)
            .parse()
            .expect("cookie header is valid"),
    );
    Ok(response)
}

#[derive(Serialize)]
struct MeResponse {
    user: UserDto,
    active_profile: Option<ProfileDto>,
}

async fn me(State(state): State<AppState>, user: AuthUser) -> Result<Json<MeResponse>, AppError> {
    let active_profile = match user.active_profile_id {
        Some(pid) => crate::profiles::get_profile_for(&state.pool, user.user_id, pid).await?,
        None => None,
    };
    Ok(Json(MeResponse {
        user: UserDto {
            id: user.user_id,
            username: user.username.clone(),
            is_admin: user.is_admin,
        },
        active_profile,
    }))
}

#[derive(Serialize, FromRow)]
struct SessionDto {
    id: Uuid,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    user_agent: Option<String>,
    ip: Option<String>,
    current: bool,
}

async fn list_sessions(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<SessionDto>>, AppError> {
    let rows = sqlx::query_as::<_, SessionDto>(
        "SELECT id, created_at, last_seen_at, expires_at, user_agent, ip,
                (id = $2) AS current
         FROM sessions WHERE user_id = $1 AND expires_at > now()
         ORDER BY last_seen_at DESC",
    )
    .bind(user.user_id)
    .bind(user.session_id)
    .fetch_all(&state.pool)
    .await
    .map_err(AppError::from)?;
    Ok(Json(rows))
}

async fn revoke_session(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("no such session".into()))?;
    let done = sqlx::query("DELETE FROM sessions WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.user_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound("no such session".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// Change the signed-in user's password. The current password must
/// check out (401 otherwise), the new one must pass the same
/// validation as sign-up and differ from the current one.
/// Changing the password revokes every OTHER session for the
/// user; the session making the change stays signed in.
async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<StatusCode, AppError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT password_hash FROM users WHERE id = $1 AND disabled_at IS NULL")
            .bind(user.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(AppError::from)?;
    let Some((stored_hash,)) = row else {
        return Err(AppError::Unauthorized("not signed in".into()));
    };
    if !verify_password(&body.current_password, &stored_hash) {
        return Err(AppError::Unauthorized(
            "current password is incorrect".into(),
        ));
    }
    validate_password(&body.new_password)?;
    if body.new_password == body.current_password {
        return Err(AppError::BadRequest(
            "new password must be different from the current password".into(),
        ));
    }
    let hash = hash_password(&body.new_password)?;
    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(&hash)
        .bind(user.user_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id != $2")
        .bind(user.user_id)
        .bind(user.session_id)
        .execute(&state.pool)
        .await
        .map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/change-password", post(change_password))
        .route("/api/v1/auth/sessions", get(list_sessions))
        .route("/api/v1/auth/sessions/{id}", delete(revoke_session))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong horse", &hash));
        assert!(!verify_password("anything", "not-a-hash"));
    }

    #[test]
    fn tokens_are_unique_and_hash_stably() {
        let (raw1, hash1) = new_token();
        let (raw2, _) = new_token();
        assert_ne!(raw1, raw2);
        assert_eq!(raw1.len(), 64);
        assert_eq!(token_hash(&raw1), hash1);
        assert_ne!(token_hash(&raw1), raw1);
    }

    #[test]
    fn validation_rules() {
        assert!(validate_username("jordan").is_ok());
        assert!(validate_username("ab").is_err());
        assert!(validate_username("has space").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("short").is_err());
        assert!(validate_pin("1234").is_ok());
        assert!(validate_pin("12a4").is_err());
        assert!(validate_pin("123").is_err());
    }
}
