use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use sous::{db, queries};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, SqliteConnection, SqlitePool};
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;

const FIXTURE: &str = include_str!("../fixtures/2026-w44.sql");
const U1: &str = "11111111-1111-4111-8111-111111111111";
const U2: &str = "22222222-2222-4222-8222-222222222222";
const U3: &str = "33333333-3333-4333-8333-333333333333";
const U4: &str = "44444444-4444-4444-8444-444444444444";

struct TestDb {
    _dir: TempDir,
    path: PathBuf,
}

impl TestDb {
    async fn migrated() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        db::migrate(&path).await.unwrap();
        TestDb { _dir: dir, path }
    }

    async fn with_fixture() -> Self {
        let t = Self::migrated().await;
        let mut c = t.writer().await;
        sqlx::raw_sql(FIXTURE).execute(&mut c).await.unwrap();
        t
    }

    async fn writer(&self) -> SqliteConnection {
        SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&self.path)
                .journal_mode(SqliteJournalMode::Wal)
                .foreign_keys(true),
        )
        .await
        .unwrap()
    }

    async fn reader(&self) -> SqlitePool {
        db::open_read_pool(&self.path).await.unwrap()
    }
}

async fn exec(c: &mut SqliteConnection, sql: &str) -> Result<(), sqlx::Error> {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
        .execute(c)
        .await
        .map(|_| ())
}

async fn get(pool: &SqlitePool, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let res = sous::app(pool.clone())
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

fn week_sql(id: &str, y: i32, w: i32) -> String {
    format!("INSERT INTO weeks (id, iso_year, iso_week) VALUES ('{id}', {y}, {w})")
}

fn day_sql(id: &str, week: &str, weekday: i32) -> String {
    format!("INSERT INTO days (id, week_id, weekday) VALUES ('{id}', '{week}', {weekday})")
}

fn meal_sql(id: &str, day: &str, pos: i32, kind: &str, kcal: &str) -> String {
    format!(
        "INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two) \
         VALUES ('{id}', '{day}', {pos}, '{kind}', 'n', 'd', {kcal}, 1, 1, 1, 1, 0)"
    )
}

fn step_sql(id: &str, meal: &str, pos: i32) -> String {
    format!(
        "INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('{id}', '{meal}', {pos}, 't')"
    )
}

#[tokio::test]
async fn migrations_and_fixture_then_rerun_keeps_data() {
    let t = TestDb::with_fixture().await;
    db::migrate(&t.path).await.unwrap();
    let pool = t.reader().await;
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(doc.days.iter().map(|d| d.meals.len()).sum::<usize>(), 7);
}

#[tokio::test]
async fn constraints_reject_bad_data() {
    let t = TestDb::migrated().await;
    let mut c = t.writer().await;
    exec(&mut c, &week_sql(U1, 2026, 44)).await.unwrap();

    // duplicate week, bad uuid shape
    assert!(exec(&mut c, &week_sql(U2, 2026, 44)).await.is_err());
    assert!(exec(&mut c, &week_sql("nope", 2026, 45)).await.is_err());
    // ISO week 53: 2025 has none, 2026 has one
    assert!(exec(&mut c, &week_sql(U2, 2025, 53)).await.is_err());
    exec(&mut c, &week_sql(U2, 2026, 53)).await.unwrap();
    assert!(
        exec(
            &mut c,
            "UPDATE weeks SET iso_year = 2025 WHERE iso_year = 2026 AND iso_week = 53"
        )
        .await
        .is_err()
    );
    // unrepresentable date range
    assert!(exec(&mut c, &week_sql(U3, 9999, 52)).await.is_err());
    exec(&mut c, &week_sql(U3, 9999, 51)).await.unwrap();

    // days
    let missing = "99999999-9999-4999-8999-999999999999";
    let d2 = "55555555-5555-4555-8555-555555555555";
    assert!(exec(&mut c, &day_sql(U4, missing, 1)).await.is_err());
    exec(&mut c, &day_sql(U4, U1, 1)).await.unwrap();
    assert!(exec(&mut c, &day_sql(d2, U1, 1)).await.is_err());
    assert!(exec(&mut c, &day_sql(d2, U1, 8)).await.is_err());

    // meals
    let m1 = "66666666-6666-4666-8666-666666666666";
    let m2 = "77777777-7777-4777-8777-777777777777";
    exec(&mut c, &meal_sql(m1, U4, 0, "Lunch", "100.5"))
        .await
        .unwrap();
    assert!(
        exec(&mut c, &meal_sql(m2, U4, 0, "Lunch", "1"))
            .await
            .is_err()
    );
    assert!(
        exec(&mut c, &meal_sql(m2, U4, 1, "Lunch", "-1"))
            .await
            .is_err()
    );
    assert!(
        exec(&mut c, &meal_sql(m2, U4, 1, "Brunch", "1"))
            .await
            .is_err()
    );
    assert!(
        exec(&mut c, &meal_sql(m2, U4, -1, "Lunch", "1"))
            .await
            .is_err()
    );

    // steps
    let s1 = "88888888-8888-4888-8888-888888888888";
    let s2 = "99999999-9999-4999-8999-999999999990";
    exec(&mut c, &step_sql(s1, m1, 0)).await.unwrap();
    assert!(exec(&mut c, &step_sql(s2, m1, 0)).await.is_err());
    assert!(exec(&mut c, &step_sql(s2, m2, 1)).await.is_err());

    // cascade
    exec(&mut c, &format!("DELETE FROM weeks WHERE id = '{U1}'"))
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM meal_steps")
        .fetch_one(&mut c)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn query_shape_and_ordering() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(doc.week.start_date, "2026-10-26");
    assert_eq!(doc.week.end_date, "2026-11-01");
    assert!(doc.week.subtitle.is_some());
    assert_eq!(doc.days.len(), 7);
    for (i, d) in doc.days.iter().enumerate() {
        assert_eq!(d.weekday as usize, i + 1);
    }
    // Wed: day row without meals; Fri/Sat: no day row.
    assert!(doc.days[2].id.is_some() && doc.days[2].meals.is_empty());
    assert!(doc.days[4].id.is_none() && doc.days[5].id.is_none());
    let mon = &doc.days[0].meals;
    let kinds: Vec<_> = mon.iter().map(|m| m.kind.as_str()).collect();
    assert_eq!(kinds, ["Breakfast", "Lunch", "Dinner", "Snack"]);
    let dinner = &mon[2];
    assert_eq!(dinner.steps.len(), 5);
    assert!(dinner.steps[0].text.starts_with("Mix miso"));
    assert!(dinner.steps[4].text.starts_with("Serve"));
    assert!(dinner.assembly_time.is_none() && dinner.cook_time.is_some());
    assert!(mon[0].tip.is_some() && mon[1].tip.is_none());
    assert!(queries::load_week(&pool, 2026, 45).await.unwrap().is_none());
}

#[tokio::test]
async fn nutrition_round_trips_as_decimals() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    let lunch = &doc.days[0].meals[1];
    assert_eq!(lunch.kcal, 380.5);
    assert_eq!(lunch.protein_g, 16.25);
    assert_eq!(doc.days[0].meals[2].fat_g, 22.4);
    assert!(lunch.for_two);
    assert!(doc.to_script_json().contains("\"kcal\":380.5"));
}

