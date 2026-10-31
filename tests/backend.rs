use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sous::calendar::Clock;
use sous::{db, queries};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, SqliteConnection, SqlitePool};
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;

const FIXTURE: &str = include_str!("../fixtures/2026-w44.sql");
const WEEKLY_FIXTURE: &str = include_str!("../fixtures/2026-w44-weekly.sql");
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

    /// Fully migrated, with the frozen fixture and the weekly-content fixture.
    async fn with_fixture() -> Self {
        let t = Self::with_base_fixture().await;
        let mut c = t.writer().await;
        sqlx::raw_sql(WEEKLY_FIXTURE).execute(&mut c).await.unwrap();
        t
    }

    /// Fully migrated, with only the frozen `2026-w44.sql` fixture.
    async fn with_base_fixture() -> Self {
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
    get_at(pool, test_clock(), uri).await
}

async fn get_at(
    pool: &SqlitePool,
    clock: Clock,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let res = sous::app(pool.clone(), clock)
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

/// Parse the `sous-page` JSON payload out of an HTML body.
fn page_json(body: &str) -> Value {
    let marker = "<script id=\"sous-page\" type=\"application/json\">";
    let start = body.find(marker).expect("sous-page script") + marker.len();
    let end = start + body[start..].find("</script>").expect("script end");
    serde_json::from_str(&body[start..end]).expect("valid page json")
}

fn title(body: &str) -> &str {
    let start = body.find("<title>").unwrap() + "<title>".len();
    &body[start..start + body[start..].find("</title>").unwrap()]
}

fn berlin(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> Clock {
    use chrono::TimeZone;
    Clock::fixed(
        sous::calendar::DEFAULT_TIMEZONE,
        chrono::Utc.with_ymd_and_hms(y, m, d, h, min, s).unwrap(),
    )
}

fn uuid(n: u32) -> String {
    format!("{n:08}-0000-4000-8000-000000000000")
}

/// Insert bare week rows (no days) through a separate writer connection.
async fn insert_weeks(t: &TestDb, weeks: &[(i32, i32)]) {
    let mut c = t.writer().await;
    for (i, (y, w)) in weeks.iter().enumerate() {
        exec(&mut c, &week_sql(&uuid(100 + i as u32), *y, *w))
            .await
            .unwrap();
    }
}

fn summary(y: i64, w: i64, start: &str, end: &str, path: &str) -> Value {
    json!({"isoYear": y, "isoWeek": w, "startDate": start, "endDate": end, "path": path})
}

/// A database file without tables, and a read pool on it.
async fn tableless_pool() -> (TempDir, SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.db");
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    SqliteConnection::connect_with(&opts).await.unwrap();
    let pool = db::open_read_pool(&path).await.unwrap();
    (dir, pool)
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
    assert!(
        serde_json::to_string(&doc)
            .unwrap()
            .contains("\"kcal\":380.5")
    );
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
    assert!(body.contains("sous-page"));

    for uri in [
        "/2026/45",
        "/2025/53",
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
}

#[tokio::test]
async fn http_503_when_database_unavailable() {
    let (_dir, tableless) = tableless_pool().await;
    let t = TestDb::with_fixture().await;
    let closed = t.reader().await;
    closed.close().await;

    for pool in [&tableless, &closed] {
        for uri in ["/", "/2026/44", "/2026/45", "/weeks"] {
            let (s, h, body) = get(pool, uri).await;
            assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
            assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
            assert_eq!(page_json(&body), json!({"kind": "unavailable"}), "{uri}");
            assert_eq!(title(&body), "Unavailable — Meal Plan");
        }
    }
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

#[tokio::test]
async fn week_page_payload_and_dates() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (s, _, body) = get(&pool, "/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(title(&body), "Meal Plan — 2026 week 44");
    let page = page_json(&body);
    assert_eq!(page["kind"], "week");
    assert_eq!(page["document"]["week"]["startDate"], "2026-10-26");
    assert_eq!(page["document"]["week"]["endDate"], "2026-11-01");
    assert_eq!(page["document"]["days"].as_array().unwrap().len(), 7);
}

#[tokio::test]
async fn iso_week_53_and_year_boundary() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2026, 53)]).await;
    let pool = t.reader().await;

    let (s, _, body) = get(&pool, "/2026/53").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["document"]["week"]["startDate"], "2026-12-28");
    assert_eq!(page["document"]["week"]["endDate"], "2027-01-03");

    // Saturday 2027-01-02 belongs to 2026-W53.
    let (s, _, body) = get_at(&pool, berlin(2027, 1, 2, 12, 0, 0), "/").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["document"]["week"]["isoYear"], 2026);
    assert_eq!(page["document"]["week"]["isoWeek"], 53);
    assert_eq!(page["selectedWeekday"], 6);

    // Monday 2027-01-04 is 2027-W1, which is not stored.
    let (s, _, body) = get_at(&pool, berlin(2027, 1, 4, 12, 0, 0), "/").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["kind"], "missing");
    assert_eq!(page["week"]["path"], "/2027/1");

    // 2025 and 2027 have no week 53.
    for uri in ["/2025/53", "/2027/53"] {
        let (s, _, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(page_json(&body)["kind"], "invalid");
    }
}

#[tokio::test]
async fn root_selects_today_in_service_timezone() {
    use chrono::TimeZone;
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2026, 45)]).await;
    let pool = t.reader().await;

    // Sunday 23:59:59 Berlin (CET) is still W44, weekday 7; the URL is not redirected.
    let (s, h, body) = get_at(&pool, berlin(2026, 11, 1, 22, 59, 59), "/").await;
    assert_eq!(s, StatusCode::OK);
    assert!(h.get(header::LOCATION).is_none());
    let page = page_json(&body);
    assert_eq!(page["kind"], "week");
    assert_eq!(page["document"]["week"]["isoWeek"], 44);
    assert_eq!(page["selectedWeekday"], 7);

    // One second later it is Monday in W45.
    let (s, _, body) = get_at(&pool, berlin(2026, 11, 1, 23, 0, 0), "/").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["kind"], "week");
    assert_eq!(page["document"]["week"]["isoWeek"], 45);
    assert_eq!(page["selectedWeekday"], 1);

    // The same instant in New York is still Sunday of W44.
    let new_york = Clock::fixed(
        chrono_tz::America::New_York,
        chrono::Utc.with_ymd_and_hms(2026, 11, 1, 23, 0, 0).unwrap(),
    );
    let (_, _, body) = get_at(&pool, new_york, "/").await;
    let page = page_json(&body);
    assert_eq!(page["document"]["week"]["isoWeek"], 44);
    assert_eq!(page["selectedWeekday"], 7);
}

