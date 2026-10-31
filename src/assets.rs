//! Embedded, content-hashed web assets and the HTML shell for week pages.
//!
//! The assets are produced by `build.rs` (JSX compiled with oxc) and embedded in the binary.

use axum::Router;
use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

include!(concat!(env!("OUT_DIR"), "/assets_generated.rs"));

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Routes serving embedded, content-hashed assets under `/assets/`.
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/assets/{name}", get(asset))
}

async fn asset(Path(name): Path<String>) -> Response {
    match ASSETS.iter().find(|asset| asset.name == name) {
        Some(asset) => (
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(asset.content_type),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE)),
            ],
            asset.bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Full HTML document for any page. `title` is plain text (escaped here).
/// `page_json` is the [`crate::model::Page`] payload, already made script-safe by the caller.
pub fn page(title: &str, page_json: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<link rel="stylesheet" href="{CSS_PATH}">
</head>
<body>
<div id="root"></div>
<script id="sous-page" type="application/json">{page_json}</script>
<script defer src="{REACT_PATH}"></script>
<script defer src="{REACT_DOM_PATH}"></script>
<script defer src="{APP_PATH}"></script>
</body>
</html>
"#,
        title = escape_html(title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn get_path(path: &str) -> Response {
        routes::<()>()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn serves_every_asset_with_type_and_immutable_caching() {
        assert!(!ASSETS.is_empty());
        for asset in ASSETS {
            let response = get_path(&format!("/assets/{}", asset.name)).await;
            assert_eq!(response.status(), StatusCode::OK, "{}", asset.name);
            assert_eq!(response.headers()[header::CONTENT_TYPE], asset.content_type);
            assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(body.as_ref(), asset.bytes);
        }
    }

    #[tokio::test]
    async fn unknown_asset_is_404() {
        assert_eq!(
            get_path("/assets/nope.js").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn css_references_only_hashed_local_fonts() {
        let css = ASSETS.iter().find(|a| a.name.ends_with(".css")).unwrap();
        let css = std::str::from_utf8(css.bytes).unwrap();
        assert!(!css.contains("url(fonts/"));
        for font in ASSETS.iter().filter(|a| a.name.ends_with(".woff2")) {
            assert!(
                css.contains(&format!("/assets/{}", font.name)),
                "{}",
                font.name
            );
        }
    }

    #[test]
    fn week_page_escapes_title_and_uses_only_local_assets() {
        let page = page("<b>\"x\" & 'y'</b>", r#"{"a":1}"#);
        assert!(page.contains("<title>&lt;b&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/b&gt;</title>"));
        assert!(
            page.contains(r#"<script id="sous-page" type="application/json">{"a":1}</script>"#)
        );
        for attr in ["href=\"", "src=\""] {
            for part in page.split(attr).skip(1) {
                let url = part.split('"').next().unwrap();
                assert!(url.starts_with("/assets/"), "{url}");
                let name = url.trim_start_matches("/assets/");
                assert!(ASSETS.iter().any(|a| a.name == name), "{url}");
            }
        }
        assert!(!page.contains("http://") && !page.contains("https://"));
    }

    #[test]
    fn generated_js_has_no_jsx() {
        let app = ASSETS.iter().find(|a| a.name.starts_with("app.")).unwrap();
        let js = std::str::from_utf8(app.bytes).unwrap();
        assert!(js.contains("createElement"));
        assert!(!js.contains("</"), "unexpected JSX closing tag");
        assert!(!js.contains("className=\""), "unexpected JSX attribute");
    }
}
