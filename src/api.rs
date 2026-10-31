//! Read-only JSON API.
use crate::{AppState, calendar, calendar::WeekPath, queries};
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use serde::Serialize;

/// JSON error body: `{"error":{"code":..,"message":..}}`.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: &'static str,
}

/// Routes under `/api`; any other path beneath it is a JSON 404.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/weeks/{year}/{week}", get(week_handler))
        .route("/api", any(not_found))
        .route("/api/", any(not_found))
        .route("/api/{*path}", any(not_found))
}

/// Serialize `body` as `application/json` (exactly, without a charset) with `status`.
fn json<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let mut res = (status, Json(body)).into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    res
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    json(
        status,
        &ApiError {
            error: ErrorBody { code, message },
        },
    )
}

pub async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "Not found.")
}

fn invalid() -> Response {
    error(StatusCode::NOT_FOUND, "invalid_week", "Invalid week.")
}

fn unavailable(db_error: &sqlx::Error) -> Response {
    tracing::error!(error = %db_error, "database error");
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "service_unavailable",
        "Meal plans are temporarily unavailable.",
    )
}

async fn week_handler(
    State(state): State<AppState>,
    path: Result<Path<(String, String)>, PathRejection>,
) -> Response {
    // Malformed percent-encoding is an invalid week, not a plain-text 400.
    let Ok(Path((year, week))) = path else {
        return invalid();
    };
    match calendar::classify_week_path(&year, &week) {
        WeekPath::Canonical(iso_year, iso_week) => {
            match queries::load_week(&state.pool, iso_year, iso_week).await {
                Ok(Some(document)) => json(StatusCode::OK, &document),
                Ok(None) if queries::week_summary(iso_year, iso_week).is_none() => invalid(),
                Ok(None) => error(
                    StatusCode::NOT_FOUND,
                    "week_not_found",
                    "No meal plan for this week yet.",
                ),
                Err(db_error) => unavailable(&db_error),
            }
        }
        WeekPath::Redirect(iso_year, iso_week) => Redirect::permanent(&format!(
            "/api/weeks{}",
            calendar::week_path(iso_year, iso_week)
        ))
        .into_response(),
        WeekPath::Invalid => invalid(),
    }
}