#[tokio::test]
async fn root_missing_current_week_is_200_with_summary() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (s, h, body) = get(&pool, "/").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(title(&body), "Meal Plan — 2026 week 49");
    assert_eq!(
        page_json(&body),
        json!({
            "kind": "missing",
            "week": summary(2026, 49, "2026-11-30", "2026-12-06", "/2026/49"),
        })
    );
}

#[tokio::test]
async fn explicit_week_selects_today_only_in_current_week() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2027, 2), (2026, 49)]).await;
    let pool = t.reader().await;

    // Clock is Wednesday of 2026-W49.
    let (s, _, body) = get(&pool, "/2026/49").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page_json(&body)["selectedWeekday"], 3);
    // Past week and future week: Monday, no redirect.
    for uri in ["/2026/44", "/2027/2"] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        assert!(h.get(header::LOCATION).is_none());
        assert_eq!(page_json(&body)["selectedWeekday"], 1, "{uri}");
    }
    // With the clock inside W44 (Friday), /2026/44 selects today.
    let (_, _, body) = get_at(&pool, berlin(2026, 10, 30, 9, 0, 0), "/2026/44").await;
    assert_eq!(page_json(&body)["selectedWeekday"], 5);
}

#[tokio::test]
async fn explicit_missing_week_is_404_without_weeks() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2026, 53)]).await;
    let pool = t.reader().await;
    let (s, h, body) = get(&pool, "/2026/45").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(title(&body), "Meal Plan — 2026 week 45");
    assert_eq!(
        page_json(&body),
        json!({
            "kind": "missing",
            "week": summary(2026, 45, "2026-11-02", "2026-11-08", "/2026/45"),
        })
    );

    // Empty database: same payload shape, no weeks key.
    let empty = TestDb::migrated().await;
    let pool = empty.reader().await;
    let (s, _, body) = get(&pool, "/2026/45").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(page_json(&body).get("weeks").is_none());
    let (s, _, body) = get(&pool, "/").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page_json(&body)["kind"], "missing");
    assert!(page_json(&body).get("weeks").is_none());
}

#[tokio::test]
async fn invalid_paths_are_404_invalid_and_never_500() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for uri in [
        "/2025/53",
        "/2025/053",
        "/9999/52",
        "/2026/54",
        "/2026/0",
        "/2026/00",
        "/0000/1",
        "/10000/1",
        "/026/1",
        "/2026/+4",
        "/2026/%204",
        "/2026/%EF%BC%94",
        "/2026/abc",
        "/2026/99999999999999999999",
        "/2026/00000000000000000000000000",
    ] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8", "{uri}");
        assert_eq!(page_json(&body), json!({"kind": "invalid"}), "{uri}");
        assert_eq!(title(&body), "Invalid week — Meal Plan", "{uri}");
    }
}

#[tokio::test]
async fn zero_padded_weeks_redirect_permanently() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for (from, to) in [
        ("/2026/04", "/2026/4"),
        ("/2026/004", "/2026/4"),
        ("/2026/053", "/2026/53"),
        ("/9999/051", "/9999/51"),
        ("/0001/01", "/0001/1"),
    ] {
        let (s, h, _) = get(&pool, from).await;
        assert_eq!(s, StatusCode::PERMANENT_REDIRECT, "{from}");
        assert_eq!(h[header::LOCATION], to, "{from}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{from}");
    }
    // The redirect does not touch the database.
    let (_dir, tableless) = tableless_pool().await;
    let (s, _, _) = get(&tableless, "/2026/04").await;
    assert_eq!(s, StatusCode::PERMANENT_REDIRECT);
}

#[tokio::test]
async fn clock_is_read_once_per_request() {
    use chrono::TimeZone;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    // First read: Sunday 23:59:59 Berlin (W44). Later reads: Monday 00:00:00 (W45).
    let clock = Clock::new(sous::calendar::DEFAULT_TIMEZONE, move || {
        let (h, m, s) = if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            (22, 59, 59)
        } else {
            (23, 0, 0)
        };
        chrono::Utc.with_ymd_and_hms(2026, 11, 1, h, m, s).unwrap()
    });
    let (s, _, body) = get_at(&pool, clock, "/").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["kind"], "week");
    assert_eq!(page["document"]["week"]["isoWeek"], 44);
    assert_eq!(page["selectedWeekday"], 7);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn root_does_not_panic_when_current_week_is_unrepresentable() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (s, _, body) = get_at(&pool, berlin(9999, 12, 31, 12, 0, 0), "/").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(page_json(&body), json!({"kind": "invalid"}));
}