#[tokio::test]
async fn freshness_commit_visible_uncommitted_not() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;

    let mut c = t.writer().await;
    let mut tx = c.begin().await.unwrap();
    sqlx::query("UPDATE meals SET name = 'Renamed meal' WHERE name = 'Walnuts'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let (s, _, body) = get(&pool, "/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    assert!(!body.contains("Renamed meal"));
    tx.commit().await.unwrap();
    let (_, _, body) = get(&pool, "/2026/44").await;
    assert!(body.contains("Renamed meal"));
}

#[tokio::test]
async fn http_statuses_and_headers() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (s, h, body) = get(&pool, "/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert!(body.contains("sous-week"));

    for uri in [
        "/2026/45",
        "/2025/53",
        "/2026/04",
        "/abc/1",
        "/2026/abc",
        "/0000/1",
        "/10000/1",
        "/nothing",
    ] {
        let (s, h, _) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8", "{uri}");
    }

    let res = sous::app(pool.clone())
        .oneshot(Request::post("/2026/44").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn http_503_when_database_unavailable() {
    // Database without tables.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.db");
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    SqliteConnection::connect_with(&opts).await.unwrap();
    let pool = db::open_read_pool(&path).await.unwrap();
    let (s, h, body) = get(&pool, "/2026/44").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert!(body.contains("Meal plans are temporarily unavailable."));

    // Closed pool.
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    pool.close().await;
    let (s, _, _) = get(&pool, "/2026/44").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn script_safety() {
    let t = TestDb::with_fixture().await;
    let mut c = t.writer().await;
    sqlx::query(
        "UPDATE meals SET name = '</script><script>alert(1)</script> <b>x</b> & \u{2028}' WHERE name = 'Walnuts'",
    )
    .execute(&mut c)
    .await
    .unwrap();
    let pool = t.reader().await;
    let (s, _, body) = get(&pool, "/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    assert!(!body.contains("<script>alert(1)"));
    assert!(!body.contains("alert(1)</script>"));
    assert!(!body.contains("<b>x</b>"));
    assert!(!body.contains('\u{2028}'));
    assert!(body.contains("\\u003c/script\\u003e\\u003cscript\\u003ealert(1)"));
}

#[tokio::test]
async fn page_references_only_same_origin_assets() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (_, _, body) = get(&pool, "/2026/44").await;
    assert!(!body.contains("http://") && !body.contains("https://"));

    let mut urls = Vec::new();
    for attr in ["src=\"", "href=\""] {
        for part in body.split(attr).skip(1) {
            urls.push(part.split('"').next().unwrap().to_string());
        }
    }
    assert!(!urls.is_empty());
    for url in urls {
        assert!(url.starts_with("/assets/"), "unexpected url {url}");
        let (s, h, _) = get(&pool, &url).await;
        assert_eq!(s, StatusCode::OK, "{url}");
        assert_eq!(
            h[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
    }
}
