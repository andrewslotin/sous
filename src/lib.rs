pub mod assets;
pub mod calendar;
pub mod db;
pub mod model;
pub mod queries;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Router, middleware};
use sqlx::SqlitePool;

/// Build the application router around the read-only pool.
pub fn app(read_pool: SqlitePool) -> Router {
    Router::new()
        .route("/{year}/{week}", get(week_handler))
        .fallback(not_found)
        .layer(middleware::map_response(no_store_for_html))
        .with_state(read_pool)
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

async fn week_handler(
    State(pool): State<SqlitePool>,
    Path((year, week)): Path<(String, String)>,
) -> Response {
    let Some((iso_year, iso_week)) = calendar::parse_week_path(&year, &week) else {
        return not_found().await;
    };
    match queries::load_week(&pool, iso_year, iso_week).await {
        Ok(Some(doc)) => {
            let title = format!("Meal Plan — {iso_year} week {iso_week}");
            html(
                StatusCode::OK,
                assets::week_page(&title, &doc.to_script_json()),
            )
        }
        Ok(None) => not_found().await,
        Err(err) => {
            tracing::error!(%err, iso_year, iso_week, "failed to load week");
            html(
                StatusCode::SERVICE_UNAVAILABLE,
                message_page("Unavailable", "Meal plans are temporarily unavailable."),
            )
        }
    }
}