#[tokio::test]
async fn weeks_archive_is_ordered_and_fresh() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2027, 1), (2025, 52), (2026, 53), (2026, 1)]).await;
    let pool = t.reader().await;
    let expected = json!({"kind": "archive", "weeks": [
        summary(2027, 1, "2027-01-04", "2027-01-10", "/2027/1"),
        summary(2026, 53, "2026-12-28", "2027-01-03", "/2026/53"),
        summary(2026, 44, "2026-10-26", "2026-11-01", "/2026/44"),
        summary(2026, 1, "2025-12-29", "2026-01-04", "/2026/1"),
        summary(2025, 52, "2025-12-22", "2025-12-28", "/2025/52"),
    ]});
    let (s, h, body) = get(&pool, "/weeks").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(title(&body), "All weeks — Meal Plan");
    assert_eq!(page_json(&body), expected);

    // An open transaction's insert is invisible; once it ends the week shows up.
    let mut c = t.writer().await;
    let mut tx = c.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(week_sql(&uuid(7), 2027, 5)))
        .execute(&mut *tx)
        .await
        .unwrap();
    let (_, _, body) = get(&pool, "/weeks").await;
    assert_eq!(page_json(&body), expected);
    tx.commit().await.unwrap();
    let (_, _, body) = get(&pool, "/weeks").await;
    let page = page_json(&body);
    assert_eq!(page["weeks"].as_array().unwrap().len(), 6);
    assert_eq!(page["weeks"][0]["path"], "/2027/5");

    // Deleted weeks disappear.
    exec(
        &mut c,
        "DELETE FROM weeks WHERE iso_year = 2027 AND iso_week = 5",
    )
    .await
    .unwrap();
    exec(
        &mut c,
        "DELETE FROM weeks WHERE iso_year = 2026 AND iso_week = 53",
    )
    .await
    .unwrap();
    let (_, _, body) = get(&pool, "/weeks").await;
    let paths: Vec<_> = page_json(&body)["weeks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(paths, ["/2027/1", "/2026/44", "/2026/1", "/2025/52"]);
}

#[tokio::test]
async fn weeks_archive_empty_database() {
    let t = TestDb::migrated().await;
    let pool = t.reader().await;
    let (s, _, body) = get(&pool, "/weeks").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page_json(&body), json!({"kind": "archive", "weeks": []}));
}

#[tokio::test]
async fn non_get_methods_are_405() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for uri in ["/", "/weeks", "/2026/44"] {
        for method in ["POST", "PUT", "DELETE"] {
            let res = sous::app(pool.clone(), test_clock())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {uri}"
            );
        }
    }
}

#[tokio::test]
async fn unknown_paths_keep_plain_404() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for uri in ["/nothing", "/weeks/", "/2026", "/2026/44/extra"] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        assert!(!body.contains("sous-page"), "{uri}");
    }
}

/// Feature 2 stub: fixed clock in ISO 2026-W49 (Wed), outside the fixture week.
fn test_clock() -> sous::calendar::Clock {
    use chrono::TimeZone;
    sous::calendar::Clock::fixed(
        sous::calendar::DEFAULT_TIMEZONE,
        chrono::Utc.with_ymd_and_hms(2026, 12, 2, 12, 0, 0).unwrap(),
    )
}

// ---- Feature 3: shopping lists ----

const L1: &str = "00000005-0000-4000-8000-00000000001f";
const L2: &str = "00000005-0000-4000-8000-000000000020";
const L3: &str = "00000005-0000-4000-8000-000000000021";
const MON: &str = "00000002-0000-4000-8000-000000000002";
const TUE: &str = "00000002-0000-4000-8000-000000000003";
const FIXTURE_WEEK: &str = "00000001-0000-4000-8000-000000000001";

fn sid(kind: u32, n: u32) -> String {
    format!("{kind:08}-0000-4000-8000-{n:012x}")
}

async fn count(c: &mut SqliteConnection, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(c)
        .await
        .unwrap()
}

fn ids(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

fn list_ids(doc: &sous::model::WeekDocument) -> Vec<String> {
    doc.shopping_lists.iter().map(|l| l.id.clone()).collect()
}

#[tokio::test]
async fn shopping_day_order_follows_link_position() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(ids(&doc.days[0].shopping_list_ids), [L2, L1]);

    let mut c = t.writer().await;
    // Swap the two link positions (through a temporary position to respect UNIQUE).
    for (list, pos) in [(L2, 99), (L1, 0), (L2, 10)] {
        let sql = format!(
            "UPDATE day_shopping_lists SET position = {pos} WHERE day_id = '{MON}' AND shopping_list_id = '{list}'"
        );
        exec(&mut c, &sql).await.unwrap();
    }
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(ids(&doc.days[0].shopping_list_ids), [L1, L2]);
}

#[tokio::test]
async fn shopping_list_shared_by_days_is_stored_once() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(ids(&doc.days[0].shopping_list_ids), [L2, L1]);
    assert_eq!(ids(&doc.days[2].shopping_list_ids), [L1]);
    assert_eq!(doc.shopping_lists.iter().filter(|l| l.id == L1).count(), 1);

    let mut c = t.writer().await;
    let items = count(&mut c, "shopping_items").await;
    let lists = count(&mut c, "shopping_lists").await;
    let sql = format!(
        "INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) VALUES ('{TUE}', '{L1}', 0)"
    );
    exec(&mut c, &sql).await.unwrap();
    assert_eq!(count(&mut c, "shopping_items").await, items);
    assert_eq!(count(&mut c, "shopping_lists").await, lists);
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(ids(&doc.days[1].shopping_list_ids), [L1]);
    assert_eq!(doc.shopping_lists.iter().filter(|l| l.id == L1).count(), 1);
}

