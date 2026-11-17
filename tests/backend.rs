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
const PREP_DAYS_FIXTURE: &str = include_str!("../fixtures/2026-w44-prep-days.sql");
const STRATEGY_FIXTURE: &str = include_str!("fixtures/0003-strategy.sql");
const HISTORY_FIXTURE: &str = include_str!("../fixtures/2026-history.sql");
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

    /// `with_fixture` plus the prep session weekdays.
    async fn with_prep_days() -> Self {
        let t = Self::with_fixture().await;
        let mut c = t.writer().await;
        sqlx::raw_sql(PREP_DAYS_FIXTURE)
            .execute(&mut c)
            .await
            .unwrap();
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

fn planned(y: i64, w: i64) -> Value {
    json!({"isoYear": y, "isoWeek": w})
}

/// `markedDays` of the weekly fixture: Monday and Wednesday of 2026-W44 have shopping lists; their prep
/// marks need the prep-days fixture.
fn fixture_marks(prep: bool) -> Value {
    json!([
        {"date": "2026-10-26", "shopping": true, "prep": prep},
        {"date": "2026-10-28", "shopping": true, "prep": prep},
    ])
}

fn mark(date: &str, shopping: bool, prep: bool) -> Value {
    json!({"date": date, "shopping": shopping, "prep": prep})
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
        // `/2026/54` and `/%FF/1` are invalid weeks: their page reads `plannedWeeks`, so they fail too.
        for uri in ["/", "/2026/44", "/2026/45", "/2026", "/2026/54", "/%FF/1"] {
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
    assert_eq!(page["displayedDate"], "2027-01-02");

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
    assert_eq!(page["displayedDate"], "2026-11-01");
    assert!(page.get("selectedWeekday").is_none());

    // One second later it is Monday in W45.
    let (s, _, body) = get_at(&pool, berlin(2026, 11, 1, 23, 0, 0), "/").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["kind"], "week");
    assert_eq!(page["document"]["week"]["isoWeek"], 45);
    assert_eq!(page["displayedDate"], "2026-11-02");

    // The same instant in New York is still Sunday of W44.
    let new_york = Clock::fixed(
        chrono_tz::America::New_York,
        chrono::Utc.with_ymd_and_hms(2026, 11, 1, 23, 0, 0).unwrap(),
    );
    let (_, _, body) = get_at(&pool, new_york, "/").await;
    let page = page_json(&body);
    assert_eq!(page["document"]["week"]["isoWeek"], 44);
    assert_eq!(page["displayedDate"], "2026-11-01");
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
            "today": "2026-12-02",
            "plannedWeeks": [planned(2026, 44)],
            "markedDays": fixture_marks(false),
            "displayedDate": "2026-12-02",
            "week": summary(2026, 49, "2026-11-30", "2026-12-06", "/2026/49"),
        })
    );
}

#[tokio::test]
async fn explicit_week_displays_today_only_in_current_week() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2027, 2), (2026, 49)]).await;
    let pool = t.reader().await;

    // Clock is Wednesday 2026-12-02 of 2026-W49.
    let (s, _, body) = get(&pool, "/2026/49").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page_json(&body)["displayedDate"], "2026-12-02");
    // Past week and future week: the Monday, no redirect.
    for (uri, monday) in [("/2026/44", "2026-10-26"), ("/2027/2", "2027-01-11")] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        assert!(h.get(header::LOCATION).is_none());
        assert_eq!(page_json(&body)["displayedDate"], monday, "{uri}");
    }
    // With the clock inside W44 (Friday), /2026/44 displays today.
    let (_, _, body) = get_at(&pool, berlin(2026, 10, 30, 9, 0, 0), "/2026/44").await;
    assert_eq!(page_json(&body)["displayedDate"], "2026-10-30");
    // A missing week follows the same rule and stays a 404.
    let (s, _, body) = get(&pool, "/2026/45").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(page_json(&body)["displayedDate"], "2026-11-02");
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
            "today": "2026-12-02",
            "plannedWeeks": [planned(2026, 44), planned(2026, 53)],
            "markedDays": fixture_marks(false),
            "displayedDate": "2026-11-02",
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
        "/%FF/1",
        "/2026/%FF",
        "/2026/99999999999999999999",
        "/2026/00000000000000000000000000",
    ] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8", "{uri}");
        assert_eq!(
            page_json(&body),
            json!({
                "kind": "invalid",
                "today": "2026-12-02",
                "plannedWeeks": [planned(2026, 44)],
                "markedDays": fixture_marks(false),
                "displayedDate": null,
            }),
            "{uri}"
        );
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
    assert_eq!(page["displayedDate"], "2026-11-01");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn root_does_not_panic_when_current_week_is_unrepresentable() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (s, _, body) = get_at(&pool, berlin(9999, 12, 31, 12, 0, 0), "/").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        page_json(&body),
        json!({
            "kind": "invalid",
            "today": "9999-12-31",
            "plannedWeeks": [planned(2026, 44)],
            "markedDays": fixture_marks(false),
            "displayedDate": null,
        })
    );
}

#[tokio::test]
async fn page_today_follows_the_service_timezone_around_midnight() {
    use chrono::TimeZone;
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    // Berlin is UTC+1 on November 1, 2026: 22:59:59Z is still Sunday, 23:00:00Z is Monday.
    for (clock, today) in [
        (berlin(2026, 11, 1, 22, 59, 59), "2026-11-01"),
        (berlin(2026, 11, 1, 23, 0, 0), "2026-11-02"),
        (
            Clock::fixed(
                chrono_tz::America::New_York,
                chrono::Utc
                    .with_ymd_and_hms(2026, 11, 1, 22, 59, 59)
                    .unwrap(),
            ),
            "2026-11-01",
        ),
        (
            Clock::fixed(
                chrono_tz::America::New_York,
                chrono::Utc.with_ymd_and_hms(2026, 11, 1, 23, 0, 0).unwrap(),
            ),
            "2026-11-01",
        ),
    ] {
        // `/` is the stored W44 or the missing W45; the others are an explicit week, a missing week
        // and an invalid week.
        for (uri, kind) in [
            ("/", ["week", "missing"].as_slice()),
            ("/2026/44", ["week"].as_slice()),
            ("/2026/45", ["missing"].as_slice()),
            ("/2026/54", ["invalid"].as_slice()),
        ] {
            let (_, _, body) = get_at(&pool, clock.clone(), uri).await;
            let page = page_json(&body);
            assert!(kind.contains(&page["kind"].as_str().unwrap()), "{uri}");
            assert_eq!(page["today"], today, "{uri}");
        }
    }
}

#[tokio::test]
async fn today_is_zero_padded_to_four_digits() {
    let t = TestDb::migrated().await;
    let pool = t.reader().await;
    let (_, _, body) = get_at(&pool, berlin(1, 1, 4, 12, 0, 0), "/").await;
    let page = page_json(&body);
    assert_eq!(page["kind"], "missing");
    assert_eq!(page["today"], "0001-01-04");
    assert_eq!(page["week"]["path"], "/0001/1");
}

#[tokio::test]
async fn planned_weeks_are_ascending_and_the_same_on_every_page_kind() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2027, 1), (2020, 53), (2026, 1), (2025, 52)]).await;
    let pool = t.reader().await;
    let expected = json!([
        planned(2020, 53),
        planned(2025, 52),
        planned(2026, 1),
        planned(2026, 44),
        planned(2027, 1),
    ]);
    for (uri, kind) in [
        ("/2026/44", "week"),
        ("/2026/45", "missing"),
        ("/", "missing"),
        ("/2026/54", "invalid"),
        ("/%FF/1", "invalid"),
        ("/2026-10-30", "week"),
        ("/2026-11-02", "missing"),
        ("/2026-13-01", "invalidDate"),
    ] {
        let (_, _, body) = get(&pool, uri).await;
        let page = page_json(&body);
        assert_eq!(page["kind"], kind, "{uri}");
        assert_eq!(page["plannedWeeks"], expected, "{uri}");
        assert_eq!(page["markedDays"], fixture_marks(false), "{uri}");
    }
    // The key order puts `today` and `plannedWeeks` right after `kind`.
    let (_, _, body) = get(&pool, "/2026/44").await;
    assert!(body.contains(
        "{\"kind\":\"week\",\"today\":\"2026-12-02\",\"plannedWeeks\":[{\"isoYear\":2020,\"isoWeek\":53}"
    ));
}

#[tokio::test]
async fn empty_database_pages_have_no_planned_weeks() {
    let t = TestDb::migrated().await;
    let pool = t.reader().await;
    for (uri, status, expected) in [
        (
            "/",
            StatusCode::OK,
            json!({
                "kind": "missing",
                "today": "2026-12-02",
                "plannedWeeks": [],
                "markedDays": [],
                "displayedDate": "2026-12-02",
                "week": summary(2026, 49, "2026-11-30", "2026-12-06", "/2026/49"),
            }),
        ),
        (
            "/2026/45",
            StatusCode::NOT_FOUND,
            json!({
                "kind": "missing",
                "today": "2026-12-02",
                "plannedWeeks": [],
                "markedDays": [],
                "displayedDate": "2026-11-02",
                "week": summary(2026, 45, "2026-11-02", "2026-11-08", "/2026/45"),
            }),
        ),
        (
            "/2026/54",
            StatusCode::NOT_FOUND,
            json!({
                "kind": "invalid",
                "today": "2026-12-02",
                "plannedWeeks": [],
                "markedDays": [],
                "displayedDate": null,
            }),
        ),
        (
            "/2026-11-02",
            StatusCode::NOT_FOUND,
            json!({
                "kind": "missing",
                "today": "2026-12-02",
                "plannedWeeks": [],
                "markedDays": [],
                "displayedDate": "2026-11-02",
                "week": summary(2026, 45, "2026-11-02", "2026-11-08", "/2026/45"),
            }),
        ),
        (
            "/2026-02-30",
            StatusCode::NOT_FOUND,
            json!({
                "kind": "invalidDate",
                "today": "2026-12-02",
                "plannedWeeks": [],
                "markedDays": [],
                "displayedDate": null,
            }),
        ),
    ] {
        let (s, _, body) = get(&pool, uri).await;
        assert_eq!(s, status, "{uri}");
        assert_eq!(page_json(&body), expected, "{uri}");
    }
}

#[tokio::test]
async fn embedded_document_equals_the_api_body() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (_, _, body) = get(&pool, "/2026/44").await;
    let page = page_json(&body);
    let (s, _, api_body) = get(&pool, "/api/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        page["document"],
        serde_json::from_str::<Value>(&api_body).unwrap()
    );
    assert!(page["document"].get("strategyCards").is_none());
    assert_eq!(page["document"]["schemaVersion"], 3);
    assert!(page["document"].get("today").is_none());
    assert!(page["document"].get("plannedWeeks").is_none());
}

#[tokio::test]
async fn planned_weeks_are_read_fresh_on_every_request() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let planned_in = |body: &str| page_json(body)["plannedWeeks"].clone();

    let (_, _, body) = get(&pool, "/2026/44").await;
    assert_eq!(planned_in(&body), json!([planned(2026, 44)]));

    insert_weeks(&t, &[(2026, 46)]).await;
    for uri in ["/2026/44", "/2026/45", "/2026/54"] {
        let (_, _, body) = get(&pool, uri).await;
        assert_eq!(
            planned_in(&body),
            json!([planned(2026, 44), planned(2026, 46)]),
            "{uri}"
        );
    }

    let mut c = t.writer().await;
    exec(&mut c, "DELETE FROM weeks WHERE iso_week = 46")
        .await
        .unwrap();
    for uri in ["/2026/44", "/2026/45", "/2026/54"] {
        let (_, _, body) = get(&pool, uri).await;
        assert_eq!(planned_in(&body), json!([planned(2026, 44)]), "{uri}");
    }
}

