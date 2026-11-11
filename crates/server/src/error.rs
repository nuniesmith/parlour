//! HTTP error mapping (task F6).
//!
//! API errors are always JSON: `{ "error": { "code", "message" } }`.
//! Internal details are logged, never sent to the client.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("rate limited, retry in {0}s")]
    RateLimited(u64),
    #[error("database error")]
    Db(#[from] parlour_db::DbError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    fn parts(&self) -> (StatusCode, &'static str, String) {
        match self {
            AppError::NotFound(what) => (StatusCode::NOT_FOUND, "not_found", what.clone()),
            AppError::BadRequest(why) => (StatusCode::BAD_REQUEST, "bad_request", why.clone()),
            AppError::Unauthorized(why) => (StatusCode::UNAUTHORIZED, "unauthorized", why.clone()),
            AppError::Conflict(why) => (StatusCode::CONFLICT, "conflict", why.clone()),
            AppError::RateLimited(secs) => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                format!("too many failed attempts — retry in {secs} seconds"),
            ),
            AppError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "the database could not complete that request".to_string(),
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "something went wrong on the server".to_string(),
            ),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.parts();
        if status.is_server_error() {
            tracing::error!(error = %self, code, "request failed");
        }
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
