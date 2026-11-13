pub mod api;
pub mod assets;
pub mod calendar;
pub mod db;
pub mod mcp;
pub mod model;
pub mod queries;
pub mod save;

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Router, middleware};
use calendar::{Clock, DatePath, WeekPath};
use chrono::NaiveDate;
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
        .route(
            "/{segment}",
            get(segment_handler).fallback(year_method_fallback),
        )
        .route("/{year}/{week}", get(week_handler))
        .merge(api::routes())
        .fallback(fallback)
        .layer(middleware::map_response(no_store))
        .with_state(AppState {
            pool: read_pool,
            clock,
        })
        .merge(assets::routes())
}

/// Write pool and request-validation settings for `/mcp`.
pub struct McpConfig {
    /// Dedicated writable pool; used only by `save_week`.
    pub write_pool: SqlitePool,
    /// Accepted `Host` values; empty means the loopback defaults.
    pub allowed_hosts: Vec<String>,
    /// Accepted `Origin` values; empty rejects every request that carries an `Origin` header.
    pub allowed_origins: Vec<String>,
}

/// `app(read_pool, clock)` plus the `/mcp` endpoint.
pub fn app_with_mcp(read_pool: SqlitePool, clock: Clock, mcp: McpConfig) -> Router {
    let service = mcp::service(
        read_pool.clone(),
        mcp.write_pool,
        clock.clone(),
        mcp.allowed_hosts,
        mcp.allowed_origins,
    );
    app(read_pool, clock).nest_service("/mcp", service)
}