#[tokio::test]
async fn week_and_planned_weeks_come_from_one_snapshot() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let mut writer = t.writer().await;

    // The week document is read and the snapshot is open; another connection commits a new week.
    let pause = async {
        exec(&mut writer, &week_sql(&uuid(900), 2026, 46))
            .await
            .unwrap();
    };
    let (document, calendar) = queries::load_week_page_with_pause(&pool, 2026, 44, pause)
        .await
        .unwrap();
    assert!(document.is_some());
    assert_eq!(
        serde_json::to_value(&calendar.planned_weeks).unwrap(),
        json!([planned(2026, 44)])
    );

    let (document, calendar) = queries::load_week_page(&pool, 2026, 44).await.unwrap();
    assert!(document.is_some());
    assert_eq!(
        serde_json::to_value(&calendar.planned_weeks).unwrap(),
        json!([planned(2026, 44), planned(2026, 46)])
    );
}

#[tokio::test]
async fn marked_days_come_from_the_same_snapshot_as_the_document() {
    let t = TestDb::with_prep_days().await;
    let pool = t.reader().await;
    let mut writer = t.writer().await;

    // The document is read and the snapshot is open; another connection commits a shopping link and
    // a prep weekday change. Neither is visible in this snapshot.
    let pause = async {
        exec(
            &mut writer,
            &format!(
                "INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) VALUES ('{TUE}', '{L1}', 0)"
            ),
        )
        .await
        .unwrap();
        exec(
            &mut writer,
            &format!("UPDATE prep_sessions SET weekday = 5 WHERE id = '{SESSION_MON}'"),
        )
        .await
        .unwrap();
    };
    let (document, calendar) = queries::load_week_page_with_pause(&pool, 2026, 44, pause)
        .await
        .unwrap();
    let document = serde_json::to_value(document.unwrap()).unwrap();
    assert_eq!(document["days"][1]["shoppingListIds"], json!([]));
    assert_eq!(document["prepSessions"][0]["weekday"], 1);
    assert_eq!(
        serde_json::to_value(&calendar.marked_days).unwrap(),
        fixture_marks(true)
    );

    let (document, calendar) = queries::load_week_page(&pool, 2026, 44).await.unwrap();
    let document = serde_json::to_value(document.unwrap()).unwrap();
    assert_eq!(document["days"][1]["shoppingListIds"], json!([L1]));
    assert_eq!(
        serde_json::to_value(&calendar.marked_days).unwrap(),
        json!([
            mark("2026-10-26", true, false),
            mark("2026-10-27", true, false),
            mark("2026-10-28", true, true),
            mark("2026-10-30", false, true),
        ])
    );
}

#[tokio::test]
async fn year_pages_have_no_calendar_data() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (_, _, body) = get(&pool, "/2026").await;
    let page = page_json(&body);
    assert_eq!(page["kind"], "year");
    assert!(page.get("today").is_none());
    assert!(page.get("plannedWeeks").is_none());
}

#[tokio::test]
async fn year_page_lists_the_year_newest_first() {
    // The fixture clock is in December 2026, so 2026 is the current year.
    let t = TestDb::with_fixture().await;
    insert_weeks(
        &t,
        &[
            (2027, 1),
            (2025, 52),
            (2026, 53),
            (2026, 1),
            (2026, 49),
            (2020, 53),
        ],
    )
    .await;
    let pool = t.reader().await;

    // Future weeks are listed; 2026/1 starts in 2025 but belongs to 2026; 2026/53 ends in 2027.
    let (s, h, body) = get(&pool, "/2026").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(title(&body), "2026 — Meal Plan");
    assert_eq!(
        page_json(&body),
        json!({
            "kind": "year",
            "year": 2026,
            "previousYear": 2025,
            "nextYear": 2027,
            "weeks": [
                summary(2026, 53, "2026-12-28", "2027-01-03", "/2026/53"),
                summary(2026, 49, "2026-11-30", "2026-12-06", "/2026/49"),
                summary(2026, 44, "2026-10-26", "2026-11-01", "/2026/44"),
                summary(2026, 1, "2025-12-29", "2026-01-04", "/2026/1"),
            ],
        })
    );

    for (uri, expected) in [
        ("/2025", vec!["/2025/52"]),
        ("/2027", vec!["/2027/1"]),
        ("/2020", vec!["/2020/53"]),
    ] {
        let (s, _, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        let paths: Vec<_> = page_json(&body)["weeks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["path"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(paths, expected, "{uri}");
    }
}

#[tokio::test]
async fn year_title_is_not_zero_padded() {
    let t = TestDb::migrated().await;
    insert_weeks(&t, &[(1, 1)]).await;
    let pool = t.reader().await;
    let (s, _, body) = get(&pool, "/0001").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(title(&body), "1 — Meal Plan");
    assert_eq!(page_json(&body)["year"], 1);
}

#[tokio::test]
async fn pages_carry_their_own_iso_week_year_and_the_clock_date() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2025, 50)]).await;
    let pool = t.reader().await;

    // Stored and missing weeks both carry their own ISO week-year.
    let (_, _, body) = get(&pool, "/2025/50").await;
    assert_eq!(page_json(&body)["document"]["week"]["isoYear"], 2025);
    let (_, _, body) = get(&pool, "/2025/49").await;
    assert_eq!(page_json(&body)["week"]["isoYear"], 2025);

    // `/` at 2025-12-29 Berlin is ISO 2026 week 1, a calendar-2025 date.
    let (_, _, body) = get_at(&pool, berlin(2025, 12, 29, 12, 0, 0), "/").await;
    let page = page_json(&body);
    assert_eq!(page["kind"], "missing");
    assert_eq!(page["week"]["isoYear"], 2026);

    // Invalid week pages carry the clock's date in the service timezone, on both sides of New Year.
    for (clock, today) in [
        (berlin(2025, 12, 28, 21, 59, 59), "2025-12-28"),
        (berlin(2025, 12, 28, 22, 59, 59), "2025-12-28"),
        (berlin(2025, 12, 28, 23, 0, 0), "2025-12-29"),
        (berlin(2027, 1, 3, 12, 0, 0), "2027-01-03"),
        (berlin(2027, 1, 4, 12, 0, 0), "2027-01-04"),
    ] {
        let (s, _, body) = get_at(&pool, clock, "/2025/53").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(
            page_json(&body),
            json!({
                "kind": "invalid",
                "today": today,
                "plannedWeeks": [planned(2025, 50), planned(2026, 44)],
                "markedDays": fixture_marks(false),
                "displayedDate": null,
            })
        );
    }
}

#[tokio::test]
async fn year_links_skip_gap_years() {
    let t = TestDb::migrated().await;
    insert_weeks(&t, &[(2023, 5), (2024, 5), (2026, 5)]).await;
    let pool = t.reader().await;

    for (uri, status, previous, next) in [
        ("/2024", StatusCode::OK, json!(2023), json!(null)),
        ("/2023", StatusCode::OK, json!(null), json!(2024)),
        // 2025 has no plans: not linked from 2026, even though 2024 has some.
        ("/2026", StatusCode::OK, json!(null), json!(null)),
        // Empty, not current: 404 with the adjacent years that do have plans.
        ("/2025", StatusCode::NOT_FOUND, json!(2024), json!(2026)),
    ] {
        let (s, _, body) = get(&pool, uri).await;
        let page = page_json(&body);
        assert_eq!(s, status, "{uri}");
        assert_eq!(page["kind"], "year", "{uri}");
        assert_eq!(page["previousYear"], previous, "{uri}");
        assert_eq!(page["nextYear"], next, "{uri}");
    }
}

#[tokio::test]
async fn year_status_empty_database_and_current_year() {
    let empty = TestDb::migrated().await;
    let pool = empty.reader().await;
    let (s, _, body) = get(&pool, "/2026").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        page_json(&body),
        json!({"kind": "year", "year": 2026, "previousYear": null, "nextYear": null, "weeks": []})
    );
    let (s, h, body) = get(&pool, "/2025").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(
        page_json(&body),
        json!({"kind": "year", "year": 2025, "previousYear": null, "nextYear": null, "weeks": []})
    );
    assert_eq!(title(&body), "2025 — Meal Plan");

    // The year that `/` shows is always a 200 page, whatever is stored.
    for clock in [
        berlin(2026, 12, 2, 12, 0, 0),
        berlin(2025, 12, 29, 12, 0, 0),
    ] {
        let (_, _, body) = get_at(&pool, clock.clone(), "/").await;
        let year = page_json(&body)["week"]["isoYear"].as_i64().unwrap();
        let (s, _, _) = get_at(&pool, clock, &format!("/{year:04}")).await;
        assert_eq!(s, StatusCode::OK, "/{year:04}");
    }

    // Non-empty database: the current year without plans is 200, another empty year is 404.
    let t = TestDb::migrated().await;
    insert_weeks(&t, &[(2024, 10)]).await;
    let pool = t.reader().await;
    let (s, _, body) = get(&pool, "/2026").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page_json(&body)["weeks"], json!([]));
    let (s, _, _) = get(&pool, "/2025").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, _) = get(&pool, "/2024").await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn current_year_follows_the_iso_week_year_in_the_service_timezone() {
    use chrono::TimeZone;
    let t = TestDb::migrated().await;
    let pool = t.reader().await;

    for (clock, current) in [
        // Sunday 2025-12-28 23:59:59 Berlin is ISO 2025; one second later it is Monday, ISO 2026.
        (berlin(2025, 12, 28, 22, 59, 59), 2025),
        (berlin(2025, 12, 28, 23, 0, 0), 2026),
        // Sunday 2027-01-03 is still ISO 2026; Monday 2027-01-04 is ISO 2027.
        (berlin(2027, 1, 3, 12, 0, 0), 2026),
        (berlin(2027, 1, 4, 12, 0, 0), 2027),
    ] {
        for year in [2025, 2026, 2027] {
            let (s, _, _) = get_at(&pool, clock.clone(), &format!("/{year}")).await;
            let expected = if year == current {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(s, expected, "/{year} with current year {current}");
        }
    }

    // The same instant is Monday in Berlin (ISO 2026) and still Sunday in New York (ISO 2025).
    let instant = chrono::Utc
        .with_ymd_and_hms(2025, 12, 29, 0, 30, 0)
        .unwrap();
    for (tz, current) in [
        (sous::calendar::DEFAULT_TIMEZONE, 2026),
        (chrono_tz::America::New_York, 2025),
    ] {
        for year in [2025, 2026] {
            let (s, _, _) = get_at(&pool, Clock::fixed(tz, instant), &format!("/{year}")).await;
            let expected = if year == current {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(s, expected, "/{year} in {tz}");
        }
    }
}

#[tokio::test]
async fn first_and_last_supported_years() {
    let t = TestDb::migrated().await;
    insert_weeks(&t, &[(1, 1), (9999, 51), (9999, 1)]).await;
    let pool = t.reader().await;

    let (s, _, body) = get(&pool, "/0001").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["previousYear"], json!(null));
    assert_eq!(page["nextYear"], json!(null));
    assert_eq!(
        page["weeks"],
        json!([summary(1, 1, "0001-01-01", "0001-01-07", "/0001/1")])
    );

    let (s, _, body) = get(&pool, "/0002").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(page_json(&body)["previousYear"], 1);
    assert_eq!(page_json(&body)["nextYear"], json!(null));

    let (s, _, body) = get(&pool, "/9999").await;
    assert_eq!(s, StatusCode::OK);
    let page = page_json(&body);
    assert_eq!(page["nextYear"], json!(null));
    assert_eq!(page["weeks"].as_array().unwrap().len(), 2);
    assert_eq!(page["weeks"][0]["path"], "/9999/51");

    let (s, _, body) = get(&pool, "/9998").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(page_json(&body)["nextYear"], 9999);
}

#[tokio::test]
async fn non_year_single_segments_are_plain_404() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let (_dir, tableless) = tableless_pool().await;
    let closed = t.reader().await;
    closed.close().await;

    for uri in [
        "/weeks",
        "/weeks/",
        "/nothing",
        "/26",
        "/0000",
        "/02026",
        "/10000",
        "/+026",
        "/%EF%BC%92%EF%BC%90%EF%BC%92%EF%BC%96",
        "/%FF",
        "/%202026",
        "/2026%20",
    ] {
        // Rejected before any database access: an unavailable database changes nothing.
        for pool in [&pool, &tableless, &closed] {
            let (s, h, body) = get(pool, uri).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
            assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8", "{uri}");
            assert!(!body.contains("sous-page"), "{uri}");
        }
    }
}

