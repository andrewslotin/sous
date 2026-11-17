//! The household profile over HTTP: the `/profile` page, `GET /api/profile` and `PUT /api/profile/{key}`.
use crate::api::{self, ErrorBody};
use crate::model::{Page, ProfileSection, SectionKey};
use crate::profile::{self, UpdateError};
use crate::{AppState, mcp, page_response};
use axum::Router;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, Request, State};
use axum::http::uri::Authority;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// Largest accepted request body, in bytes.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// What `PUT /api/profile/{key}` needs: the write pool and the request protections shared with `/mcp`.
pub struct ProfileWrite {
    write_pool: SqlitePool,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
}

impl ProfileWrite {
    /// An empty `allowed_hosts` means the loopback defaults, as for `/mcp`.
    pub fn new(
        write_pool: SqlitePool,
        allowed_hosts: Vec<String>,
        allowed_origins: Vec<String>,
    ) -> Self {
        let allowed_hosts = if allowed_hosts.is_empty() {
            mcp::DEFAULT_ALLOWED_HOSTS
                .iter()
                .map(|host| host.to_string())
                .collect()
        } else {
            allowed_hosts
        };
        ProfileWrite {
            write_pool,
            allowed_hosts,
            allowed_origins,
        }
    }
}

/// `/profile`, matched before the single-segment year and date route.
pub fn page_routes() -> Router<AppState> {
    Router::new().route("/profile", get(page).fallback(method_not_allowed_get))
}

/// `/api/profile` and `/api/profile/{key}`, matched before the API's year, date and week routes.
pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/profile",
            get(profile_json).fallback(method_not_allowed_get),
        )
        .route(
            "/api/profile/{key}",
            put(put_section).fallback(method_not_allowed_put),
        )
}

async fn method_not_allowed_get() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "GET,HEAD")],
    )
        .into_response()
}

async fn method_not_allowed_put() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "PUT")]).into_response()
}

async fn page(State(state): State<AppState>) -> Response {
    match profile::load_profile(&state.pool).await {
        Ok(profile) => page_response(
            StatusCode::OK,
            "Household — Meal Plan",
            &Page::Profile {
                sections: profile.sections,
            },
        ),
        Err(error) => crate::unavailable(&error),
    }
}

async fn profile_json(State(state): State<AppState>) -> Response {
    match profile::load_profile(&state.pool).await {
        Ok(profile) => api::json(StatusCode::OK, &profile),
        Err(error) => api::unavailable(&error),
    }
}

/// The 409 body: the usual error object plus the section as now saved.
#[derive(Serialize)]
struct RevisionConflictBody {
    error: ErrorBody,
    current: ProfileSection,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PutBody {
    text: String,
    expected_revision: String,
}

fn forbidden() -> Response {
    api::error(
        StatusCode::FORBIDDEN,
        "forbidden",
        "This request is not allowed.",
    )
}

async fn put_section(
    State(state): State<AppState>,
    path: Result<Path<String>, PathRejection>,
    request: Request,
) -> Response {
    let Some(write) = state.profile_write.as_deref() else {
        return api::error(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "Meal plans are temporarily unavailable.",
        );
    };
    let (parts, body) = request.into_parts();

    // 1. Host and Origin.
    if !request_allowed(&parts.uri, &parts.headers, write) {
        return forbidden();
    }
    // 2. The section.
    let Some(key) = path.ok().and_then(|Path(key)| SectionKey::parse(&key)) else {
        return api::error(
            StatusCode::NOT_FOUND,
            "section_not_found",
            "No such profile section.",
        );
    };
    // 3. The media type.
    if !is_json(&parts.headers) {
        return api::error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "The request body must be application/json.",
        );
    }
    // 4. The body size, with or without Content-Length.
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
        return api::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "The request body must be at most 64 KiB.",
        );
    };
    // 5. The shape.
    let Ok(body) = serde_json::from_slice::<PutBody>(&bytes) else {
        return invalid_input(
            "The body must be a JSON object with text and expectedRevision strings, and no other fields.",
        );
    };
    match profile::update_section(&write.write_pool, key, &body.text, &body.expected_revision).await
    {
        Ok(saved) => api::json(StatusCode::OK, &saved),
        Err(UpdateError::InvalidInput(_)) => {
            invalid_input("Text must be at most 4,000 characters.")
        }
        Err(UpdateError::Conflict { current }) => api::json(
            StatusCode::CONFLICT,
            &RevisionConflictBody {
                error: ErrorBody {
                    code: "revision_conflict",
                    message: "The section changed since it was read.",
                },
                current,
            },
        ),
        Err(UpdateError::Database(error)) => api::unavailable(&error),
    }
}