/// Dynamic HTML and JSON are never cached; assets keep their own headers.
async fn no_store(mut res: Response) -> Response {
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

/// Unmatched paths: JSON under `/api`, the HTML not-found page elsewhere.
async fn fallback(uri: Uri) -> Response {
    if api::is_api_path(uri.path()) {
        api::not_found().await
    } else {
        not_found().await
    }
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

/// `YYYY-MM-DD` for the page data's `today`.
fn today_string(today: NaiveDate) -> String {
    today.format("%Y-%m-%d").to_string()
}

/// The invalid week page; a failed calendar read is the database failure page.
async fn invalid(state: &AppState, today: NaiveDate) -> Response {
    let calendar = match queries::calendar_data(&state.pool).await {
        Ok(calendar) => calendar,
        Err(error) => return unavailable(&error),
    };
    page_response(
        StatusCode::NOT_FOUND,
        "Invalid week — Meal Plan",
        &Page::Invalid {
            today: today_string(today),
            planned_weeks: calendar.planned_weeks,
            marked_days: calendar.marked_days,
            displayed_date: None,
        },
    )
}

/// The invalid date page; a failed calendar read is the database failure page.
async fn invalid_date(state: &AppState, today: NaiveDate) -> Response {
    let calendar = match queries::calendar_data(&state.pool).await {
        Ok(calendar) => calendar,
        Err(error) => return unavailable(&error),
    };
    page_response(
        StatusCode::NOT_FOUND,
        "Invalid date — Meal Plan",
        &Page::InvalidDate {
            today: today_string(today),
            planned_weeks: calendar.planned_weeks,
            marked_days: calendar.marked_days,
            displayed_date: None,
        },
    )
}

/// Render one ISO week with `displayed` as the shown day: the stored plan, or the "missing" page with
/// `missing_status`. `today` is read from the clock once per request by the caller. `displayed` is `None`
/// for the week's default day (today inside the current week, otherwise the Monday).
async fn show_week(
    state: &AppState,
    today: NaiveDate,
    iso_year: i32,
    iso_week: u32,
    displayed: Option<NaiveDate>,
    missing_status: StatusCode,
) -> Response {
    let Some(week) = queries::week_summary(iso_year, iso_week) else {
        return invalid(state, today).await;
    };
    let displayed = displayed.unwrap_or_else(|| {
        if calendar::iso_week_of(today) == (iso_year, iso_week) {
            today
        } else {
            calendar::week_dates(iso_year, iso_week).expect("summary exists")[0]
        }
    });
    let title = format!("Meal Plan — {iso_year} week {iso_week}");

    let (document, calendar) = match queries::load_week_page(&state.pool, iso_year, iso_week).await
    {
        Ok(loaded) => loaded,
        Err(error) => return unavailable(&error),
    };
    if let Some(document) = document {
        return page_response(
            StatusCode::OK,
            &title,
            &Page::Week {
                today: today_string(today),
                planned_weeks: calendar.planned_weeks,
                marked_days: calendar.marked_days,
                displayed_date: today_string(displayed),
                document,
            },
        );
    }

    page_response(
        missing_status,
        &title,
        &Page::Missing {
            today: today_string(today),
            planned_weeks: calendar.planned_weeks,
            marked_days: calendar.marked_days,
            displayed_date: today_string(displayed),
            week,
        },
    )
}

async fn root_handler(State(state): State<AppState>) -> Response {
    let today = state.clock.today();
    let (iso_year, iso_week) = calendar::iso_week_of(today);
    show_week(&state, today, iso_year, iso_week, None, StatusCode::OK).await
}

async fn week_handler(
    State(state): State<AppState>,
    path: Result<Path<(String, String)>, PathRejection>,
) -> Response {
    // Malformed percent-encoding is an invalid week, not a plain-text 400.
    let today = state.clock.today();
    let Ok(Path((year, week))) = path else {
        return invalid(&state, today).await;
    };
    match calendar::classify_week_path(&year, &week) {
        WeekPath::Canonical(iso_year, iso_week) => {
            show_week(
                &state,
                today,
                iso_year,
                iso_week,
                None,
                StatusCode::NOT_FOUND,
            )
            .await
        }
        WeekPath::Redirect(iso_year, iso_week) => {
            Redirect::permanent(&calendar::week_path(iso_year, iso_week)).into_response()
        }
        WeekPath::Invalid => invalid(&state, today).await,
    }
}

/// `/{segment}`: a year page, a date page, or the plain not-found page.
async fn segment_handler(
    State(state): State<AppState>,
    path: Result<Path<String>, PathRejection>,
) -> Response {
    let Ok(Path(segment)) = path else {
        return not_found().await;
    };
    if let Some(year) = calendar::parse_year_path(&segment) {
        return year_page(&state, year).await;
    }
    match calendar::classify_date_path(&segment) {
        Some(DatePath::Valid(date)) => {
            let today = state.clock.today();
            let (iso_year, iso_week) = calendar::iso_week_of(date);
            show_week(
                &state,
                today,
                iso_year,
                iso_week,
                Some(date),
                StatusCode::NOT_FOUND,
            )
            .await
        }
        Some(DatePath::Invalid) => invalid_date(&state, state.clock.today()).await,
        None => not_found().await,
    }
}

async fn year_page(state: &AppState, year: i32) -> Response {
    let current_year = calendar::iso_week_of(state.clock.today()).0;
    let listing = match queries::load_year(&state.pool, year).await {
        Ok(listing) => listing,
        Err(error) => return unavailable(&error),
    };
    // A year without plans is a 404, except the current one.
    let status = if listing.weeks.is_empty() && year != current_year {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::OK
    };
    page_response(
        status,
        &format!("{year} — Meal Plan"),
        &Page::Year {
            year,
            previous_year: listing.previous_year,
            next_year: listing.next_year,
            weeks: listing.weeks,
        },
    )
}

/// Non-GET methods: 405 on a year or date-shaped path, the plain not-found response on any other single segment.
async fn year_method_fallback(path: Result<Path<String>, PathRejection>) -> Response {
    match path {
        Ok(Path(segment))
            if calendar::parse_year_path(&segment).is_some()
                || calendar::classify_date_path(&segment).is_some() =>
        {
            (
                StatusCode::METHOD_NOT_ALLOWED,
                [(header::ALLOW, "GET,HEAD")],
            )
                .into_response()
        }
        _ => not_found().await,
    }
}
