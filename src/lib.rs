pub mod assets;
pub mod calendar;
pub mod db;
pub mod model;
pub mod queries;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Router, middleware};
use calendar::{Clock, WeekPath};
use chrono::{Datelike, NaiveDate};
use model::Page;
use sqlx::SqlitePool;

/// Shared request state: the read-only pool and the service clock.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub clock: Clock,
}

/// Build the application router around the read-only pool and the service clock.
pub fn app(read_pool: SqlitePool, clock: Clock) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/weeks", get(weeks_handler))
        .route("/{year}/{week}", get(week_handler))
        .fallback(not_found)
        .layer(middleware::map_response(no_store_for_html))
        .with_state(AppState {
            pool: read_pool,
            clock,
        })
        .merge(assets::routes())
}

/// Dynamic HTML is never cached; assets keep their own headers.
async fn no_store_for_html(mut res: Response) -> Response {
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn html(status: StatusCode, body: String) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

fn message_page(title: &str, message: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title></head><body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    )
}

async fn not_found() -> Response {
    html(
        StatusCode::NOT_FOUND,
        message_page("Not found", "No meal plan exists at this address."),
    )
}

fn page_response(status: StatusCode, title: &str, page: &Page) -> Response {
    html(status, assets::page(title, &page.to_script_json()))
}

fn unavailable(error: &sqlx::Error) -> Response {
    tracing::error!(%error, "database error");
    page_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "Unavailable — Meal Plan",
        &Page::Unavailable,
    )
}

fn invalid() -> Response {
    page_response(
        StatusCode::NOT_FOUND,
        "Invalid week — Meal Plan",
        &Page::Invalid,
    )
}

/// Render one ISO week: the stored plan, or the "missing" page with `missing_status`.
/// `today` is read from the clock once per request by the caller.
async fn show_week(
    state: &AppState,
    today: NaiveDate,
    iso_year: i32,
    iso_week: u32,
    missing_status: StatusCode,
) -> Response {
    let title = format!("Meal Plan — {iso_year} week {iso_week}");

    let document = match queries::load_week(&state.pool, iso_year, iso_week).await {
        Ok(document) => document,
        Err(error) => return unavailable(&error),
    };
    if let Some(document) = document {
        let selected_weekday = if calendar::iso_week_of(today) == (iso_year, iso_week) {
            today.weekday().number_from_monday()
        } else {
            1
        };
        return page_response(
            StatusCode::OK,
            &title,
            &Page::Week {
                selected_weekday,
                document,
            },
        );
    }

    let Some(week) = queries::week_summary(iso_year, iso_week) else {
        return invalid();
    };
    page_response(missing_status, &title, &Page::Missing { week })
}

async fn root_handler(State(state): State<AppState>) -> Response {
    let today = state.clock.today();
    let (iso_year, iso_week) = calendar::iso_week_of(today);
    show_week(&state, today, iso_year, iso_week, StatusCode::OK).await
}

async fn week_handler(
    State(state): State<AppState>,
    Path((year, week)): Path<(String, String)>,
) -> Response {
    match calendar::classify_week_path(&year, &week) {
        WeekPath::Canonical(iso_year, iso_week) => {
            let today = state.clock.today();
            show_week(&state, today, iso_year, iso_week, StatusCode::NOT_FOUND).await
        }
        WeekPath::Redirect(iso_year, iso_week) => {
            Redirect::permanent(&calendar::week_path(iso_year, iso_week)).into_response()
        }
        WeekPath::Invalid => invalid(),
    }
}

async fn weeks_handler(State(state): State<AppState>) -> Response {
    match queries::list_weeks(&state.pool).await {
        Ok(weeks) => page_response(
            StatusCode::OK,
            "All weeks — Meal Plan",
            &Page::Archive { weeks },
        ),
        Err(error) => unavailable(&error),
    }
}