fn invalid_input(message: &'static str) -> Response {
    api::error(StatusCode::BAD_REQUEST, "invalid_input", message)
}

/// `application/json`, case-insensitive, parameters allowed.
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

// ---- Host and Origin, with the semantics of rmcp's `/mcp` checks ----

/// `host` lowercased, without IPv6 brackets.
fn normalize_host(host: &str) -> String {
    host.trim_matches('[')
        .trim_matches(']')
        .to_ascii_lowercase()
}

/// The `Host` header (or the URI's authority, as rmcp does) as a normalized host and port.
fn request_authority(uri: &Uri, headers: &HeaderMap) -> Option<(String, Option<u16>)> {
    let authority = match headers.get(header::HOST) {
        Some(value) => Authority::try_from(value.to_str().ok()?).ok()?,
        None => uri.authority()?.clone(),
    };
    Some((normalize_host(authority.host()), authority.port_u16()))
}

/// An allowed host matches when its host does, and its port too if it names one.
fn host_allowed(host: &(String, Option<u16>), allowed_hosts: &[String]) -> bool {
    allowed_hosts
        .iter()
        .map(|allowed| allowed.trim())
        .filter(|allowed| !allowed.is_empty())
        .map(|allowed| match Authority::try_from(allowed) {
            Ok(authority) => (normalize_host(authority.host()), authority.port_u16()),
            Err(_) => (normalize_host(allowed), None),
        })
        .any(|(allowed_host, allowed_port)| {
            allowed_host == host.0 && allowed_port.is_none_or(|port| host.1 == Some(port))
        })
}

fn request_allowed(uri: &Uri, headers: &HeaderMap, write: &ProfileWrite) -> bool {
    let Some(host) = request_authority(uri, headers) else {
        return false;
    };
    if !host_allowed(&host, &write.allowed_hosts) {
        return false;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if let Some(host_header) = headers.get(header::HOST).and_then(|v| v.to_str().ok())
        && (origin == format!("http://{host_header}") || origin == format!("https://{host_header}"))
    {
        return true;
    }
    write
        .allowed_origins
        .iter()
        .any(|allowed| origin_listed(origin, allowed))
}

/// A parsed origin: `None` is the opaque origin `null`.
fn parse_origin(value: &str) -> Option<Option<(String, String, Option<u16>)>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("null") {
        return Some(None);
    }
    let uri = Uri::try_from(value).ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let authority = uri.authority()?;
    Some(Some((
        scheme,
        normalize_host(authority.host()),
        authority.port_u16(),
    )))
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

/// Whether `origin` matches one `allowed_origins` entry as `/mcp` matches it: scheme and host equal;
/// the port equal (default ports implied), or any port when the entry has none or ends in `:*`.
fn origin_listed(origin: &str, allowed: &str) -> bool {
    let Some(origin) = parse_origin(origin) else {
        return false;
    };
    let allowed = allowed.trim();
    let (base, any_port) = match allowed.strip_suffix(":*") {
        Some(base) => (base, true),
        None => (allowed, false),
    };
    let Some(allowed) = parse_origin(base) else {
        return false;
    };
    match (origin, allowed) {
        (None, None) => true,
        (Some((scheme, host, port)), Some((a_scheme, a_host, a_port))) => {
            scheme == a_scheme
                && host == a_host
                && match a_port {
                    _ if any_port => true,
                    None => true,
                    Some(a_port) => port.or_else(|| default_port(&scheme)) == Some(a_port),
                }
        }
        _ => false,
    }
}