#[tokio::test]
async fn shopping_content_order_and_freshness() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();

    assert_eq!(ids(&list_ids(&doc)), [L3, L1, L2]);
    let (l3, l1, l2) = (
        &doc.shopping_lists[0],
        &doc.shopping_lists[1],
        &doc.shopping_lists[2],
    );
    assert_eq!(l1.label, "🛒 Monday Shop → covers Mon–Wed");
    assert_eq!(
        l1.note.as_deref(),
        Some("You have: salmon, oats, walnuts, almonds, chia, miso, spices.")
    );
    assert_eq!(l2.label, "🧴 Household");
    assert_eq!(l2.note, None);
    assert_eq!(l3.label, "🛒 Thursday Top-Up → covers Thu–Sun");
    assert_eq!(l3.note.as_deref(), Some("Small trip."));

    let shape = |l: &sous::model::ShoppingList| -> Vec<(String, Vec<String>)> {
        l.categories
            .iter()
            .map(|c| {
                (
                    c.name.clone(),
                    c.items.iter().map(|i| i.text.clone()).collect(),
                )
            })
            .collect()
    };
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        shape(l1),
        [
            (
                "Produce".to_string(),
                s(&[
                    "1 cucumber",
                    "1 courgette",
                    "1 bag baby spinach (200g)",
                    "200g mixed berries",
                    "Fresh ginger"
                ])
            ),
            (
                "Dairy / Chilled".to_string(),
                s(&["2L oat milk", "Skyr (400g)"])
            ),
            ("Bakery".to_string(), s(&["1 loaf dark rye bread"])),
        ]
    );
    assert_eq!(
        shape(l2),
        [
            ("Cleaning".to_string(), s(&["Dish soap"])),
            ("Spices".to_string(), s(&[])),
        ]
    );
    assert_eq!(
        shape(l3),
        [(
            "Meat".to_string(),
            s(&["4 skinless chicken thighs (~400g)"])
        )]
    );

    // Unassigned L3 is in no day; days without links have empty ids.
    for d in &doc.days {
        assert!(!d.shopping_list_ids.iter().any(|i| i == L3));
    }
    for i in [1, 3, 4, 5, 6] {
        assert!(doc.days[i].shopping_list_ids.is_empty(), "day {i}");
    }

    // Payload shape.
    let (_, _, body) = get(&pool, "/2026/44").await;
    let page = page_json(&body);
    let lists = page["document"]["shoppingLists"].as_array().unwrap();
    assert_eq!(lists.len(), 3);
    assert_eq!(lists[0]["id"], L3);
    assert_eq!(lists[2]["note"], Value::Null);
    assert_eq!(lists[1]["categories"][0]["name"], "Produce");
    assert_eq!(lists[1]["categories"][0]["items"][0]["text"], "1 cucumber");
    assert!(lists[1]["categories"][0]["items"][0]["id"].is_string());
    assert_eq!(
        page["document"]["days"][0]["shoppingListIds"],
        json!([L2, L1])
    );
    assert_eq!(page["document"]["days"][4]["shoppingListIds"], json!([]));

    // Edits through a separate writer are visible on the next load.
    let mut c = t.writer().await;
    exec(
        &mut c,
        "UPDATE shopping_items SET text = '2 cucumbers' WHERE text = '1 cucumber'",
    )
    .await
    .unwrap();
    exec(
        &mut c,
        &format!("UPDATE shopping_lists SET note = 'New note' WHERE id = '{L1}'"),
    )
    .await
    .unwrap();
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(doc.shopping_lists[1].note.as_deref(), Some("New note"));
    assert_eq!(
        doc.shopping_lists[1].categories[0].items[0].text,
        "2 cucumbers"
    );
    let (_, _, body) = get(&pool, "/2026/44").await;
    let page = page_json(&body);
    assert_eq!(page["document"]["shoppingLists"][1]["note"], "New note");
    assert_eq!(
        page["document"]["shoppingLists"][1]["categories"][0]["items"][0]["text"],
        "2 cucumbers"
    );
}