#[tokio::test]
async fn year_path_methods() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for (uri, expected) in [
        ("/2026", StatusCode::METHOD_NOT_ALLOWED),
        ("/0001", StatusCode::METHOD_NOT_ALLOWED),
        ("/weeks", StatusCode::NOT_FOUND),
        ("/nothing", StatusCode::NOT_FOUND),
        ("/02026", StatusCode::NOT_FOUND),
    ] {
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
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
            assert_eq!(res.status(), expected, "{method} {uri}");
            assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        }
    }
}

#[tokio::test]
async fn year_page_503_when_database_unavailable() {
    let (_dir, tableless) = tableless_pool().await;
    let t = TestDb::with_fixture().await;
    let closed = t.reader().await;
    closed.close().await;

    // Current year and another year alike.
    for pool in [&tableless, &closed] {
        for uri in ["/2026", "/2025"] {
            let (s, h, body) = get(pool, uri).await;
            assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
            assert_eq!(page_json(&body), json!({"kind": "unavailable"}), "{uri}");
        }
    }
}

#[tokio::test]
async fn year_page_is_fresh() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let paths = |body: &str| -> Vec<String> {
        page_json(body)["weeks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["path"].as_str().unwrap().to_string())
            .collect()
    };

    let (_, _, body) = get(&pool, "/2026").await;
    assert_eq!(paths(&body), ["/2026/44"]);

    // An open transaction's insert is invisible; once it ends the week shows up.
    let mut c = t.writer().await;
    let mut tx = c.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(week_sql(&uuid(7), 2026, 50)))
        .execute(&mut *tx)
        .await
        .unwrap();
    let (_, _, body) = get(&pool, "/2026").await;
    assert_eq!(paths(&body), ["/2026/44"]);
    tx.commit().await.unwrap();
    let (_, _, body) = get(&pool, "/2026").await;
    assert_eq!(paths(&body), ["/2026/50", "/2026/44"]);

    // The first plan of the next year makes the link appear; deleting it removes the link.
    assert_eq!(page_json(&body)["nextYear"], json!(null));
    exec(&mut c, &week_sql(&uuid(8), 2027, 1)).await.unwrap();
    let (_, _, body) = get(&pool, "/2026").await;
    assert_eq!(page_json(&body)["nextYear"], 2027);
    exec(&mut c, "DELETE FROM weeks WHERE iso_year = 2027")
        .await
        .unwrap();
    let (_, _, body) = get(&pool, "/2026").await;
    assert_eq!(page_json(&body)["nextYear"], json!(null));

    // Deleted weeks disappear.
    exec(&mut c, "DELETE FROM weeks WHERE iso_week = 50")
        .await
        .unwrap();
    let (_, _, body) = get(&pool, "/2026").await;
    assert_eq!(paths(&body), ["/2026/44"]);
}

#[tokio::test]
async fn non_get_methods_are_405() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for uri in ["/", "/2026", "/2026/44"] {
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
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
    for uri in ["/nothing", "/weeks/", "/2026/", "/2026/44/extra"] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        assert!(!body.contains("sous-page"), "{uri}");
    }
}

// ---- Feature 13: day view ----

/// Page body, plus the `displayedDate` and `kind` of its payload.
async fn page_at(pool: &SqlitePool, clock: Clock, uri: &str) -> (StatusCode, Value) {
    let (s, _, body) = get_at(pool, clock, uri).await;
    (s, page_json(&body))
}

#[tokio::test]
async fn root_displayed_date_is_today_in_the_service_timezone() {
    use chrono::TimeZone;
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2026, 45), (2026, 46)]).await;
    let pool = t.reader().await;
    let new_york = |y, m, d, h, min, s| {
        Clock::fixed(
            chrono_tz::America::New_York,
            chrono::Utc.with_ymd_and_hms(y, m, d, h, min, s).unwrap(),
        )
    };

    for (clock, date, week, what) in [
        (
            berlin(2026, 11, 11, 22, 59, 59),
            "2026-11-11",
            46,
            "Berlin before midnight",
        ),
        (
            berlin(2026, 11, 11, 23, 0, 0),
            "2026-11-12",
            46,
            "Berlin after midnight",
        ),
        // Sunday -> Monday: the week changes with the date.
        (
            berlin(2026, 11, 8, 22, 59, 59),
            "2026-11-08",
            45,
            "Berlin Sunday",
        ),
        (
            berlin(2026, 11, 8, 23, 0, 0),
            "2026-11-09",
            46,
            "Berlin Monday",
        ),
        // The same instants in New York are still the previous evening.
        (
            new_york(2026, 11, 11, 22, 59, 59),
            "2026-11-11",
            46,
            "New York 17:59",
        ),
        (
            new_york(2026, 11, 11, 23, 0, 0),
            "2026-11-11",
            46,
            "New York 18:00",
        ),
        (
            new_york(2026, 11, 12, 4, 59, 59),
            "2026-11-11",
            46,
            "New York before midnight",
        ),
        (
            new_york(2026, 11, 12, 5, 0, 0),
            "2026-11-12",
            46,
            "New York after midnight",
        ),
        (
            new_york(2026, 11, 9, 4, 59, 59),
            "2026-11-08",
            45,
            "New York Sunday",
        ),
        (
            new_york(2026, 11, 9, 5, 0, 0),
            "2026-11-09",
            46,
            "New York Monday",
        ),
    ] {
        let (s, page) = page_at(&pool, clock, "/").await;
        assert_eq!(s, StatusCode::OK, "{what}");
        assert_eq!(page["kind"], "week", "{what}");
        assert_eq!(page["displayedDate"], date, "{what}");
        assert_eq!(page["today"], date, "{what}");
        assert_eq!(page["document"]["week"]["isoWeek"], week, "{what}");
        assert!(page.get("selectedWeekday").is_none(), "{what}");
    }

    // No page kind carries `selectedWeekday`.
    for uri in [
        "/",
        "/2026/44",
        "/2026/45",
        "/2026/54",
        "/2026-10-28",
        "/2026-11-02",
        "/2026-02-30",
    ] {
        let (_, page) = page_at(&pool, test_clock(), uri).await;
        assert!(page.get("selectedWeekday").is_none(), "{uri}");
    }
}

#[tokio::test]
async fn date_pages_show_the_week_that_holds_the_date() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2026, 46), (2026, 1), (2026, 53)]).await;
    let pool = t.reader().await;

    let (s, h, body) = get(&pool, "/2026-11-11").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(title(&body), "Meal Plan — 2026 week 46");
    let page = page_json(&body);
    assert_eq!(page["kind"], "week");
    assert_eq!(page["displayedDate"], "2026-11-11");
    assert_eq!(page["document"]["week"]["isoYear"], 2026);
    assert_eq!(page["document"]["week"]["isoWeek"], 46);
    assert_eq!(page["today"], "2026-12-02");

    // The date of an explicit week page: today inside the current week, else the Monday.
    let (_, page) = page_at(&pool, test_clock(), "/2026/46").await;
    assert_eq!(page["displayedDate"], "2026-11-09");
    let (_, page) = page_at(&pool, berlin(2026, 11, 12, 9, 0, 0), "/2026/46").await;
    assert_eq!(page["displayedDate"], "2026-11-12");
    // The clock does not matter for a date page.
    let (_, page) = page_at(&pool, berlin(2026, 11, 12, 9, 0, 0), "/2026-11-15").await;
    assert_eq!(page["displayedDate"], "2026-11-15");

    // ISO year boundaries: 2025-12-29 opens 2026-W1, 2027-01-03 opens 2026-W53.
    let (s, h, body) = get(&pool, "/2025-12-29").await;
    assert_eq!(s, StatusCode::OK);
    assert!(h.get(header::LOCATION).is_none());
    assert_eq!(title(&body), "Meal Plan — 2026 week 1");
    let page = page_json(&body);
    assert_eq!(page["displayedDate"], "2025-12-29");
    assert_eq!(page["document"]["week"]["isoYear"], 2026);
    assert_eq!(page["document"]["week"]["isoWeek"], 1);
    let (_, page) = page_at(&pool, test_clock(), "/2027-01-03").await;
    assert_eq!(page["displayedDate"], "2027-01-03");
    assert_eq!(page["document"]["week"]["isoYear"], 2026);
    assert_eq!(page["document"]["week"]["isoWeek"], 53);

    // The API serves the same document, byte for byte, under the date and under the week.
    let (_, _, by_date) = get(&pool, "/api/2025-12-29").await;
    let (s, _, by_week) = get(&pool, "/api/2026/1").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(by_date, by_week);
}

#[tokio::test]
async fn date_page_status_table() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;

    // Valid date, no plan: the missing page for the date's ISO week, a 404.
    for (uri, path, range) in [
        ("/2026-11-11", "/2026/46", ("2026-11-09", "2026-11-15")),
        ("/0001-01-01", "/0001/1", ("0001-01-01", "0001-01-07")),
        ("/9999-12-26", "/9999/51", ("9999-12-20", "9999-12-26")),
    ] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        let page = page_json(&body);
        assert_eq!(page["kind"], "missing", "{uri}");
        assert_eq!(page["displayedDate"], &uri[1..], "{uri}");
        assert_eq!(page["week"]["path"], path, "{uri}");
        assert_eq!(page["week"]["startDate"], range.0, "{uri}");
        assert_eq!(page["week"]["endDate"], range.1, "{uri}");
        assert_eq!(page["today"], "2026-12-02", "{uri}");
        assert!(page["plannedWeeks"].is_array(), "{uri}");
        assert!(page["markedDays"].is_array(), "{uri}");
    }
    let (_, _, body) = get(&pool, "/2026-11-11").await;
    assert_eq!(title(&body), "Meal Plan — 2026 week 46");

    // The same dates with a bare week row stored are 200 week pages.
    insert_weeks(&t, &[(1, 1), (9999, 51)]).await;
    for uri in ["/0001-01-01", "/9999-12-26"] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        let page = page_json(&body);
        assert_eq!(page["kind"], "week", "{uri}");
        assert_eq!(page["displayedDate"], &uri[1..], "{uri}");
    }

    // Date-shaped but not a valid date path.
    for uri in [
        "/2026-02-30",
        "/2025-02-29",
        "/0000-01-01",
        "/2026-13-01",
        "/2026-00-10",
        "/2026-01-00",
        "/9999-12-27",
        "/9999-12-31",
    ] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8", "{uri}");
        assert_eq!(title(&body), "Invalid date — Meal Plan", "{uri}");
        assert_eq!(
            page_json(&body),
            json!({
                "kind": "invalidDate",
                "today": "2026-12-02",
                "plannedWeeks": [planned(1, 1), planned(2026, 44), planned(9999, 51)],
                "markedDays": fixture_marks(false),
                "displayedDate": null,
            }),
            "{uri}"
        );
    }

    // Not date-shaped: the plain not-found page.
    for uri in [
        "/2026-1-5",
        "/2026-11",
        "/2026-11-111",
        "/20261111",
        "/%FF",
        "/weeks",
        "/%EF%BC%92%EF%BC%90%EF%BC%92%EF%BC%96-11-11",
        "/%202026-11-11",
    ] {
        let (s, h, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8", "{uri}");
        assert!(!body.contains("sous-page"), "{uri}");
    }

    // A missing plan at `/` is still a 200.
    let (s, _, body) = get(&pool, "/").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page_json(&body)["kind"], "missing");
}

