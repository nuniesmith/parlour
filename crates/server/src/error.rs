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
    // Part of the error vocabulary from day one; first constructed
    // by the input-validating routes in Milestone 1.2.
    #[allow(dead_code)]
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("database error")]
    Db(#[from] parlour_db::DbError),
    #[allow(dead_code)]
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    fn parts(&self) -> (StatusCode, &'static str, String) {
        match self {
            AppError::NotFound(what) => (StatusCode::NOT_FOUND, "not_found", what.clone()),
            AppError::BadRequest(why) => (StatusCode::BAD_REQUEST, "bad_request", why.clone()),
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