#[tokio::test]
async fn shopping_constraints_reject_bad_data() {
    let t = TestDb::migrated().await;
    let mut c = t.writer().await;
    let (wa, wb) = (sid(1, 1), sid(1, 2));
    let (da, da2, db_) = (sid(2, 1), sid(2, 2), sid(2, 3));
    let (la1, la2, la3, lb) = (sid(5, 1), sid(5, 2), sid(5, 3), sid(5, 4));
    let (ca, ia) = (sid(6, 1), sid(7, 1));
    let missing = "99999999-9999-4999-8999-999999999999";
    for sql in [
        week_sql(&wa, 2026, 44),
        week_sql(&wb, 2026, 45),
        day_sql(&da, &wa, 1),
        day_sql(&da2, &wa, 2),
        day_sql(&db_, &wb, 1),
    ] {
        exec(&mut c, &sql).await.unwrap();
    }
    let list = |id: &str, week: &str, pos: i32, label: &str| {
        format!(
            "INSERT INTO shopping_lists (id, week_id, position, label) VALUES ('{id}', '{week}', {pos}, {label})"
        )
    };
    let link = |day: &str, l: &str, pos: i32| {
        format!(
            "INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) VALUES ('{day}', '{l}', {pos})"
        )
    };
    let cat = |id: &str, l: &str, pos: i32, name: &str| {
        format!(
            "INSERT INTO shopping_categories (id, shopping_list_id, position, name) VALUES ('{id}', '{l}', {pos}, {name})"
        )
    };
    let item = |id: &str, cat: &str, pos: i32, text: &str| {
        format!(
            "INSERT INTO shopping_items (id, category_id, position, text) VALUES ('{id}', '{cat}', {pos}, {text})"
        )
    };

    exec(&mut c, &list(&la1, &wa, 0, "'a'")).await.unwrap();
    exec(&mut c, &list(&la2, &wa, 5, "'b'")).await.unwrap();
    exec(&mut c, &list(&la3, &wa, 7, "'c'")).await.unwrap();
    exec(&mut c, &list(&lb, &wb, 0, "'d'")).await.unwrap();
    exec(&mut c, &link(&da, &la1, 0)).await.unwrap();
    exec(&mut c, &link(&da2, &la2, 0)).await.unwrap();
    exec(&mut c, &cat(&ca, &la1, 0, "'cat'")).await.unwrap();
    exec(&mut c, &item(&ia, &ca, 0, "'it'")).await.unwrap();

    let n = sid(5, 9);
    let (cn, inew) = (sid(6, 9), sid(7, 9));
    let bad: Vec<(&str, String)> = vec![
        ("list fk week", list(&n, missing, 1, "'x'")),
        ("link fk day", link(missing, &la1, 3)),
        ("link fk list", link(&da, missing, 3)),
        ("category fk list", cat(&cn, missing, 3, "'x'")),
        ("item fk category", item(&inew, missing, 3, "'x'")),
        ("list dup position", list(&n, &wa, 0, "'x'")),
        ("category dup position", cat(&cn, &la1, 0, "'x'")),
        ("item dup position", item(&inew, &ca, 0, "'x'")),
        ("link dup position", link(&da, &la3, 0)),
        ("link dup pair", link(&da, &la1, 4)),
        ("list negative position", list(&n, &wa, -1, "'x'")),
        ("link negative position", link(&da, &la3, -1)),
        ("category negative position", cat(&cn, &la1, -1, "'x'")),
        ("item negative position", item(&inew, &ca, -1, "'x'")),
        ("list bad uuid", list("nope", &wa, 1, "'x'")),
        ("category bad uuid", cat("nope", &la1, 1, "'x'")),
        ("item bad uuid", item("nope", &ca, 1, "'x'")),
        ("list null label", list(&n, &wa, 1, "NULL")),
        ("category null name", cat(&cn, &la1, 1, "NULL")),
        ("item null text", item(&inew, &ca, 1, "NULL")),
    ];
    for (what, sql) in bad {
        assert!(exec(&mut c, &sql).await.is_err(), "{what} was accepted");
    }

    // Cross-week links and moves are rejected by the triggers.
    let cross: Vec<(&str, String)> = vec![
        ("cross-week insert", link(&db_, &la3, 5)),
        ("cross-week insert (other way)", link(&da2, &lb, 5)),
        (
            "update day_id",
            format!("UPDATE day_shopping_lists SET day_id = '{db_}' WHERE day_id = '{da}'"),
        ),
        (
            "update shopping_list_id",
            format!(
                "UPDATE day_shopping_lists SET shopping_list_id = '{lb}' WHERE day_id = '{da}'"
            ),
        ),
        (
            "move linked day",
            format!("UPDATE days SET week_id = '{wb}' WHERE id = '{da2}'"),
        ),
        (
            "move linked list",
            format!("UPDATE shopping_lists SET week_id = '{wb}' WHERE id = '{la2}'"),
        ),
    ];
    for (what, sql) in cross {
        let err = exec(&mut c, &sql).await.expect_err(what).to_string();
        assert!(
            err.contains("different weeks") || err.contains("another week"),
            "{what}: {err}"
        );
    }

    // Rejected statements changed nothing.
    assert_eq!(count(&mut c, "day_shopping_lists").await, 2);
    // Moving an unlinked list is allowed.
    exec(
        &mut c,
        &format!("UPDATE shopping_lists SET week_id = '{wb}' WHERE id = '{la3}'"),
    )
    .await
    .unwrap();
    // So is moving an unlinked day.
    exec(
        &mut c,
        &format!("DELETE FROM day_shopping_lists WHERE day_id = '{da2}'"),
    )
    .await
    .unwrap();
    exec(
        &mut c,
        &format!("UPDATE days SET week_id = '{wb}' WHERE id = '{da2}'"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn shopping_deletes_cascade() {
    // Deleting a list.
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let mut c = t.writer().await;
    exec(
        &mut c,
        &format!("DELETE FROM shopping_lists WHERE id = '{L1}'"),
    )
    .await
    .unwrap();
    assert_eq!(count(&mut c, "shopping_lists").await, 2);
    assert_eq!(count(&mut c, "shopping_categories").await, 3);
    assert_eq!(count(&mut c, "shopping_items").await, 2);
    assert_eq!(count(&mut c, "day_shopping_lists").await, 1);
    assert_eq!(count(&mut c, "days").await, 4);
    let doc = queries::load_week(&pool, 2026, 44).await.unwrap().unwrap();
    assert_eq!(ids(&doc.days[0].shopping_list_ids), [L2]);
    assert!(doc.days[2].shopping_list_ids.is_empty());
    assert_eq!(ids(&list_ids(&doc)), [L3, L2]);
    assert_eq!(doc.shopping_lists[1].categories.len(), 2);
    assert_eq!(doc.shopping_lists[0].categories[0].items.len(), 1);

    // Deleting a day removes its links but not the lists.
    let t = TestDb::with_fixture().await;
    let mut c = t.writer().await;
    exec(&mut c, &format!("DELETE FROM days WHERE id = '{MON}'"))
        .await
        .unwrap();
    assert_eq!(count(&mut c, "day_shopping_lists").await, 1);
    assert_eq!(count(&mut c, "shopping_lists").await, 3);
    assert_eq!(count(&mut c, "shopping_items").await, 10);

    // Deleting the week removes everything.
    let t = TestDb::with_fixture().await;
    let mut c = t.writer().await;
    exec(
        &mut c,
        &format!("DELETE FROM weeks WHERE id = '{FIXTURE_WEEK}'"),
    )
    .await
    .unwrap();
    for table in [
        "shopping_lists",
        "day_shopping_lists",
        "shopping_categories",
        "shopping_items",
    ] {
        assert_eq!(count(&mut c, table).await, 0, "{table}");
    }
}

// ---- Feature 4: prep, strategy, reminders ----

const SESSION_MON: &str = "00000008-0000-4000-8000-000000000035";
const SESSION_WED: &str = "00000008-0000-4000-8000-000000000032";
const CARD_GREEN: &str = "0000000a-0000-4000-8000-00000000003b";
const WED: &str = "00000002-0000-4000-8000-000000000004";

type Group = Vec<(String, Vec<String>)>;

/// Prep and strategy content as `(title, [child text])` pairs in document order.
fn weekly_order(doc: &Value) -> (Group, Group) {
    let pick = |key: &str, children: &str, text: &str| -> Group {
        doc[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["title"].as_str().unwrap().to_string(),
                    p[children]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|c| c[text].as_str().unwrap().to_string())
                        .collect(),
                )
            })
            .collect()
    };
    (
        pick("prepSessions", "tasks", "name"),
        pick("strategyCards", "items", "text"),
    )
}

fn strs(v: &Group) -> Vec<(&str, Vec<&str>)> {
    v.iter()
        .map(|(t, c)| (t.as_str(), c.iter().map(String::as_str).collect()))
        .collect()
}

async fn week_value(pool: &SqlitePool, y: i32, w: u32) -> Value {
    serde_json::to_value(queries::load_week(pool, y, w).await.unwrap().unwrap()).unwrap()
}

