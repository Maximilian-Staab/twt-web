//! RFC 9457 problem+json errors (see openapi.yaml `Problem` schema).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub type_: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Problem {
    pub fn new(status: StatusCode, type_: &str, title: &str) -> Self {
        Self {
            type_: type_.to_string(),
            title: title.to_string(),
            status: status.as_u16(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "Unauthorized").with_detail(detail)
    }

    // Unused-for-now helpers kept for the OpenAPI-documented problem types.
    #[allow(dead_code)]
    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "Forbidden").with_detail(detail)
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not-found", "Not found")
    }

    pub fn validation(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "validation", "Validation failed").with_detail(detail)
    }

    pub fn precondition_failed() -> Self {
        Self::new(
            StatusCode::PRECONDITION_FAILED,
            "stale-write",
            "Stale write",
        )
        .with_detail("stored updatedAt is newer than the precondition")
    }

    pub fn payload_too_large(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload-too-large",
            "Payload too large",
        )
        .with_detail(detail)
    }

    pub fn gone(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::GONE, "cursor-expired", "Sync cursor expired").with_detail(detail)
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal error",
        )
        .with_detail(detail)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut resp = (status, axum::Json(&self)).into_response();
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/problem+json"),
        );
        // RFC 6750 §3: 401s carry the Bearer challenge (DESIGN §9).
        if status == StatusCode::UNAUTHORIZED {
            resp.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static(r#"Bearer error="invalid_token""#),
            );
        }
        resp
    }
}

impl From<sqlx::Error> for Problem {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = ?e, "database error");
        Problem::internal("database error")
    }
}
