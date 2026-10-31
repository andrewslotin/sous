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