fn sorted_ids(doc: &Value, key: &str) -> Vec<String> {
    let mut v: Vec<String> = doc[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap().to_string())
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn upgrade_from_version_2_keeps_data() {
    // A database migrated only through version 2, holding the frozen fixture.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let mut old = sqlx::migrate!();
    old.migrations = old
        .migrations
        .iter()
        .filter(|m| m.version <= 2)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    assert_eq!(old.migrations.len(), 2);
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    {
        let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
        old.run(&mut c).await.unwrap();
        sqlx::raw_sql(FIXTURE).execute(&mut c).await.unwrap();
    }
    db::migrate(&path).await.unwrap();
    let pool = db::open_read_pool(&path).await.unwrap();
    let upgraded = week_value(&pool, 2026, 44).await;

    let fresh = TestDb::with_base_fixture().await;
    assert_eq!(upgraded, week_value(&fresh.reader().await, 2026, 44).await);

    assert_eq!(upgraded["week"]["prepIntro"], Value::Null);
    assert_eq!(upgraded["prepSessions"], json!([]));
    assert_eq!(upgraded["strategyCards"], json!([]));
    for d in upgraded["days"].as_array().unwrap() {
        assert_eq!(d["reminder"], Value::Null);
    }
    assert_eq!(upgraded["days"][0]["meals"].as_array().unwrap().len(), 4);
    assert_eq!(upgraded["shoppingLists"].as_array().unwrap().len(), 3);
    assert_eq!(upgraded["days"][0]["shoppingListIds"], json!([L2, L1]));

    let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
    sqlx::raw_sql(WEEKLY_FIXTURE).execute(&mut c).await.unwrap();
    assert_eq!(count(&mut c, "prep_tasks").await, 5);
    let doc = week_value(&pool, 2026, 44).await;
    assert_eq!(doc["prepSessions"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn weekly_content_order_and_reorder() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = week_value(&pool, 2026, 44).await;
    let (prep, strategy) = weekly_order(&doc);
    assert_eq!(
        strs(&prep),
        [
            (
                "Monday Prep Session (~1.5 hours)",
                vec![
                    "Start the Gravlax",
                    "Air-Fryer Roasted Chickpeas",
                    "Cook Brown Rice"
                ]
            ),
            (
                "Wednesday Evening Prep (~45 min)",
                vec![
                    "Overnight Oats × 1 jar",
                    "Lentil & Roasted Veg Salad (3 portions)"
                ]
            ),
        ]
    );
    assert_eq!(
        strs(&strategy),
        [
            (
                "Weekly Variety",
                vec![
                    "Oily fish 3×",
                    "Legumes daily",
                    "Nuts daily — walnuts & almonds"
                ]
            ),
            ("Air Fryer Features", vec!["Mon: crispy chickpeas"]),
            ("The Salmon Plan", vec!["~300g → gravlax"]),
            ("This Week's Notes", vec!["Monday + Thursday shop split"]),
        ]
    );
    let themes: Vec<_> = doc["strategyCards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["theme"].as_str().unwrap())
        .collect();
    assert_eq!(themes, ["green", "blue", "purple", "yellow"]);
    assert_eq!(doc["strategyCards"][2]["icon"], Value::Null);
    assert_eq!(doc["strategyCards"][0]["icon"], "🐟");
    assert_eq!(doc["prepSessions"][1]["covers"], Value::Null);
    assert_eq!(
        doc["prepSessions"][0]["covers"],
        "Covers Monday through Wednesday"
    );
    assert_eq!(doc["prepSessions"][0]["id"], SESSION_MON);
    let session_ids = sorted_ids(&doc, "prepSessions");
    let card_ids = sorted_ids(&doc, "strategyCards");

    // Reorder through a separate writer: shift every position out of the way, then set.
    let mut c = t.writer().await;
    let mut tx = c.begin().await.unwrap();
    for table in [
        "prep_sessions",
        "prep_tasks",
        "strategy_cards",
        "strategy_items",
    ] {
        exec(
            &mut tx,
            &format!("UPDATE {table} SET position = position + 100"),
        )
        .await
        .unwrap();
    }
    for (table, id, pos) in [
        ("prep_sessions", SESSION_WED, 0),
        ("prep_sessions", SESSION_MON, 1),
        ("prep_tasks", "00000009-0000-4000-8000-000000000036", 0),
        ("prep_tasks", "00000009-0000-4000-8000-000000000038", 1),
        ("prep_tasks", "00000009-0000-4000-8000-000000000037", 2),
        ("prep_tasks", "00000009-0000-4000-8000-000000000033", 0),
        ("prep_tasks", "00000009-0000-4000-8000-000000000034", 1),
        ("strategy_cards", "0000000a-0000-4000-8000-000000000039", 0),
        ("strategy_cards", "0000000a-0000-4000-8000-00000000003f", 1),
        ("strategy_cards", "0000000a-0000-4000-8000-000000000041", 2),
        ("strategy_cards", CARD_GREEN, 3),
        ("strategy_items", "0000000b-0000-4000-8000-00000000003e", 0),
        ("strategy_items", "0000000b-0000-4000-8000-00000000003c", 1),
        ("strategy_items", "0000000b-0000-4000-8000-00000000003d", 2),
    ] {
        exec(
            &mut tx,
            &format!("UPDATE {table} SET position = {pos} WHERE id = '{id}'"),
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    let (s, _, body) = get(&pool, "/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    let doc = page_json(&body)["document"].clone();
    let (prep, strategy) = weekly_order(&doc);
    assert_eq!(
        strs(&prep),
        [
            (
                "Wednesday Evening Prep (~45 min)",
                vec![
                    "Lentil & Roasted Veg Salad (3 portions)",
                    "Overnight Oats × 1 jar"
                ]
            ),
            (
                "Monday Prep Session (~1.5 hours)",
                vec![
                    "Cook Brown Rice",
                    "Air-Fryer Roasted Chickpeas",
                    "Start the Gravlax"
                ]
            ),
        ]
    );
    let card_titles: Vec<_> = strategy.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        card_titles,
        [
            "This Week's Notes",
            "The Salmon Plan",
            "Air Fryer Features",
            "Weekly Variety"
        ]
    );
    assert_eq!(
        strategy[3].1,
        [
            "Nuts daily — walnuts & almonds",
            "Legumes daily",
            "Oily fish 3×"
        ]
    );
    assert_eq!(sorted_ids(&doc, "prepSessions"), session_ids);
    assert_eq!(sorted_ids(&doc, "strategyCards"), card_ids);
    assert_eq!(doc["strategyCards"][3]["id"], CARD_GREEN);
}

#[tokio::test]
async fn weekly_constraints_reject_bad_data() {
    let t = TestDb::migrated().await;
    let mut c = t.writer().await;
    let (wa, wb) = (sid(1, 1), sid(1, 2));
    let (s1, s2, s3) = (sid(8, 1), sid(8, 2), sid(8, 3));
    let (k1, k2) = (sid(10, 1), sid(10, 2));
    let (t1, i1) = (sid(9, 1), sid(11, 1));
    let missing = "99999999-9999-4999-8999-999999999999";
    exec(&mut c, &week_sql(&wa, 2026, 44)).await.unwrap();
    exec(&mut c, &week_sql(&wb, 2026, 45)).await.unwrap();

    let session = |id: &str, week: &str, pos: i32, title: &str| {
        format!(
            "INSERT INTO prep_sessions (id, week_id, position, title) VALUES ('{id}', '{week}', {pos}, {title})"
        )
    };
    let task = |id: &str, s: &str, pos: i32, name: &str, time: &str, ins: &str| {
        format!(
            "INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) VALUES ('{id}', '{s}', {pos}, {name}, {time}, {ins})"
        )
    };
    let card = |id: &str, week: &str, pos: i32, title: &str, theme: &str| {
        format!(
            "INSERT INTO strategy_cards (id, week_id, position, title, theme) VALUES ('{id}', '{week}', {pos}, {title}, {theme})"
        )
    };
    let item = |id: &str, card: &str, pos: i32, text: &str| {
        format!(
            "INSERT INTO strategy_items (id, card_id, position, text) VALUES ('{id}', '{card}', {pos}, {text})"
        )
    };

    exec(&mut c, &session(&s1, &wa, 0, "'s'")).await.unwrap();
    exec(&mut c, &task(&t1, &s1, 0, "'n'", "'t'", "'i'"))
        .await
        .unwrap();
    exec(&mut c, &card(&k1, &wa, 0, "'c'", "'green'"))
        .await
        .unwrap();
    exec(&mut c, &item(&i1, &k1, 0, "'x'")).await.unwrap();

    let (n, nt, nk, ni) = (sid(8, 9), sid(9, 9), sid(10, 9), sid(11, 9));
    let upper = "AAAAAAAA-0000-4000-8000-000000000000";
    let mut bad: Vec<(String, String)> = Vec::new();
    for theme in ["'red'", "'Green'", "''", "'#00ff00'", "NULL"] {
        bad.push((format!("theme {theme}"), card(&nk, &wa, 5, "'c'", theme)));
    }
    bad.extend([
        ("session dup position".into(), session(&n, &wa, 0, "'x'")),
        (
            "card dup position".into(),
            card(&nk, &wa, 0, "'x'", "'blue'"),
        ),
        (
            "task dup position".into(),
            task(&nt, &s1, 0, "'n'", "'t'", "'i'"),
        ),
        ("item dup position".into(), item(&ni, &k1, 0, "'x'")),
        ("session negative".into(), session(&n, &wa, -1, "'x'")),
        (
            "task negative".into(),
            task(&nt, &s1, -1, "'n'", "'t'", "'i'"),
        ),
        ("card negative".into(), card(&nk, &wa, -1, "'x'", "'blue'")),
        ("item negative".into(), item(&ni, &k1, -1, "'x'")),
        ("session null title".into(), session(&n, &wa, 5, "NULL")),
        (
            "task null name".into(),
            task(&nt, &s1, 5, "NULL", "'t'", "'i'"),
        ),
        (
            "task null time".into(),
            task(&nt, &s1, 5, "'n'", "NULL", "'i'"),
        ),
        (
            "task null instructions".into(),
            task(&nt, &s1, 5, "'n'", "'t'", "NULL"),
        ),
        (
            "card null title".into(),
            card(&nk, &wa, 5, "NULL", "'blue'"),
        ),
        ("item null text".into(), item(&ni, &k1, 5, "NULL")),
        (
            "session bad id".into(),
            session("not-a-uuid", &wa, 5, "'x'"),
        ),
        ("session upper id".into(), session(upper, &wa, 5, "'x'")),
        (
            "task bad id".into(),
            task("nope", &s1, 5, "'n'", "'t'", "'i'"),
        ),
        ("card bad id".into(), card("nope", &wa, 5, "'x'", "'blue'")),
        ("item bad id".into(), item("nope", &k1, 5, "'x'")),
        ("session fk".into(), session(&n, missing, 5, "'x'")),
        ("task fk".into(), task(&nt, missing, 5, "'n'", "'t'", "'i'")),
        ("card fk".into(), card(&nk, missing, 5, "'x'", "'blue'")),
        ("item fk".into(), item(&ni, missing, 5, "'x'")),
    ]);
    for (what, sql) in &bad {
        assert!(
            exec(&mut c, sql).await.is_err(),
            "{what} should be rejected"
        );
    }
    // Rejected rows left nothing behind.
    assert_eq!(count(&mut c, "prep_sessions").await, 1);
    assert_eq!(count(&mut c, "prep_tasks").await, 1);
    assert_eq!(count(&mut c, "strategy_cards").await, 1);
    assert_eq!(count(&mut c, "strategy_items").await, 1);

    for (i, theme) in ["green", "blue", "purple", "yellow"].iter().enumerate() {
        let sql = card(
            &sid(10, 20 + i as u32),
            &wb,
            i as i32,
            "'c'",
            &format!("'{theme}'"),
        );
        exec(&mut c, &sql).await.unwrap();
    }
    // The same position under a different parent is fine.
    exec(&mut c, &session(&s2, &wb, 0, "'s'")).await.unwrap();
    exec(&mut c, &task(&sid(9, 2), &s2, 0, "'n'", "'t'", "'i'"))
        .await
        .unwrap();
    exec(&mut c, &item(&sid(11, 2), &sid(10, 20), 0, "'x'"))
        .await
        .unwrap();

    // Cascades: session -> tasks, card -> items, week -> everything.
    exec(&mut c, &session(&s3, &wa, 1, "'s3'")).await.unwrap();
    exec(&mut c, &task(&sid(9, 3), &s3, 0, "'n'", "'t'", "'i'"))
        .await
        .unwrap();
    exec(
        &mut c,
        &format!("DELETE FROM prep_sessions WHERE id = '{s3}'"),
    )
    .await
    .unwrap();
    assert_eq!(count(&mut c, "prep_tasks").await, 2);
    exec(&mut c, &card(&k2, &wa, 1, "'k2'", "'blue'"))
        .await
        .unwrap();
    exec(&mut c, &item(&sid(11, 3), &k2, 0, "'x'"))
        .await
        .unwrap();
    exec(
        &mut c,
        &format!("DELETE FROM strategy_cards WHERE id = '{k2}'"),
    )
    .await
    .unwrap();
    assert_eq!(count(&mut c, "strategy_items").await, 2);
    exec(&mut c, &format!("DELETE FROM weeks WHERE id = '{wa}'"))
        .await
        .unwrap();
    assert_eq!(count(&mut c, "prep_sessions").await, 1);
    assert_eq!(count(&mut c, "prep_tasks").await, 1);
    assert_eq!(count(&mut c, "strategy_cards").await, 4);
    assert_eq!(count(&mut c, "strategy_items").await, 1);
    exec(&mut c, &format!("DELETE FROM weeks WHERE id = '{wb}'"))
        .await
        .unwrap();
    for table in [
        "prep_sessions",
        "prep_tasks",
        "strategy_cards",
        "strategy_items",
    ] {
        assert_eq!(count(&mut c, table).await, 0, "{table}");
    }
}

#[tokio::test]
async fn reminders_follow_title() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = week_value(&pool, 2026, 44).await;
    let rem: Vec<&Value> = doc["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| &d["reminder"])
        .collect();
    assert_eq!(
        rem[2],
        &json!({"title": "Shop day", "body": "Pick up the Thursday top-up list after work.", "icon": "🧾"})
    );
    assert_eq!(
        rem[6],
        &json!({"title": "Plan next week", "body": null, "icon": null})
    );
    // Mon has a body and icon but no title.
    for i in [0, 1, 3, 4, 5] {
        assert_eq!(rem[i], &Value::Null, "weekday {}", i + 1);
    }
    assert_eq!(
        doc["week"]["prepIntro"],
        "Monday covers Mon–Wed. Wednesday is a smaller session for Thu–Sun."
    );

    let mut c = t.writer().await;
    exec(
        &mut c,
        &format!("UPDATE days SET reminder_title = 'Tue note' WHERE id = '{TUE}'"),
    )
    .await
    .unwrap();
    let doc = week_value(&pool, 2026, 44).await;
    assert_eq!(
        doc["days"][1]["reminder"],
        json!({"title": "Tue note", "body": null, "icon": null})
    );
}

#[tokio::test]
async fn week_without_weekly_content_has_empty_collections() {
    let t = TestDb::migrated().await;
    insert_weeks(&t, &[(2026, 44)]).await;
    let doc = week_value(&t.reader().await, 2026, 44).await;
    assert_eq!(doc["week"]["prepIntro"], Value::Null);
    assert_eq!(doc["prepSessions"], json!([]));
    assert_eq!(doc["strategyCards"], json!([]));
    assert_eq!(doc["schemaVersion"], 1);
    for d in doc["days"].as_array().unwrap() {
        assert_eq!(d["reminder"], Value::Null);
    }
}

#[tokio::test]
async fn weekly_content_freshness() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let mut c = t.writer().await;
    let edits = [
        "UPDATE weeks SET prep_intro = 'Fresh intro'",
        "UPDATE prep_tasks SET name = 'Fresh task' WHERE name = 'Cook Brown Rice'",
        "UPDATE strategy_items SET text = 'Fresh item' WHERE text = 'Legumes daily'",
        "UPDATE days SET reminder_title = 'Fresh reminder' WHERE reminder_title = 'Shop day'",
    ];
    let fresh = ["Fresh intro", "Fresh task", "Fresh item", "Fresh reminder"];

    let mut tx = c.begin().await.unwrap();
    for sql in edits {
        exec(&mut tx, sql).await.unwrap();
    }
    let (_, _, body) = get(&pool, "/2026/44").await;
    for f in fresh {
        assert!(!body.contains(f), "{f} visible before commit");
    }
    tx.commit().await.unwrap();
    let (_, _, body) = get(&pool, "/2026/44").await;
    for f in fresh {
        assert!(body.contains(f), "{f} missing after commit");
    }
}

#[tokio::test]
async fn weekly_content_script_safety() {
    let t = TestDb::with_fixture().await;
    let mut c = t.writer().await;
    let evil = "</script><script>alert(7)</script>";
    for sql in [
        format!("UPDATE weeks SET prep_intro = '{evil}'"),
        format!(
            "UPDATE prep_sessions SET title = '{evil}', covers = '{evil}' WHERE id = '{SESSION_MON}'"
        ),
        format!(
            "UPDATE prep_tasks SET name = '{evil}', time = '{evil}', instructions = '{evil}' WHERE name = 'Cook Brown Rice'"
        ),
        format!(
            "UPDATE strategy_cards SET title = '{evil}', icon = '{evil}' WHERE id = '{CARD_GREEN}'"
        ),
        format!("UPDATE strategy_items SET text = '{evil}' WHERE text = 'Legumes daily'"),
        format!(
            "UPDATE days SET reminder_title = '{evil}', reminder_body = '{evil}', reminder_icon = '{evil}' WHERE id = '{WED}'"
        ),
    ] {
        exec(&mut c, &sql).await.unwrap();
    }
    let (s, _, body) = get(&t.reader().await, "/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    assert!(!body.contains("alert(7)</script>"));
    assert!(!body.contains("<script>alert(7)"));
    // Intro, session x2, task x3, card x2, item, reminder x3.
    assert_eq!(
        body.matches("\\u003c/script\\u003e\\u003cscript\\u003ealert(7)")
            .count(),
        12
    );
    let page = page_json(&body);
    assert_eq!(page["document"]["week"]["prepIntro"], evil);
    assert_eq!(page["document"]["days"][2]["reminder"]["icon"], evil);
}
