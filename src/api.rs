//! Read-only JSON API.
use crate::{AppState, calendar, calendar::WeekPath, model, model::YearDocument, queries};
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

/// Routes under `/api`. `/api/{year}` overlaps a catch-all, so every other path beneath `/api`
/// reaches the router fallback, which answers it with [`not_found`] (see [`is_api_path`]).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/{year}/{week}", get(week_handler))
        .route(
            "/api/{year}",
            get(year_handler).fallback(year_method_fallback),
        )
        .route("/api", any(not_found))
        .route("/api/", any(not_found))
}

/// Whether `path` is `/api` or beneath it: unmatched ones get a JSON 404, not the HTML page.
pub fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
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
        WeekPath::Redirect(iso_year, iso_week) => {
            Redirect::permanent(&format!("/api{}", calendar::week_path(iso_year, iso_week)))
                .into_response()
        }
        WeekPath::Invalid => invalid(),
    }
}

/// `GET /api/<year>`: the year page's listing and status rule, from one query.
async fn year_handler(
    State(state): State<AppState>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let Some(year) = path
        .ok()
        .and_then(|Path(segment)| calendar::parse_year_path(&segment))
    else {
        return not_found().await;
    };
    let current_year = calendar::iso_week_of(state.clock.today()).0;
    let listing = match queries::load_year(&state.pool, year).await {
        Ok(listing) => listing,
        Err(db_error) => return unavailable(&db_error),
    };
    // A year without plans is a 404, except the current one (as for the page).
    if listing.weeks.is_empty() && year != current_year {
        return error(
            StatusCode::NOT_FOUND,
            "year_not_found",
            "No meal plans available yet.",
        );
    }
    json(
        StatusCode::OK,
        &YearDocument {
            schema_version: model::SCHEMA_VERSION,
            year,
            previous_year: listing.previous_year,
            next_year: listing.next_year,
            weeks: listing.weeks,
        },
    )
}

/// Non-GET methods: 405 on a year path, the JSON not-found response on any other single segment.
async fn year_method_fallback(path: Result<Path<String>, PathRejection>) -> Response {
    match path {
        Ok(Path(segment)) if calendar::parse_year_path(&segment).is_some() => (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET,HEAD")],
        )
            .into_response(),
        _ => not_found().await,
    }
}