#[tokio::test]
async fn date_paths_allow_only_get_and_head() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for uri in [
        "/2026-11-11",
        "/2026-02-30",
        "/0000-01-01",
        "/9999-12-31",
        "/api/2026-11-11",
        "/api/2026-02-30",
        "/api/9999-12-31",
    ] {
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            let (s, h, _) = send(&pool, method, uri, None).await;
            assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri}");
            assert_eq!(h[header::ALLOW], "GET,HEAD", "{method} {uri}");
            assert_eq!(h[header::CACHE_CONTROL], "no-store", "{method} {uri}");
        }
    }
    // Other single segments keep the plain not-found response.
    for method in ["POST", "PUT", "DELETE"] {
        let (s, _, body) = send(&pool, method, "/2026-1-5", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{method}");
        assert!(!body.contains("sous-page"), "{method}");
        let res = send(&pool, method, "/api/2026-1-5", None).await;
        assert_json_response(&res, StatusCode::NOT_FOUND, &api_not_found(), "api");
    }
}

#[tokio::test]
async fn date_pages_503_when_database_unavailable() {
    let (_dir, tableless) = tableless_pool().await;
    let t = TestDb::with_fixture().await;
    let closed = t.reader().await;
    closed.close().await;
    for pool in [&tableless, &closed] {
        // Valid with or without a plan, and invalid: all need the database.
        for uri in ["/2026-10-28", "/2026-11-11", "/2026-02-30"] {
            let (s, h, body) = get(pool, uri).await;
            assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            assert_eq!(h[header::CACHE_CONTROL], "no-store", "{uri}");
            assert_eq!(page_json(&body), json!({"kind": "unavailable"}), "{uri}");
        }
        // A non-date segment is rejected before any database access.
        let (s, _, _) = get(pool, "/2026-1-5").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn api_date_routes() {
    let t = TestDb::with_fixture().await;
    insert_weeks(&t, &[(2026, 46), (2026, 1), (2026, 53)]).await;
    let pool = t.reader().await;
    let nf = StatusCode::NOT_FOUND;

    // Same document as the week route, byte for byte; no redirects.
    for (date, year, week) in [
        ("2026-10-28", 2026, 44),
        ("2026-11-11", 2026, 46),
        ("2025-12-29", 2026, 1),
        ("2027-01-03", 2026, 53),
    ] {
        let (s, h, by_date) = get(&pool, &format!("/api/{date}")).await;
        assert_eq!(s, StatusCode::OK, "{date}");
        assert_eq!(h[header::CONTENT_TYPE], "application/json", "{date}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{date}");
        let (_, _, by_week) = get(&pool, &format!("/api/{year}/{week}")).await;
        assert_eq!(by_date, by_week, "{date}");
    }

    // Valid date, no plan.
    for uri in [
        "/api/2026-11-02",
        "/api/0001-01-01",
        "/api/9999-12-26",
        "/api/2026-12-02",
    ] {
        let res = get(&pool, uri).await;
        assert_json_response(&res, nf, &week_not_found(), uri);
    }
    let mut c = t.writer().await;
    exec(&mut c, &week_sql(&uuid(900), 1, 1)).await.unwrap();
    exec(&mut c, &week_sql(&uuid(901), 9999, 51)).await.unwrap();
    for uri in ["/api/0001-01-01", "/api/9999-12-26"] {
        let (s, _, _) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
    }

    // Date-shaped but invalid.
    for uri in [
        "/api/2026-02-30",
        "/api/2025-02-29",
        "/api/0000-01-01",
        "/api/2026-13-01",
        "/api/9999-12-27",
        "/api/9999-12-31",
    ] {
        let res = get(&pool, uri).await;
        assert_json_response(&res, nf, &invalid_date_error(), uri);
    }

    // Neither a year nor date-shaped.
    for uri in [
        "/api/2026-1-5",
        "/api/2026-11",
        "/api/20261111",
        "/api/%FF",
        "/api/weeks",
    ] {
        let res = get(&pool, uri).await;
        assert_json_response(&res, nf, &api_not_found(), uri);
    }

    // The year route is unchanged.
    assert_eq!(get(&pool, "/api/2026").await.0, StatusCode::OK);
    assert_eq!(get(&pool, "/api/2025").await.0, nf);
}

/// Insert week `(y, w)` with a day on `weekday` that links one shopping list. Ids derive from `n`.
async fn add_shopping_day(c: &mut SqliteConnection, n: u32, y: i32, w: i32, weekday: i32) {
    let (week, day, list) = (uuid(n), uuid(n + 1), uuid(n + 2));
    exec(c, &week_sql(&week, y, w)).await.unwrap();
    exec(c, &day_sql(&day, &week, weekday)).await.unwrap();
    exec(
        c,
        &format!(
            "INSERT INTO shopping_lists (id, week_id, position, label) VALUES ('{list}', '{week}', 0, 'l')"
        ),
    )
    .await
    .unwrap();
    exec(
        c,
        &format!(
            "INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) VALUES ('{day}', '{list}', 0)"
        ),
    )
    .await
    .unwrap();
}

/// Insert a prep session with `tasks` tasks on an existing week. Ids derive from `n`.
async fn add_session(
    c: &mut SqliteConnection,
    n: u32,
    week: &str,
    position: i32,
    weekday: Option<i32>,
    tasks: u32,
) {
    let session = uuid(n);
    let weekday = weekday.map_or("NULL".to_string(), |d| d.to_string());
    exec(
        c,
        &format!(
            "INSERT INTO prep_sessions (id, week_id, position, title, weekday) \
             VALUES ('{session}', '{week}', {position}, 's', {weekday})"
        ),
    )
    .await
    .unwrap();
    for i in 0..tasks {
        exec(
            c,
            &format!(
                "INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) \
                 VALUES ('{}', '{session}', {i}, 'n', 't', 'i')",
                uuid(n + 1 + i)
            ),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn marked_days_for_the_pinned_fixtures() {
    // Without the prep-days file the sessions have no weekday: shopping marks only.
    let t = TestDb::with_fixture().await;
    let (_, page) = page_at(&t.reader().await, test_clock(), "/2026/44").await;
    assert_eq!(page["markedDays"], fixture_marks(false));

    let t = TestDb::with_prep_days().await;
    let pool = t.reader().await;
    let (_, page) = page_at(&pool, test_clock(), "/2026/44").await;
    assert_eq!(
        page["markedDays"],
        json!([
            {"date": "2026-10-26", "shopping": true, "prep": true},
            {"date": "2026-10-28", "shopping": true, "prep": true},
        ])
    );
    // The key order is date, shopping, prep.
    let (_, _, body) = get(&pool, "/2026/44").await;
    assert!(
        body.contains("\"markedDays\":[{\"date\":\"2026-10-26\",\"shopping\":true,\"prep\":true}")
    );
    // The prep sessions carry their weekday right after `covers`.
    let (_, _, api) = get(&pool, "/api/2026/44").await;
    assert!(api.contains("\"covers\":"));
    let doc: Value = serde_json::from_str(&api).unwrap();
    let weekdays: Vec<(&str, &Value)> = doc["prepSessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap(), &s["weekday"]))
        .collect();
    assert!(weekdays.contains(&(SESSION_MON, &json!(1))));
    assert!(weekdays.contains(&(SESSION_WED, &json!(3))));

    // History weeks have no shopping links and no prep sessions.
    let mut c = t.writer().await;
    sqlx::raw_sql(HISTORY_FIXTURE)
        .execute(&mut c)
        .await
        .unwrap();
    let (_, page) = page_at(&pool, test_clock(), "/2026/44").await;
    assert_eq!(page["markedDays"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn marked_days_need_a_weekday_and_a_task_for_prep() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let mut c = t.writer().await;

    // A weekday but no tasks, and tasks but no weekday (the fixture sessions): no prep mark.
    add_session(&mut c, 3000, FIXTURE_WEEK, 20, Some(5), 0).await;
    let (_, page) = page_at(&pool, test_clock(), "/2026/44").await;
    assert_eq!(page["markedDays"], fixture_marks(false));

    // A weekday and a task: a prep mark on a day without shopping.
    add_session(&mut c, 3100, FIXTURE_WEEK, 21, Some(5), 1).await;
    let (_, page) = page_at(&pool, test_clock(), "/2026/44").await;
    assert_eq!(
        page["markedDays"],
        json!([
            mark("2026-10-26", true, false),
            mark("2026-10-28", true, false),
            mark("2026-10-30", false, true),
        ])
    );

    // A day with a shopping link but no prep shows `prep: false` next to a day with both.
    exec(
        &mut c,
        &format!("UPDATE prep_sessions SET weekday = 3 WHERE id = '{SESSION_WED}'"),
    )
    .await
    .unwrap();
    let (_, page) = page_at(&pool, test_clock(), "/2026/44").await;
    assert_eq!(
        page["markedDays"],
        json!([
            mark("2026-10-26", true, false),
            mark("2026-10-28", true, true),
            mark("2026-10-30", false, true),
        ])
    );
}

#[tokio::test]
async fn marked_days_are_ascending_across_weeks_and_iso_years() {
    let t = TestDb::migrated().await;
    let pool = t.reader().await;
    let mut c = t.writer().await;
    // Inserted out of order on purpose.
    add_shopping_day(&mut c, 1000, 2027, 1, 1).await;
    add_shopping_day(&mut c, 2000, 2026, 44, 3).await;
    add_shopping_day(&mut c, 4000, 2026, 1, 1).await;
    add_shopping_day(&mut c, 5000, 2025, 52, 7).await;
    // A prep-only mark in 2026-W1.
    add_session(&mut c, 6000, &uuid(4000), 0, Some(2), 1).await;

    for uri in [
        "/2026/44",
        "/2026/45",
        "/2026/54",
        "/2026-11-11",
        "/2026-02-30",
    ] {
        let (_, page) = page_at(&pool, test_clock(), uri).await;
        assert_eq!(
            page["markedDays"],
            json!([
                mark("2025-12-28", true, false),
                mark("2025-12-29", true, false),
                mark("2025-12-30", false, true),
                mark("2026-10-28", true, false),
                mark("2027-01-04", true, false),
            ]),
            "{uri}"
        );
    }
}

#[tokio::test]
async fn marked_days_follow_committed_changes() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let marks = |uri: &'static str| {
        let pool = pool.clone();
        async move { page_at(&pool, test_clock(), uri).await.1["markedDays"].clone() }
    };
    let mut c = t.writer().await;
    assert_eq!(marks("/2026/44").await, fixture_marks(false));

    // An uncommitted weekday is invisible; once committed it shows on the next request.
    let mut tx = c.begin().await.unwrap();
    exec(
        &mut tx,
        &format!("UPDATE prep_sessions SET weekday = 1 WHERE id = '{SESSION_MON}'"),
    )
    .await
    .unwrap();
    assert_eq!(marks("/2026/44").await, fixture_marks(false));
    tx.commit().await.unwrap();
    assert_eq!(
        marks("/2026/44").await,
        json!([
            mark("2026-10-26", true, true),
            mark("2026-10-28", true, false)
        ])
    );

    // A new shopping link.
    exec(
        &mut c,
        &format!(
            "INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) VALUES ('{TUE}', '{L1}', 0)"
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        marks("/2026-10-27").await,
        json!([
            mark("2026-10-26", true, true),
            mark("2026-10-27", true, false),
            mark("2026-10-28", true, false),
        ])
    );

    // Deleting some tasks keeps the mark, deleting the last one removes it.
    exec(
        &mut c,
        &format!(
            "DELETE FROM prep_tasks WHERE id = (SELECT id FROM prep_tasks WHERE session_id = '{SESSION_MON}' ORDER BY position LIMIT 1)"
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        marks("/2026/44").await.as_array().unwrap()[0],
        mark("2026-10-26", true, true)
    );
    exec(
        &mut c,
        &format!("DELETE FROM prep_tasks WHERE session_id = '{SESSION_MON}'"),
    )
    .await
    .unwrap();
    assert_eq!(
        marks("/2026/44").await.as_array().unwrap()[0],
        mark("2026-10-26", true, false)
    );

    // Deleting the week removes its marks.
    exec(&mut c, "DELETE FROM weeks").await.unwrap();
    assert_eq!(marks("/2026/44").await, json!([]));
}

#[tokio::test]
async fn embedded_document_equals_the_api_body_on_date_routes() {
    let t = TestDb::with_prep_days().await;
    let pool = t.reader().await;
    for (page_uri, api_uri) in [
        ("/2026/44", "/api/2026/44"),
        ("/2026-10-28", "/api/2026-10-28"),
        ("/2026-11-01", "/api/2026-11-01"),
    ] {
        let (_, page) = page_at(&pool, test_clock(), page_uri).await;
        let (s, _, api) = get(&pool, api_uri).await;
        assert_eq!(s, StatusCode::OK, "{api_uri}");
        assert_eq!(
            page["document"],
            serde_json::from_str::<Value>(&api).unwrap(),
            "{page_uri}"
        );
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

// ---- Feature 4: prep and reminders (strategy cards were removed by feature 12) ----

const SESSION_MON: &str = "00000008-0000-4000-8000-000000000035";
const SESSION_WED: &str = "00000008-0000-4000-8000-000000000032";
const WED: &str = "00000002-0000-4000-8000-000000000004";

type Group = Vec<(String, Vec<String>)>;

/// Prep content as `(title, [task name])` pairs in document order.
fn weekly_order(doc: &Value) -> Group {
    doc["prepSessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["title"].as_str().unwrap().to_string(),
                p["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["name"].as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
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
    assert!(upgraded.get("strategyCards").is_none());
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

/// Every row of `tables`, all columns, ordered by all columns, as text.
async fn row_snapshot(c: &mut SqliteConnection, tables: &[&str]) -> Vec<String> {
    use sqlx::Row;
    let mut out = Vec::new();
    for table in tables {
        let columns: Vec<String> =
            sqlx::query("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(table)
                .fetch_all(&mut *c)
                .await
                .unwrap()
                .iter()
                .map(|row| row.get::<String, _>(0))
                .collect();
        assert!(!columns.is_empty(), "{table} exists");
        let expr = columns
            .iter()
            .map(|c| format!("quote(\"{c}\")"))
            .collect::<Vec<_>>()
            .join(" || ',' || ");
        let order = columns
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT {expr} FROM \"{table}\" ORDER BY {order}");
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_all(&mut *c)
            .await
            .unwrap();
        out.push(format!("== {table}: {} rows", rows.len()));
        out.extend(rows.iter().map(|row| row.get::<String, _>(0)));
    }
    out
}

#[tokio::test]
async fn upgrade_from_version_3_drops_strategy() {
    const CONTENT_TABLES: [&str; 10] = [
        "weeks",
        "days",
        "meals",
        "meal_steps",
        "shopping_lists",
        "day_shopping_lists",
        "shopping_categories",
        "shopping_items",
        "prep_sessions",
        "prep_tasks",
    ];
    // A database migrated only through version 3, holding strategy cards and all other content.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let mut old = sqlx::migrate!();
    old.migrations = old
        .migrations
        .iter()
        .filter(|m| m.version <= 3)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    assert_eq!(old.migrations.len(), 3);
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
    old.run(&mut c).await.unwrap();
    for sql in [FIXTURE, WEEKLY_FIXTURE, STRATEGY_FIXTURE, HISTORY_FIXTURE] {
        sqlx::raw_sql(sql).execute(&mut c).await.unwrap();
    }
    assert_eq!(count(&mut c, "strategy_cards").await, 4);
    assert_eq!(count(&mut c, "strategy_items").await, 6);

    let weeks: Vec<(i32, u32)> = {
        use sqlx::Row;
        sqlx::query("SELECT iso_year, iso_week FROM weeks ORDER BY iso_year, iso_week")
            .fetch_all(&mut c)
            .await
            .unwrap()
            .iter()
            .map(|r| (r.get::<i32, _>(0), r.get::<u32, _>(1)))
            .collect()
    };
    assert_eq!(weeks, [(2026, 38), (2026, 41), (2026, 44)]);
    let before_rows = row_snapshot(&mut c, &CONTENT_TABLES).await;
    drop(c);

    db::migrate(&path).await.unwrap();

    let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
    use sqlx::Row;
    let tables: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(&mut c)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    assert!(!tables.iter().any(|t| t.contains("strategy")), "{tables:?}");
    assert_eq!(
        without_weekday_column(row_snapshot(&mut c, &CONTENT_TABLES).await),
        before_rows
    );

    let pool = db::open_read_pool(&path).await.unwrap();
    // The same fixtures in a database that never had strategy cards: the weeks must match it.
    let fresh = TestDb::with_fixture().await;
    sqlx::raw_sql(HISTORY_FIXTURE)
        .execute(&mut fresh.writer().await)
        .await
        .unwrap();
    let fresh_pool = fresh.reader().await;
    for (y, w) in weeks {
        let doc = week_value(&pool, y, w).await;
        assert!(doc.get("strategyCards").is_none(), "{y}-W{w}");
        assert_eq!(doc["schemaVersion"], 3, "{y}-W{w}");
        assert_eq!(doc, week_value(&fresh_pool, y, w).await, "{y}-W{w}");
    }
    let doc = week_value(&pool, 2026, 44).await;
    assert_eq!(doc["prepSessions"].as_array().unwrap().len(), 2);
    assert_eq!(doc["days"][2]["reminder"]["title"], "Shop day");
}

/// `row_snapshot` rows of a database that has `prep_sessions.weekday` (the last column), with that
/// column removed and asserted NULL, so they compare with a snapshot taken before the column existed.
fn without_weekday_column(rows: Vec<String>) -> Vec<String> {
    let mut in_sessions = false;
    rows.into_iter()
        .map(|row| {
            if row.starts_with("== ") {
                in_sessions = row.starts_with("== prep_sessions:");
                return row;
            }
            if in_sessions {
                return row
                    .strip_suffix(",NULL")
                    .unwrap_or_else(|| panic!("weekday is NULL after the upgrade: {row}"))
                    .to_string();
            }
            row
        })
        .collect()
}

/// Feature 17 upgrade: a database at migration 0005 holding all fixtures gains five empty profile
/// sections and every existing row is unchanged.
#[tokio::test]
async fn upgrade_from_version_5_adds_five_empty_profile_sections() {
    const CONTENT_TABLES: [&str; 10] = [
        "weeks",
        "days",
        "meals",
        "meal_steps",
        "shopping_lists",
        "day_shopping_lists",
        "shopping_categories",
        "shopping_items",
        "prep_sessions",
        "prep_tasks",
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let mut old = sqlx::migrate!();
    old.migrations = old
        .migrations
        .iter()
        .filter(|m| m.version <= 5)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    assert_eq!(old.migrations.len(), 5);
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
    old.run(&mut c).await.unwrap();
    for sql in [FIXTURE, WEEKLY_FIXTURE, HISTORY_FIXTURE] {
        sqlx::raw_sql(sql).execute(&mut c).await.unwrap();
    }
    let before_rows = row_snapshot(&mut c, &CONTENT_TABLES).await;
    assert!(
        sqlx::query("SELECT 1 FROM pragma_table_info('profile_sections')")
            .fetch_optional(&mut c)
            .await
            .unwrap()
            .is_none()
    );
    drop(c);

    db::migrate(&path).await.unwrap();

    let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
    assert_eq!(row_snapshot(&mut c, &CONTENT_TABLES).await, before_rows);
    assert_eq!(
        row_snapshot(&mut c, &["profile_sections"]).await,
        [
            "== profile_sections: 5 rows",
            "'food',''",
            "'household',''",
            "'kitchen',''",
            "'other',''",
            "'planning',''",
        ]
    );
    drop(c);

    // Migrating again changes nothing.
    db::migrate(&path).await.unwrap();
    let pool = db::open_read_pool(&path).await.unwrap();
    let profile = sous::profile::load_profile(&pool).await.unwrap();
    assert_eq!(profile.sections.len(), 5);
    assert!(profile.sections.iter().all(|s| s.text.is_empty()));
    let (status, _, _) = get(&pool, "/2026/44").await;
    assert_eq!(status, StatusCode::OK);
}

/// Feature 13 upgrade: a database at migration 0004 holding all fixtures gets `weekday` as NULL on
/// every session and nothing else changes.
#[tokio::test]
async fn upgrade_from_version_4_adds_a_null_prep_weekday() {
    const CONTENT_TABLES: [&str; 10] = [
        "weeks",
        "days",
        "meals",
        "meal_steps",
        "shopping_lists",
        "day_shopping_lists",
        "shopping_categories",
        "shopping_items",
        "prep_sessions",
        "prep_tasks",
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let mut old = sqlx::migrate!();
    old.migrations = old
        .migrations
        .iter()
        .filter(|m| m.version <= 4)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    assert_eq!(old.migrations.len(), 4);
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
    old.run(&mut c).await.unwrap();
    for sql in [FIXTURE, WEEKLY_FIXTURE, HISTORY_FIXTURE] {
        sqlx::raw_sql(sql).execute(&mut c).await.unwrap();
    }
    let before_rows = row_snapshot(&mut c, &CONTENT_TABLES).await;
    assert!(
        before_rows
            .iter()
            .any(|r| r.starts_with("== prep_sessions: 2 rows"))
    );
    drop(c);

    db::migrate(&path).await.unwrap();

    let mut c = SqliteConnection::connect_with(&opts).await.unwrap();
    // Every session is NULL, every other row and column is unchanged.
    assert_eq!(
        without_weekday_column(row_snapshot(&mut c, &CONTENT_TABLES).await),
        before_rows
    );
    drop(c);

    // Each week serves what a fresh database with the same files serves, weekday null everywhere.
    let pool = db::open_read_pool(&path).await.unwrap();
    let fresh = TestDb::with_fixture().await;
    sqlx::raw_sql(HISTORY_FIXTURE)
        .execute(&mut fresh.writer().await)
        .await
        .unwrap();
    let fresh_pool = fresh.reader().await;
    for (y, w) in [(2026, 38), (2026, 41), (2026, 44)] {
        let doc = week_value(&pool, y, w).await;
        assert_eq!(doc, week_value(&fresh_pool, y, w).await, "{y}-W{w}");
        assert_eq!(doc["schemaVersion"], 3, "{y}-W{w}");
        for session in doc["prepSessions"].as_array().unwrap() {
            assert_eq!(session["weekday"], Value::Null, "{y}-W{w}");
        }
    }
    assert_eq!(
        week_value(&pool, 2026, 44).await["prepSessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn weekly_content_order_and_reorder() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let doc = week_value(&pool, 2026, 44).await;
    let prep = weekly_order(&doc);
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
    assert_eq!(doc["prepSessions"][1]["covers"], Value::Null);
    assert_eq!(
        doc["prepSessions"][0]["covers"],
        "Covers Monday through Wednesday"
    );
    assert_eq!(doc["prepSessions"][0]["id"], SESSION_MON);
    let session_ids = sorted_ids(&doc, "prepSessions");

    // Reorder through a separate writer: shift every position out of the way, then set.
    let mut c = t.writer().await;
    let mut tx = c.begin().await.unwrap();
    for table in ["prep_sessions", "prep_tasks"] {
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
    let prep = weekly_order(&doc);
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
    assert_eq!(sorted_ids(&doc, "prepSessions"), session_ids);
}

#[tokio::test]
async fn weekly_constraints_reject_bad_data() {
    let t = TestDb::migrated().await;
    let mut c = t.writer().await;
    let (wa, wb) = (sid(1, 1), sid(1, 2));
    let (s1, s2, s3) = (sid(8, 1), sid(8, 2), sid(8, 3));
    let t1 = sid(9, 1);
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

    exec(&mut c, &session(&s1, &wa, 0, "'s'")).await.unwrap();
    exec(&mut c, &task(&t1, &s1, 0, "'n'", "'t'", "'i'"))
        .await
        .unwrap();

    let (n, nt) = (sid(8, 9), sid(9, 9));
    let upper = "AAAAAAAA-0000-4000-8000-000000000000";
    let bad: Vec<(String, String)> = vec![
        ("session dup position".into(), session(&n, &wa, 0, "'x'")),
        (
            "task dup position".into(),
            task(&nt, &s1, 0, "'n'", "'t'", "'i'"),
        ),
        ("session negative".into(), session(&n, &wa, -1, "'x'")),
        (
            "task negative".into(),
            task(&nt, &s1, -1, "'n'", "'t'", "'i'"),
        ),
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
            "session bad id".into(),
            session("not-a-uuid", &wa, 5, "'x'"),
        ),
        ("session upper id".into(), session(upper, &wa, 5, "'x'")),
        (
            "task bad id".into(),
            task("nope", &s1, 5, "'n'", "'t'", "'i'"),
        ),
        ("session fk".into(), session(&n, missing, 5, "'x'")),
        ("task fk".into(), task(&nt, missing, 5, "'n'", "'t'", "'i'")),
    ];
    for (what, sql) in &bad {
        assert!(
            exec(&mut c, sql).await.is_err(),
            "{what} should be rejected"
        );
    }
    // Rejected rows left nothing behind.
    assert_eq!(count(&mut c, "prep_sessions").await, 1);
    assert_eq!(count(&mut c, "prep_tasks").await, 1);

    // The same position under a different parent is fine.
    exec(&mut c, &session(&s2, &wb, 0, "'s'")).await.unwrap();
    exec(&mut c, &task(&sid(9, 2), &s2, 0, "'n'", "'t'", "'i'"))
        .await
        .unwrap();

    // Cascades: session -> tasks, week -> everything.
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
    exec(&mut c, &format!("DELETE FROM weeks WHERE id = '{wa}'"))
        .await
        .unwrap();
    assert_eq!(count(&mut c, "prep_sessions").await, 1);
    assert_eq!(count(&mut c, "prep_tasks").await, 1);
    exec(&mut c, &format!("DELETE FROM weeks WHERE id = '{wb}'"))
        .await
        .unwrap();
    for table in ["prep_sessions", "prep_tasks"] {
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
    assert!(doc.get("strategyCards").is_none());
    assert_eq!(doc["schemaVersion"], 3);
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
        "UPDATE days SET reminder_title = 'Fresh reminder' WHERE reminder_title = 'Shop day'",
    ];
    let fresh = ["Fresh intro", "Fresh task", "Fresh reminder"];

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
            "UPDATE days SET reminder_title = '{evil}', reminder_body = '{evil}', reminder_icon = '{evil}' WHERE id = '{WED}'"
        ),
    ] {
        exec(&mut c, &sql).await.unwrap();
    }
    let (s, _, body) = get(&t.reader().await, "/2026/44").await;
    assert_eq!(s, StatusCode::OK);
    assert!(!body.contains("alert(7)</script>"));
    assert!(!body.contains("<script>alert(7)"));
    // Intro, session x2, task x3, reminder x3.
    assert_eq!(
        body.matches("\\u003c/script\\u003e\\u003cscript\\u003ealert(7)")
            .count(),
        9
    );
    let page = page_json(&body);
    assert_eq!(page["document"]["week"]["prepIntro"], evil);
    assert_eq!(page["document"]["days"][2]["reminder"]["icon"], evil);
}

// ---- Feature 5: read API ----

const THU: &str = "00000002-0000-4000-8000-0000000000f1";
const THU_MEAL: &str = "00000003-0000-4000-8000-0000000000f2";

type Reply = (StatusCode, axum::http::HeaderMap, String);

/// Send `method uri` through the full router (optionally with an `Origin` header).
async fn send(pool: &SqlitePool, method: &str, uri: &str, origin: Option<&str>) -> Reply {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(origin) = origin {
        req = req.header(header::ORIGIN, origin);
    }
    let res = sous::app(pool.clone(), test_clock())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

fn api_error(code: &str, message: &str) -> Value {
    json!({"error": {"code": code, "message": message}})
}

fn week_not_found() -> Value {
    api_error("week_not_found", "No meal plan for this week yet.")
}

fn invalid_week() -> Value {
    api_error("invalid_week", "Invalid week.")
}

fn invalid_date_error() -> Value {
    api_error("invalid_date", "Invalid date.")
}

fn api_not_found() -> Value {
    api_error("not_found", "Not found.")
}

fn unavailable_error() -> Value {
    api_error(
        "service_unavailable",
        "Meal plans are temporarily unavailable.",
    )
}

/// Assert a JSON response: status, exact content type, no-store, exact body.
fn assert_json_response(res: &Reply, status: StatusCode, body: &Value, what: &str) {
    let (s, h, text) = res;
    assert_eq!(*s, status, "{what}");
    assert_eq!(h[header::CONTENT_TYPE], "application/json", "{what}");
    assert_eq!(h[header::CACHE_CONTROL], "no-store", "{what}");
    assert!(
        h.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(),
        "{what}"
    );
    assert_eq!(
        &serde_json::from_str::<Value>(text).unwrap(),
        body,
        "{what}"
    );
}

fn meal_names(doc: &Value) -> Vec<String> {
    doc["days"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|d| d["meals"].as_array().unwrap())
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect()
}

async fn api_json(pool: &SqlitePool, uri: &str) -> Value {
    let (s, _, body) = get(pool, uri).await;
    assert_eq!(s, StatusCode::OK, "{uri}");
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
async fn api_matches_page_bootstrap() {
    for t in [
        TestDb::with_fixture().await,
        TestDb::with_base_fixture().await,
    ] {
        let pool = t.reader().await;
        let (s, h, body) = get(&pool, "/api/2026/44").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "application/json");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        let api: Value = serde_json::from_str(&body).unwrap();
        let (_, _, html) = get(&pool, "/2026/44").await;
        assert_eq!(api, page_json(&html)["document"]);
    }

    // Null and empty values are explicit keys.
    let t = TestDb::with_fixture().await;
    let doc = api_json(&t.reader().await, "/api/2026/44").await;
    assert_eq!(doc["schemaVersion"], 3);
    assert!(doc.get("strategyCards").is_none());
    let thursday = doc["days"][3].as_object().unwrap();
    assert_eq!(thursday["id"], Value::Null);
    assert_eq!(thursday["reminder"], Value::Null);
    assert_eq!(thursday["meals"], json!([]));
    assert_eq!(thursday["shoppingListIds"], json!([]));

    assert!(doc["week"]["id"].is_string());
    assert_eq!(doc["week"]["startDate"], doc["days"][0]["date"]);
    assert_eq!(doc["week"]["endDate"], doc["days"][6]["date"]);
    let days = doc["days"].as_array().unwrap();
    assert_eq!(days.len(), 7);
    let monday = chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
    for (i, d) in days.iter().enumerate() {
        assert_eq!(d["weekday"], i as u64 + 1);
        let date = d["date"].as_str().unwrap();
        assert_eq!(date.len(), 10);
        assert_eq!(
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            monday + chrono::Days::new(i as u64)
        );
        assert!(d["id"].is_null() || d["id"].is_string());
    }
    for key in ["shoppingLists", "prepSessions"] {
        for item in doc[key].as_array().unwrap() {
            assert!(item["id"].is_string(), "{key}");
        }
    }

    // A week without weekly content.
    let t = TestDb::with_base_fixture().await;
    let doc = api_json(&t.reader().await, "/api/2026/44").await;
    assert_eq!(doc["prepSessions"], json!([]));
    assert_eq!(doc["week"]["prepIntro"], Value::Null);
}

#[tokio::test]
async fn api_error_contract() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let nf = StatusCode::NOT_FOUND;

    let res = get(&pool, "/api/2026/45").await;
    assert_json_response(&res, nf, &week_not_found(), "missing week");

    for uri in [
        "/api/2025/53",
        "/api/abc/1",
        "/api/2026/abc",
        "/api/0000/1",
        "/api/10000/1",
        "/api/2026/0",
        "/api/2026/54",
        "/api/%FF/1",
        "/api/2026/%FF",
        "/api/weeks/2026",
    ] {
        let res = get(&pool, uri).await;
        assert_json_response(&res, nf, &invalid_week(), uri);
    }

    for uri in [
        "/api",
        "/api/",
        "/api/nothing",
        "/api/weeks",
        "/api/weeks/",
        "/api/weeks/2026/44",
        "/api/weeks/2026/45",
        "/api/2026/",
        "/api/2026/44/extra",
        "/api/2026/44/",
    ] {
        let res = get(&pool, uri).await;
        assert_json_response(&res, nf, &api_not_found(), uri);
    }

    let (_dir, tableless) = tableless_pool().await;
    let closed = t.reader().await;
    closed.close().await;
    for (name, pool) in [("tableless", &tableless), ("closed", &closed)] {
        for uri in ["/api/2026/44", "/api/2026/45", "/api/2026-10-28"] {
            let res = get(pool, uri).await;
            assert_json_response(
                &res,
                StatusCode::SERVICE_UNAVAILABLE,
                &unavailable_error(),
                &format!("{name} {uri}"),
            );
        }
    }
}

#[tokio::test]
async fn api_zero_padded_redirects() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for (from, to) in [
        ("/api/2026/04", "/api/2026/4"),
        ("/api/2026/004", "/api/2026/4"),
        ("/api/2026/044", "/api/2026/44"),
        ("/api/2026/053", "/api/2026/53"),
        ("/api/9999/051", "/api/9999/51"),
        ("/api/0001/01", "/api/0001/1"),
    ] {
        let (s, h, _) = get(&pool, from).await;
        assert_eq!(s, StatusCode::PERMANENT_REDIRECT, "{from}");
        assert_eq!(h[header::LOCATION], to, "{from}");
        assert_eq!(h[header::CACHE_CONTROL], "no-store", "{from}");
    }
    // The redirect does not touch the database.
    let (_dir, tableless) = tableless_pool().await;
    let (s, _, _) = get(&tableless, "/api/2026/044").await;
    assert_eq!(s, StatusCode::PERMANENT_REDIRECT);
}

#[tokio::test]
async fn api_snapshot_is_atomic_under_concurrent_commit() {
    use tokio::sync::oneshot;

    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let before = week_value(&pool, 2026, 44).await;

    let (paused_tx, paused_rx) = oneshot::channel::<()>();
    let (committed_tx, committed_rx) = oneshot::channel::<()>();
    let mut writer = t.writer().await;
    let writer_task = tokio::spawn(async move {
        paused_rx.await.unwrap();
        let mut tx = writer.begin().await.unwrap();
        for sql in [
            "UPDATE weeks SET subtitle = 'Changed subtitle'".to_string(),
            "UPDATE meals SET name = 'Renamed walnuts' WHERE name = 'Walnuts'".to_string(),
            "DELETE FROM meal_steps WHERE id = '00000004-0000-4000-8000-000000000007'".to_string(),
            day_sql(THU, FIXTURE_WEEK, 4),
            meal_sql(THU_MEAL, THU, 0, "Dinner", "100"),
            "UPDATE shopping_items SET position = 20 WHERE text = '2L oat milk'".to_string(),
            format!("UPDATE prep_sessions SET title = 'Changed prep' WHERE id = '{SESSION_MON}'"),
        ] {
            exec(&mut tx, &sql).await.unwrap();
        }
        tx.commit().await.unwrap();
        committed_tx.send(()).unwrap();
    });

    // The week row is read and the snapshot is open; the writer commits before any related row is read.
    let pause = async move {
        paused_tx.send(()).unwrap();
        committed_rx.await.unwrap();
    };
    let paused = queries::load_week_with_pause(&pool, 2026, 44, pause)
        .await
        .unwrap()
        .unwrap();
    writer_task.await.unwrap();
    assert_eq!(serde_json::to_value(&paused).unwrap(), before);

    let after = week_value(&pool, 2026, 44).await;
    assert_ne!(after, before);
    assert_eq!(after["week"]["subtitle"], "Changed subtitle");
    assert!(meal_names(&before).contains(&"Walnuts".to_string()));
    let names = meal_names(&after);
    assert!(
        names.contains(&"Renamed walnuts".to_string()) && !names.contains(&"Walnuts".to_string())
    );
    let walnuts = after["days"][0]["meals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "Renamed walnuts")
        .unwrap();
    assert_eq!(walnuts["steps"], json!([]));
    assert_eq!(after["days"][3]["meals"][0]["id"], THU_MEAL);
    let texts = |doc: &Value| -> Vec<String> {
        let category = doc["shoppingLists"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|l| l["categories"].as_array().unwrap())
            .find(|c| c["name"] == "Dairy / Chilled")
            .unwrap();
        category["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["text"].as_str().unwrap().to_string())
            .collect()
    };
    assert_ne!(texts(&before), texts(&after));
    assert_eq!(texts(&after).last().unwrap(), "2L oat milk");
    let session = |doc: &Value| -> String {
        doc["prepSessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == SESSION_MON)
            .unwrap()["title"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(session(&before), "Monday Prep Session (~1.5 hours)");
    assert_eq!(session(&after), "Changed prep");
}

#[tokio::test]
async fn api_freshness_after_commit() {
    let t = TestDb::with_fixture().await;
    let app = sous::app(t.reader().await, test_clock());
    let fetch = || async {
        let res = app
            .clone()
            .oneshot(Request::get("/api/2026/44").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&body).unwrap()
    };

    let mut c = t.writer().await;
    let mut tx = c.begin().await.unwrap();
    exec(
        &mut tx,
        "UPDATE meals SET name = 'Renamed meal' WHERE name = 'Walnuts'",
    )
    .await
    .unwrap();
    assert!(meal_names(&fetch().await).contains(&"Walnuts".to_string()));
    tx.commit().await.unwrap();
    let names = meal_names(&fetch().await);
    assert!(names.contains(&"Renamed meal".to_string()) && !names.contains(&"Walnuts".to_string()));
}

#[tokio::test]
async fn api_mutations_405_and_no_leaks() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for uri in ["/api/2026/44", "/api/2026/45", "/api/abc/1", "/api/2026"] {
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            let (s, h, _) = send(&pool, method, uri, None).await;
            assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{method} {uri}");
            assert_eq!(h[header::CACHE_CONTROL], "no-store", "{method} {uri}");
            if uri == "/api/2026" {
                assert_eq!(h[header::ALLOW], "GET,HEAD", "{method} {uri}");
            }
        }
    }
    // Not a year: the JSON not-found response, whatever the method.
    for uri in ["/api/nothing", "/api/weeks"] {
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            let res = send(&pool, method, uri, None).await;
            assert_json_response(&res, StatusCode::NOT_FOUND, &api_not_found(), uri);
        }
    }

    // No cross-origin access.
    let origin = Some("https://example.com");
    for uri in [
        "/api/2026/44",
        "/api/2026/45",
        "/api/2026",
        "/api/nothing",
        "/2026/44",
    ] {
        let (_, h, _) = send(&pool, "GET", uri, origin).await;
        assert!(
            h.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(),
            "{uri}"
        );
    }
    let (_, h, _) = send(&pool, "OPTIONS", "/api/2026/44", origin).await;
    assert!(h.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());

    // Database errors leak nothing.
    let (dir, tableless) = tableless_pool().await;
    let tableless_dir = dir.path().to_str().unwrap().to_string();
    let closed = t.reader().await;
    closed.close().await;
    let fixture_dir = t._dir.path().to_str().unwrap().to_string();
    for pool in [&tableless, &closed] {
        for uri in ["/api/2026/44", "/api/2026", "/2026/44", "/2026", "/"] {
            let (s, _, body) = get(pool, uri).await;
            assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            for needle in [
                "sqlite",
                "SQL",
                "no such table",
                "pool",
                ".db",
                "migration",
                tableless_dir.as_str(),
                fixture_dir.as_str(),
            ] {
                assert!(!body.contains(needle), "{uri} leaks {needle}");
            }
        }
    }
}

/// Body of a `YearDocument` as the API serializes it.
fn year_doc(year: i64, previous: Value, next: Value, weeks: Vec<Value>) -> Value {
    json!({
        "schemaVersion": 1,
        "year": year,
        "previousYear": previous,
        "nextYear": next,
        "weeks": weeks,
    })
}

fn year_not_found() -> Value {
    api_error("year_not_found", "No meal plans available yet.")
}

/// `/api/<year>` against the bootstrap of `/<year>`: same status class, same data.
async fn assert_year_api_matches_page(pool: &SqlitePool, year: i32) {
    let page_uri = format!("/{year:04}");
    let api_uri = format!("/api/{year:04}");
    let (page_status, _, page_body) = get(pool, &page_uri).await;
    let api = get(pool, &api_uri).await;
    if page_status == StatusCode::NOT_FOUND {
        assert_json_response(&api, StatusCode::NOT_FOUND, &year_not_found(), &api_uri);
        return;
    }
    assert_eq!(page_status, StatusCode::OK, "{page_uri}");
    let page = page_json(&page_body);
    let (s, h, body) = &api;
    assert_eq!(*s, StatusCode::OK, "{api_uri}");
    assert_eq!(h[header::CONTENT_TYPE], "application/json", "{api_uri}");
    assert_eq!(h[header::CACHE_CONTROL], "no-store", "{api_uri}");
    assert!(
        body.starts_with("{\"schemaVersion\":1,\"year\":"),
        "{api_uri}: {body}"
    );
    let doc: Value = serde_json::from_str(body).unwrap();
    let mut keys: Vec<&str> = doc
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["nextYear", "previousYear", "schemaVersion", "weeks", "year"],
        "{api_uri}"
    );
    assert_eq!(doc["schemaVersion"], 1, "{api_uri}");
    for key in ["year", "previousYear", "nextYear", "weeks"] {
        assert_eq!(doc[key], page[key], "{api_uri} {key}");
    }
}

#[tokio::test]
async fn api_year_matches_year_page_bootstrap() {
    let t = TestDb::with_fixture().await;
    insert_weeks(
        &t,
        &[
            (2027, 1),
            (2025, 52),
            (2026, 53),
            (2026, 1),
            (2026, 49),
            (2020, 53),
        ],
    )
    .await;
    let pool = t.reader().await;
    for year in [2020, 2025, 2026, 2027, 2019, 2024, 2028] {
        assert_year_api_matches_page(&pool, year).await;
    }
    let api = api_json(&pool, "/api/2026").await;
    assert_eq!(api["weeks"].as_array().unwrap().len(), 4);
    assert_eq!(
        api["weeks"][0],
        summary(2026, 53, "2026-12-28", "2027-01-03", "/2026/53")
    );
    assert_eq!(
        api["weeks"][3],
        summary(2026, 1, "2025-12-29", "2026-01-04", "/2026/1")
    );
    assert_eq!(api["previousYear"], 2025);
    assert_eq!(api["nextYear"], 2027);

    // Gap years: 2025 has no plans, so 2026 links to nothing and 2025 is a 404.
    let t = TestDb::migrated().await;
    insert_weeks(&t, &[(2023, 5), (2024, 5), (2026, 5)]).await;
    let pool = t.reader().await;
    for year in [2022, 2023, 2024, 2025, 2026, 2027] {
        assert_year_api_matches_page(&pool, year).await;
    }
    let api = api_json(&pool, "/api/2024").await;
    assert_eq!(
        (&api["previousYear"], &api["nextYear"]),
        (&json!(2023), &json!(null))
    );
    let (s, _, _) = get(&pool, "/api/2025").await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // First and last supported years.
    let t = TestDb::migrated().await;
    insert_weeks(&t, &[(1, 1), (9999, 51), (9999, 1)]).await;
    let pool = t.reader().await;
    for year in [1, 2, 9998, 9999] {
        assert_year_api_matches_page(&pool, year).await;
    }
    let api = api_json(&pool, "/api/0001").await;
    assert_eq!(
        api["weeks"],
        json!([summary(1, 1, "0001-01-01", "0001-01-07", "/0001/1")])
    );
    assert_eq!(api["year"], 1);
}

#[tokio::test]
async fn api_year_contract() {
    let ok = StatusCode::OK;
    let nf = StatusCode::NOT_FOUND;
    let week44 = summary(2026, 44, "2026-10-26", "2026-11-01", "/2026/44");

    // A year with plans.
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let res = get(&pool, "/api/2026").await;
    let expected = year_doc(2026, json!(null), json!(null), vec![week44]);
    assert_json_response(&res, ok, &expected, "year with plans");

    // The current year without plans is 200, in an empty and in a non-empty database.
    let empty = TestDb::migrated().await;
    let empty_pool = empty.reader().await;
    let current_empty = year_doc(2026, json!(null), json!(null), vec![]);
    let res = get(&empty_pool, "/api/2026").await;
    assert_json_response(&res, ok, &current_empty, "current year, empty database");
    let res = get(&empty_pool, "/api/2025").await;
    assert_json_response(&res, nf, &year_not_found(), "other year, empty database");

    let other = TestDb::migrated().await;
    insert_weeks(&other, &[(2024, 10)]).await;
    let other_pool = other.reader().await;
    let res = get(&other_pool, "/api/2026").await;
    assert_json_response(&res, ok, &current_empty, "current year, no plans");
    let res = get(&other_pool, "/api/2025").await;
    assert_json_response(&res, nf, &year_not_found(), "other year, no plans");

    // Not a year: not_found without touching the database.
    let (_dir, tableless) = tableless_pool().await;
    let closed = t.reader().await;
    closed.close().await;
    for segment in [
        "weeks", "0000", "10000", "nothing", "26", "02026", "+026", "%FF", "%202026",
    ] {
        let uri = format!("/api/{segment}");
        for (name, pool) in [
            ("good", &pool),
            ("tableless", &tableless),
            ("closed", &closed),
        ] {
            let res = get(pool, &uri).await;
            assert_json_response(&res, nf, &api_not_found(), &format!("{name} {uri}"));
        }
    }
    for uri in ["/api/weeks/2026/44", "/api/weeks", "/api/nothing"] {
        let res = get(&pool, uri).await;
        assert_json_response(&res, nf, &api_not_found(), uri);
    }

    // Database failure, for the current year and another one.
    for (name, pool) in [("tableless", &tableless), ("closed", &closed)] {
        for uri in ["/api/2026", "/api/2025"] {
            let res = get(pool, uri).await;
            assert_json_response(
                &res,
                StatusCode::SERVICE_UNAVAILABLE,
                &unavailable_error(),
                &format!("{name} {uri}"),
            );
        }
    }
}

#[tokio::test]
async fn api_year_current_year_around_new_year() {
    use chrono::TimeZone;
    let t = TestDb::migrated().await;
    let pool = t.reader().await;
    let check = |year: i32, current: i32, res: &Reply, what: &str| {
        if year == current {
            let body = year_doc(year as i64, json!(null), json!(null), vec![]);
            assert_json_response(res, StatusCode::OK, &body, what);
        } else {
            assert_json_response(res, StatusCode::NOT_FOUND, &year_not_found(), what);
        }
    };

    for (clock, current) in [
        (berlin(2025, 12, 28, 22, 59, 59), 2025),
        (berlin(2025, 12, 28, 23, 0, 0), 2026),
        (berlin(2027, 1, 3, 12, 0, 0), 2026),
        (berlin(2027, 1, 4, 12, 0, 0), 2027),
    ] {
        for year in [2025, 2026, 2027] {
            let res = get_at(&pool, clock.clone(), &format!("/api/{year}")).await;
            check(
                year,
                current,
                &res,
                &format!("/api/{year}, current {current}"),
            );
        }
    }

    let instant = chrono::Utc
        .with_ymd_and_hms(2025, 12, 29, 0, 30, 0)
        .unwrap();
    for (tz, current) in [
        (sous::calendar::DEFAULT_TIMEZONE, 2026),
        (chrono_tz::America::New_York, 2025),
    ] {
        for year in [2025, 2026] {
            let res = get_at(&pool, Clock::fixed(tz, instant), &format!("/api/{year}")).await;
            check(year, current, &res, &format!("/api/{year} in {tz}"));
        }
    }
}

#[tokio::test]
async fn api_year_is_fresh() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let paths = |doc: &Value| -> Vec<String> {
        doc["weeks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["path"].as_str().unwrap().to_string())
            .collect()
    };

    assert_eq!(paths(&api_json(&pool, "/api/2026").await), ["/2026/44"]);

    // An open transaction's insert is invisible; once it ends the week shows up.
    let mut c = t.writer().await;
    let mut tx = c.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(week_sql(&uuid(7), 2026, 50)))
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(paths(&api_json(&pool, "/api/2026").await), ["/2026/44"]);
    tx.commit().await.unwrap();
    let doc = api_json(&pool, "/api/2026").await;
    assert_eq!(paths(&doc), ["/2026/50", "/2026/44"]);

    // The first plan of the next year makes `nextYear` appear; deleting it removes it.
    assert_eq!(doc["nextYear"], json!(null));
    exec(&mut c, &week_sql(&uuid(8), 2027, 1)).await.unwrap();
    assert_eq!(api_json(&pool, "/api/2026").await["nextYear"], 2027);
    exec(&mut c, "DELETE FROM weeks WHERE iso_year = 2027")
        .await
        .unwrap();
    assert_eq!(api_json(&pool, "/api/2026").await["nextYear"], json!(null));

    // Deleted weeks disappear.
    exec(&mut c, "DELETE FROM weeks WHERE iso_week = 50")
        .await
        .unwrap();
    assert_eq!(paths(&api_json(&pool, "/api/2026").await), ["/2026/44"]);
}

#[tokio::test]
async fn missing_assets_are_plain_404() {
    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    for uri in ["/assets/nope.js", "/assets/app.js", "/assets/"] {
        let (s, _, body) = get(&pool, uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert!(!body.contains("sous-page"), "{uri}");
    }
}

const OPENAPI: &str = include_str!("../docs/openapi.json");

/// Validate `value` against an OpenAPI schema (a small subset checker: refs, type, enum, const,
/// properties/required/additionalProperties, items).
fn check_schema(root: &Value, schema: &Value, value: &Value, at: &str) {
    if let Some(r) = schema.get("$ref") {
        let name = r
            .as_str()
            .unwrap()
            .strip_prefix("#/components/schemas/")
            .expect("local schema ref");
        return check_schema(root, &root["components"]["schemas"][name], value, at);
    }
    if let Some(branches) = schema.get("oneOf") {
        // A nullable `$ref`: null selects the `null` branch, anything else the other.
        // Two documents: the branch whose `schemaVersion` const equals the value's.
        let branches = branches.as_array().unwrap();
        let resolve = |b: &'_ Value| -> Value {
            match b.get("$ref").and_then(Value::as_str) {
                Some(r) => root["components"]["schemas"]
                    [r.strip_prefix("#/components/schemas/").unwrap()]
                .clone(),
                None => b.clone(),
            }
        };
        let versioned = |b: &Value| {
            let schema = resolve(b);
            let v = &schema["properties"]["schemaVersion"]["const"];
            !v.is_null() && *v == value["schemaVersion"]
        };
        let branch = if branches.iter().any(|b| b["type"] == "null") {
            branches
                .iter()
                .find(|b| (b["type"] == "null") == value.is_null())
        } else {
            let matching: Vec<_> = branches.iter().filter(|b| versioned(b)).collect();
            assert!(matching.len() <= 1, "{at}: ambiguous oneOf");
            matching.into_iter().next()
        }
        .unwrap_or_else(|| panic!("{at}: no oneOf branch"));
        return check_schema(root, branch, value, at);
    }
    if let Some(c) = schema.get("const") {
        assert_eq!(value, c, "{at}: const");
    }
    if let Some(e) = schema.get("enum") {
        assert!(e.as_array().unwrap().contains(value), "{at}: enum {value}");
    }
    let kind = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::String(_) => "string",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let types: Vec<&str> = match &schema["type"] {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().map(|t| t.as_str().unwrap()).collect(),
        other => panic!("{at}: schema without type: {other}"),
    };
    assert!(
        types.contains(&kind) || (kind == "integer" && types.contains(&"number")),
        "{at}: {kind} is not one of {types:?}"
    );
    match value {
        Value::Object(map) => {
            let props = schema["properties"].as_object().expect("properties");
            assert_eq!(schema["additionalProperties"], false, "{at}");
            let mut declared: Vec<&str> = props.keys().map(String::as_str).collect();
            let mut required: Vec<&str> = schema["required"]
                .as_array()
                .expect("required")
                .iter()
                .map(|r| r.as_str().unwrap())
                .collect();
            let mut actual: Vec<&str> = map.keys().map(String::as_str).collect();
            declared.sort();
            required.sort();
            actual.sort();
            assert_eq!(declared, required, "{at}: properties vs required");
            assert_eq!(actual, declared, "{at}: keys vs properties");
            for (k, v) in map {
                check_schema(root, &props[k], v, &format!("{at}.{k}"));
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                check_schema(root, &schema["items"], v, &format!("{at}[{i}]"));
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn openapi_matches_api() {
    let spec: Value = serde_json::from_str(OPENAPI).unwrap();
    assert_eq!(spec["openapi"], "3.1.0");
    let paths = spec["paths"].as_object().unwrap();
    assert_eq!(paths.len(), 4);
    assert!(paths.keys().all(|p| !p.contains("/weeks")));
    assert!(!OPENAPI.contains("/api/weeks"));
    let responses = &spec["paths"]["/api/{year}/{week}"]["get"]["responses"];
    assert_eq!(
        spec["paths"]["/api/{year}/{week}"]["get"]["operationId"],
        "getWeek"
    );
    assert_eq!(
        spec["paths"]["/api/{segment}"]["get"]["operationId"],
        "getYearOrDate"
    );
    assert!(!OPENAPI.contains("getYear\"") && !OPENAPI.contains("/api/{year}\""));
    let year_responses = &spec["paths"]["/api/{segment}"]["get"]["responses"];

    let t = TestDb::with_fixture().await;
    let pool = t.reader().await;
    let live = api_json(&pool, "/api/2026/44").await;

    // (a) The documented example is the live output.
    let content = &responses["200"]["content"]["application/json"];
    assert_eq!(content["example"], live);
    // (b) The live output satisfies the schema, with and without weekly content.
    check_schema(&spec, &content["schema"], &live, "$");
    let bare = TestDb::with_base_fixture().await;
    let live_bare = api_json(&bare.reader().await, "/api/2026/44").await;
    check_schema(&spec, &content["schema"], &live_bare, "$bare");

    // The year route: the example is the live output with a plan in the previous year.
    let year_t = TestDb::with_fixture().await;
    insert_weeks(&year_t, &[(2025, 52)]).await;
    let year_pool = year_t.reader().await;
    let live_year = api_json(&year_pool, "/api/2026").await;
    let year_content = &year_responses["200"]["content"]["application/json"];
    assert_eq!(year_content["examples"]["year"]["value"], live_year);
    check_schema(&spec, &year_content["schema"], &live_year, "$year");
    // The date form returns the week document, which satisfies the same `oneOf` (second branch).
    let live_date = api_json(&pool, "/api/2026-10-28").await;
    assert_eq!(live_date, live);
    check_schema(&spec, &year_content["schema"], &live_date, "$date");
    let live_date_bare = api_json(&bare.reader().await, "/api/2026-10-28").await;
    check_schema(
        &spec,
        &year_content["schema"],
        &live_date_bare,
        "$date-bare",
    );
    // The year document must not satisfy the week branch by accident.
    assert_ne!(live_year["schemaVersion"], live_date["schemaVersion"]);
    let empty = TestDb::migrated().await;
    let live_empty = api_json(&empty.reader().await, "/api/2026").await;
    assert_eq!(live_empty["weeks"], json!([]));
    check_schema(&spec, &year_content["schema"], &live_empty, "$year-empty");

    let schemas = spec["components"]["schemas"].as_object().unwrap();
    for name in [
        "WeekDocument",
        "Week",
        "Day",
        "Reminder",
        "Meal",
        "Step",
        "ShoppingList",
        "ShoppingCategory",
        "ShoppingItem",
        "PrepSession",
        "PrepTask",
        "YearDocument",
        "WeekSummary",
        "Error",
        "Section",
        "Profile",
        "RevisionConflict",
    ] {
        assert!(schemas.contains_key(name), "{name}");
    }
    for name in ["StrategyCard", "StrategyItem"] {
        assert!(!schemas.contains_key(name), "{name}");
    }
    assert_eq!(
        schemas["WeekDocument"]["properties"]["schemaVersion"]["const"],
        3
    );
    assert_eq!(
        schemas["YearDocument"]["properties"]["schemaVersion"]["const"],
        1
    );

    // (c) Every error example is what the server returns, and each route has no others.
    let (_dir, tableless) = tableless_pool().await;
    let week_cases = [
        ("week_not_found", &pool, "/api/2026/45"),
        ("invalid_week", &pool, "/api/2025/53"),
        ("service_unavailable", &tableless, "/api/2026/44"),
    ];
    let year_cases = [
        ("year_not_found", &pool, "/api/2025"),
        ("week_not_found", &pool, "/api/2026-11-02"),
        ("invalid_date", &pool, "/api/2026-02-30"),
        ("not_found", &pool, "/api/nothing"),
        ("service_unavailable", &tableless, "/api/2026"),
    ];
    for (route_responses, cases) in [
        (responses, &week_cases[..]),
        (year_responses, &year_cases[..]),
    ] {
        for (code, pool, uri) in cases {
            let (s, _, body) = get(pool, uri).await;
            let body: Value = serde_json::from_str(&body).unwrap();
            let media = &route_responses[s.as_u16().to_string()]["content"]["application/json"];
            let example = media["examples"]
                .as_object()
                .and_then(|e| e.values().find(|e| e["value"]["error"]["code"] == *code))
                .unwrap_or_else(|| panic!("no example for {code}"));
            assert_eq!(example["value"], body, "{code}");
            check_schema(&spec, &media["schema"], &body, code);
        }
        let documented: usize = ["404", "503"]
            .iter()
            .map(|s| {
                route_responses[*s]["content"]["application/json"]["examples"]
                    .as_object()
                    .unwrap()
                    .len()
            })
            .sum();
        assert_eq!(documented, cases.len());
    }
}
