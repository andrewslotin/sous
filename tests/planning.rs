//! Feature 6 data layer: write pool, revisions, paging, search, recall, shopping lists and `save_week`.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sous::calendar::{Clock, week_dates};
use sous::model::{RevisedWeek, WeekInput};
use sous::queries::{self, Conflict, PageRequest, QueryError};
use sous::{db, save};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, SqliteConnection, SqlitePool};
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;

const FIXTURE: &str = include_str!("../fixtures/2026-w44.sql");
const WEEKLY_FIXTURE: &str = include_str!("../fixtures/2026-w44-weekly.sql");
const HISTORY_FIXTURE: &str = include_str!("../fixtures/2026-history.sql");

const TABLES: [&str; 12] = [
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
    "strategy_cards",
    "strategy_items",
];

const W44_ID: &str = "00000001-0000-4000-8000-000000000001";

// ---- harness ----

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

    /// Migrated, with the W44 fixtures and the history fixture.
    async fn full() -> Self {
        let t = Self::migrated().await;
        let mut c = t.writer().await;
        for sql in [FIXTURE, WEEKLY_FIXTURE, HISTORY_FIXTURE] {
            sqlx::raw_sql(sql).execute(&mut c).await.unwrap();
        }
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

    async fn write_pool(&self) -> SqlitePool {
        db::open_write_pool(&self.path).await.unwrap()
    }

    async fn exec(&self, sql: &str) {
        let mut c = self.writer().await;
        sqlx::query(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(&mut c)
            .await
            .unwrap();
    }
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

/// Every row of every content table, as text, for byte-for-byte comparisons.
async fn dump(pool: &SqlitePool) -> String {
    let mut out = String::new();
    for table in TABLES {
        let columns: Vec<String> = sqlx::query_scalar::<sqlx::Sqlite, String>(sqlx::AssertSqlSafe(
            format!("SELECT name FROM pragma_table_info('{table}')"),
        ))
        .fetch_all(pool)
        .await
        .unwrap();
        let object = columns
            .iter()
            .map(|c| format!("'{c}', \"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let rows: String =
            sqlx::query_scalar::<sqlx::Sqlite, String>(sqlx::AssertSqlSafe(format!(
                "SELECT json_group_array(json_object({object})) FROM \
             (SELECT * FROM {table} ORDER BY 1, 2)"
            )))
            .fetch_one(pool)
            .await
            .unwrap();
        out.push_str(&format!("{table}: {rows}\n"));
    }
    out
}

async fn counts(pool: &SqlitePool) -> Vec<i64> {
    let mut out = Vec::new();
    for table in TABLES {
        out.push(
            sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table}"
            )))
            .fetch_one(pool)
            .await
            .unwrap(),
        );
    }
    out
}

async fn exists(pool: &SqlitePool, table: &str, id: &str) -> bool {
    sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {table} WHERE id = '{id}'"
    )))
    .fetch_one(pool)
    .await
    .unwrap()
        > 0
}

fn code<T: std::fmt::Debug>(result: Result<T, QueryError>) -> &'static str {
    result.expect_err("expected an error").code()
}

fn conflict<T: std::fmt::Debug>(result: Result<T, QueryError>) -> Conflict {
    match result.expect_err("expected an error") {
        QueryError::RevisionConflict(conflict) => conflict,
        other => panic!("expected a revision conflict, got {other:?}"),
    }
}

fn ids<T>(items: &[T], id: impl Fn(&T) -> &str) -> Vec<String> {
    items.iter().map(|i| id(i).to_string()).collect()
}

fn d(y: i32, m: u32, day: u32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

fn page(limit: Option<u32>, cursor: Option<&str>) -> PageRequest {
    PageRequest {
        limit,
        cursor: cursor.map(str::to_string),
    }
}

fn meal(n: u32) -> String {
    format!("000000c3-0000-4000-8000-{n:012}")
}

fn berlin_clock() -> Clock {
    use chrono::TimeZone;
    Clock::fixed(
        sous::calendar::DEFAULT_TIMEZONE,
        chrono::Utc.with_ymd_and_hms(2026, 10, 26, 9, 0, 0).unwrap(),
    )
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

// ---- sample documents ----

fn uid(kind: u32, n: u32) -> String {
    format!("{kind:08x}-0000-4000-8000-{n:012x}")
}

fn step(n: u32, text: &str) -> Value {
    json!({"id": uid(0xa3, n), "text": text})
}

fn sample_meal(n: u32, kind: &str, name: &str, steps: Vec<Value>) -> Value {
    json!({
        "id": uid(0xa2, n), "type": kind, "name": name, "description": format!("{name} for two"),
        "kcal": 450.5, "proteinG": 30.0, "fatG": 12.25, "carbsG": 40.0, "fiberG": 6.0,
        "forTwo": true, "assemblyTime": "10 min", "cookTime": null, "tip": "Season well.",
        "steps": steps,
    })
}

/// A complete week document touching every field.
fn sample() -> Value {
    json!({
        "week": {"id": uid(0xa0, 1), "subtitle": "Cozy week", "prepIntro": null},
        "days": [
            {
                "id": uid(0xa1, 1), "weekday": 1,
                "meals": [
                    sample_meal(1, "Breakfast", "Oats", vec![step(1, "Stir"), step(2, "Wait")]),
                    sample_meal(2, "Dinner", "Soup", vec![step(3, "Simmer")]),
                ],
                "shoppingListIds": [uid(0xa4, 2), uid(0xa4, 1)],
                "reminder": {"title": "Shop", "body": null, "icon": "🧾"},
            },
            {
                "id": uid(0xa1, 2), "weekday": 3,
                "meals": [sample_meal(3, "Lunch", "Salad", vec![])],
                "shoppingListIds": [uid(0xa4, 1)],
                "reminder": null,
            },
        ],
        "shoppingLists": [
            {
                "id": uid(0xa4, 1), "label": "Weekly", "note": "Be quick",
                "categories": [
                    {"id": uid(0xa5, 1), "name": "Produce", "items": [
                        {"id": uid(0xa6, 1), "text": "Carrots"},
                        {"id": uid(0xa6, 2), "text": "Leeks"},
                    ]},
                    {"id": uid(0xa5, 2), "name": "Dairy", "items": []},
                ],
            },
            {"id": uid(0xa4, 2), "label": "Empty", "note": null, "categories": []},
        ],
        "prepSessions": [{
            "id": uid(0xa7, 1), "title": "Sunday prep", "covers": null,
            "tasks": [
                {"id": uid(0xa8, 1), "name": "Chop", "time": "10 min", "instructions": "Chop all"},
                {"id": uid(0xa8, 2), "name": "Cook", "time": "20 min", "instructions": "Cook all"},
            ],
        }],
        "strategyCards": [{
            "id": uid(0xa9, 1), "title": "Light", "icon": "🥣", "theme": "blue",
            "items": [{"id": uid(0xaa, 1), "text": "Soup first"}, {"id": uid(0xaa, 2), "text": "Walk"}],
        }],
    })
}

fn input(value: Value) -> WeekInput {
    serde_json::from_value(value).expect("valid week input")
}

fn edited(edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = sample();
    edit(&mut value);
    value
}

/// The document `load_week` should return for a saved input.
fn expected_document(input: &Value, iso_year: i32, iso_week: u32) -> Value {
    let dates = week_dates(iso_year, iso_week).unwrap();
    let date = |i: usize| dates[i].format("%Y-%m-%d").to_string();
    let days: Vec<Value> = (1..=7usize)
        .map(|weekday| {
            let record = input["days"]
                .as_array()
                .unwrap()
                .iter()
                .find(|day| day["weekday"] == weekday);
            match record {
                Some(day) => json!({
                    "id": day["id"], "date": date(weekday - 1), "weekday": weekday,
                    "meals": day["meals"], "shoppingListIds": day["shoppingListIds"],
                    "reminder": day["reminder"],
                }),
                None => json!({
                    "id": null, "date": date(weekday - 1), "weekday": weekday,
                    "meals": [], "shoppingListIds": [], "reminder": null,
                }),
            }
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "week": {
            "id": input["week"]["id"], "isoYear": iso_year, "isoWeek": iso_week,
            "startDate": date(0), "endDate": date(6),
            "subtitle": input["week"]["subtitle"], "prepIntro": input["week"]["prepIntro"],
        },
        "days": days,
        "shoppingLists": input["shoppingLists"],
        "prepSessions": input["prepSessions"],
        "strategyCards": input["strategyCards"],
    })
}

/// Row counts that `value` adds to each table of `TABLES` (except `weeks`: one).
fn input_counts(value: &Value) -> Vec<i64> {
    let arr = |v: &Value| v.as_array().unwrap().clone();
    let days = arr(&value["days"]);
    let meals: Vec<Value> = days.iter().flat_map(|d| arr(&d["meals"])).collect();
    let steps = meals
        .iter()
        .map(|m| arr(&m["steps"]).len() as i64)
        .sum::<i64>();
    let lists = arr(&value["shoppingLists"]);
    let cats: Vec<Value> = lists.iter().flat_map(|l| arr(&l["categories"])).collect();
    let items = cats
        .iter()
        .map(|c| arr(&c["items"]).len() as i64)
        .sum::<i64>();
    let links = days
        .iter()
        .map(|d| arr(&d["shoppingListIds"]).len() as i64)
        .sum::<i64>();
    let sessions = arr(&value["prepSessions"]);
    let tasks = sessions
        .iter()
        .map(|s| arr(&s["tasks"]).len() as i64)
        .sum::<i64>();
    let cards = arr(&value["strategyCards"]);
    let card_items = cards
        .iter()
        .map(|c| arr(&c["items"]).len() as i64)
        .sum::<i64>();
    vec![
        1,
        days.len() as i64,
        meals.len() as i64,
        steps,
        lists.len() as i64,
        links,
        cats.len() as i64,
        items,
        sessions.len() as i64,
        tasks,
        cards.len() as i64,
        card_items,
    ]
}

fn add(a: &[i64], b: &[i64]) -> Vec<i64> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

async fn save_w45(
    wp: &SqlitePool,
    expected: Option<&str>,
    value: Value,
) -> Result<RevisedWeek, QueryError> {
    save::save_week(wp, 2026, 45, expected, input(value)).await
}

// ---- tests: calendar and pools ----

#[tokio::test]
async fn write_pool_writes_and_read_pool_rejects() {
    let t = TestDb::migrated().await;
    let wp = t.write_pool().await;
    sqlx::query("INSERT INTO weeks (id, iso_year, iso_week) VALUES (?, 2026, 10)")
        .bind(uid(0xb0, 1))
        .execute(&wp)
        .await
        .unwrap();
    // Foreign keys are enforced on the write pool.
    let orphan = sqlx::query("INSERT INTO days (id, week_id, weekday) VALUES (?, ?, 1)")
        .bind(uid(0xb1, 1))
        .bind(uid(0xb0, 99))
        .execute(&wp)
        .await;
    assert!(orphan.is_err());
    // A missing database is not created by the write pool.
    let missing = t.path.with_file_name("missing.db");
    assert!(db::open_write_pool(&missing).await.is_err());

    let rp = t.reader().await;
    assert!(
        queries::load_week(&rp, 2026, 10).await.unwrap().is_some(),
        "reader sees the committed write"
    );
    let rejected = sqlx::query("INSERT INTO weeks (id, iso_year, iso_week) VALUES (?, 2026, 11)")
        .bind(uid(0xb0, 2))
        .execute(&rp)
        .await;
    assert!(rejected.is_err(), "read pool must not write");
}

// ---- tests: revision and get_week ----

#[tokio::test]
async fn revision_is_stable_and_hex() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let a = queries::get_week(&rp, 2026, 44).await.unwrap();
    let b = queries::get_week(&rp, 2026, 44).await.unwrap();
    assert_eq!(a.revision, b.revision);
    assert_eq!(a.revision.len(), 64);
    assert!(
        a.revision
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    );
    let other = queries::get_week(&rp, 2026, 41).await.unwrap();
    assert_ne!(other.revision, a.revision);
}

/// An external edit through a separate connection must make a stale `save_week` conflict and leave
/// the database untouched, even when the edit is invisible in the week document.
async fn assert_external_edit_conflicts(edit_sql: String) {
    let t = TestDb::migrated().await;
    let wp = t.write_pool().await;
    save_w45(&wp, None, sample()).await.unwrap();
    let rp = t.reader().await;
    let read = queries::get_week(&rp, 2026, 45).await.unwrap();
    let again = queries::get_week(&rp, 2026, 45).await.unwrap();
    assert_eq!(read.revision, again.revision);

    t.exec(&edit_sql).await;
    let before = dump(&rp).await;
    let stale = save_w45(
        &wp,
        Some(&read.revision),
        edited(|v| v["week"]["subtitle"] = json!("x")),
    )
    .await;
    assert_eq!(conflict(stale), Conflict::Stale);
    assert_eq!(
        dump(&rp).await,
        before,
        "conflicting save changed the database"
    );
    let after = queries::get_week(&rp, 2026, 45).await.unwrap();
    assert_ne!(after.revision, read.revision);
}

#[tokio::test]
async fn external_position_only_edit_is_a_revision_conflict() {
    assert_external_edit_conflicts(format!(
        "UPDATE meals SET position = 17 WHERE id = '{}'",
        uid(0xa2, 3)
    ))
    .await;
}

#[tokio::test]
async fn external_hidden_reminder_body_edit_is_a_revision_conflict() {
    assert_external_edit_conflicts(format!(
        "UPDATE days SET reminder_body = 'hidden' WHERE id = '{}' AND reminder_title IS NULL",
        uid(0xa1, 2)
    ))
    .await;
}

#[tokio::test]
async fn revision_changes_after_every_kind_of_edit() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let rev = |y, w| {
        let rp = rp.clone();
        async move { queries::get_week(&rp, y, w).await.unwrap().revision }
    };

    let mut w41 = vec![rev(2026, 41).await];
    t.exec(&format!(
        "UPDATE meals SET name = 'Edited casserole' WHERE id = '{}'",
        meal(1)
    ))
    .await;
    w41.push(rev(2026, 41).await);
    t.exec("UPDATE meal_steps SET position = 40 WHERE id = '000000c4-0000-4000-8000-000000000001'")
        .await;
    w41.push(rev(2026, 41).await);

    let mut w44 = vec![rev(2026, 44).await];
    t.exec(
        "UPDATE day_shopping_lists SET position = 20 \
         WHERE shopping_list_id = '00000005-0000-4000-8000-000000000020'",
    )
    .await;
    w44.push(rev(2026, 44).await);
    t.exec(
        "UPDATE days SET reminder_title = 'Changed' \
         WHERE id = '00000002-0000-4000-8000-000000000004'",
    )
    .await;
    w44.push(rev(2026, 44).await);
    t.exec(
        "UPDATE shopping_items SET text = 'Changed text' \
         WHERE id = '00000007-0000-4000-8000-000000000028'",
    )
    .await;
    w44.push(rev(2026, 44).await);

    for revisions in [w41, w44] {
        let unique: std::collections::HashSet<_> = revisions.iter().collect();
        assert_eq!(unique.len(), revisions.len(), "{revisions:?}");
    }
}

#[tokio::test]
async fn get_week_found_missing_and_unrepresentable() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let got = queries::get_week(&rp, 2026, 44).await.unwrap();
    let loaded = queries::load_week(&rp, 2026, 44).await.unwrap().unwrap();
    assert_eq!(to_json(&got.document), to_json(&loaded));
    assert_eq!(
        code(queries::get_week(&rp, 2026, 45).await),
        "week_not_found"
    );
    assert_eq!(
        code(queries::get_week(&rp, 2025, 53).await),
        "invalid_input"
    );
    assert_eq!(code(queries::get_week(&rp, 2026, 0).await), "invalid_input");
}

// ---- tests: list_weeks_page ----

async fn week_numbers(page: &sous::model::WeekPage) -> Vec<(i32, u32)> {
    page.weeks
        .iter()
        .map(|w| (w.iso_year, w.iso_week))
        .collect()
}

#[tokio::test]
async fn list_weeks_page_orders_newest_first() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let all = queries::list_weeks_page(&rp, None, None, page(None, None))
        .await
        .unwrap();
    assert_eq!(
        week_numbers(&all).await,
        [(2026, 44), (2026, 41), (2026, 38)]
    );
    assert!(all.next_cursor.is_none());
    let w44 = &all.weeks[0];
    assert_eq!(w44.id, W44_ID);
    assert_eq!(w44.start_date, "2026-10-26");
    assert_eq!(w44.end_date, "2026-11-01");
    assert_eq!(w44.path, "/2026/44");
    let value = to_json(&all);
    assert!(value["nextCursor"].is_null(), "explicit null");
}

#[tokio::test]
async fn list_weeks_page_overlap_boundaries() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let list = |from: Option<chrono::NaiveDate>, to: Option<chrono::NaiveDate>| {
        let rp = rp.clone();
        async move {
            let p = queries::list_weeks_page(&rp, from, to, page(None, None))
                .await
                .unwrap();
            week_numbers(&p).await
        }
    };
    // W41 is 2026-10-05 (Mon) .. 2026-10-11 (Sun).
    assert_eq!(
        list(Some(d(2026, 10, 11)), None).await,
        [(2026, 44), (2026, 41)]
    );
    assert_eq!(list(Some(d(2026, 10, 12)), None).await, [(2026, 44)]);
    assert_eq!(
        list(None, Some(d(2026, 10, 5))).await,
        [(2026, 41), (2026, 38)]
    );
    assert_eq!(list(None, Some(d(2026, 10, 4))).await, [(2026, 38)]);
    assert_eq!(
        list(Some(d(2026, 10, 7)), Some(d(2026, 10, 7))).await,
        [(2026, 41)]
    );
    assert_eq!(
        list(Some(d(2026, 10, 12)), Some(d(2026, 10, 25))).await,
        Vec::<(i32, u32)>::new()
    );
    assert_eq!(
        list(Some(d(2026, 9, 1)), Some(d(2026, 12, 31))).await,
        [(2026, 44), (2026, 41), (2026, 38)]
    );
}

#[tokio::test]
async fn list_weeks_page_walks_every_page_once() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..3 {
        let p = queries::list_weeks_page(&rp, None, None, page(Some(1), cursor.as_deref()))
            .await
            .unwrap();
        assert_eq!(p.weeks.len(), 1);
        seen.extend(week_numbers(&p).await);
        cursor = p.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, [(2026, 44), (2026, 41), (2026, 38)]);
    assert!(cursor.is_none(), "the last page ends with null");
}

#[tokio::test]
async fn list_weeks_page_is_stable_when_weeks_are_inserted() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let first = queries::list_weeks_page(&rp, None, None, page(Some(1), None))
        .await
        .unwrap();
    assert_eq!(week_numbers(&first).await, [(2026, 44)]);
    // A newer week (already passed) and an older one (still ahead) appear between fetches.
    t.exec(&format!(
        "INSERT INTO weeks (id, iso_year, iso_week) VALUES ('{}', 2026, 45)",
        uid(0xb0, 45)
    ))
    .await;
    t.exec(&format!(
        "INSERT INTO weeks (id, iso_year, iso_week) VALUES ('{}', 2026, 40)",
        uid(0xb0, 40)
    ))
    .await;
    let mut seen = week_numbers(&first).await;
    let mut cursor = first.next_cursor;
    while let Some(c) = cursor {
        let p = queries::list_weeks_page(&rp, None, None, page(Some(1), Some(&c)))
            .await
            .unwrap();
        seen.extend(week_numbers(&p).await);
        cursor = p.next_cursor;
    }
    assert_eq!(seen, [(2026, 44), (2026, 41), (2026, 40), (2026, 38)]);

    // Deleting an unseen week does not skip a neighbour either.
    t.exec(&format!("DELETE FROM weeks WHERE id = '{}'", uid(0xb0, 40)))
        .await;
    let first = queries::list_weeks_page(&rp, None, None, page(Some(2), None))
        .await
        .unwrap();
    assert_eq!(week_numbers(&first).await, [(2026, 45), (2026, 44)]);
    t.exec("DELETE FROM weeks WHERE iso_week = 41").await;
    let rest =
        queries::list_weeks_page(&rp, None, None, page(Some(2), first.next_cursor.as_deref()))
            .await
            .unwrap();
    assert_eq!(week_numbers(&rest).await, [(2026, 38)]);
    assert!(rest.next_cursor.is_none());
}

#[tokio::test]
async fn list_weeks_page_rejects_bad_arguments() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let list = |from, to, p| {
        let rp = rp.clone();
        async move { queries::list_weeks_page(&rp, from, to, p).await }
    };
    assert_eq!(
        code(list(None, None, page(Some(0), None)).await),
        "invalid_input"
    );
    assert_eq!(
        code(list(None, None, page(Some(101), None)).await),
        "invalid_input"
    );
    assert!(list(None, None, page(Some(100), None)).await.is_ok());
    assert_eq!(
        code(list(Some(d(2026, 10, 2)), Some(d(2026, 10, 1)), page(None, None)).await),
        "invalid_input"
    );
    for bad in ["", "zz", "abc", "7b7d", "7B7D", "00"] {
        assert_eq!(
            code(list(None, None, page(None, Some(bad))).await),
            "invalid_input",
            "cursor {bad:?}"
        );
    }
    // A real cursor from list_weeks works only with the same filters.
    let first = list(Some(d(2026, 1, 1)), None, page(Some(1), None))
        .await
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    assert!(
        list(Some(d(2026, 1, 1)), None, page(Some(2), Some(&cursor)))
            .await
            .is_ok(),
        "limit is not part of the binding"
    );
    assert_eq!(
        code(list(Some(d(2026, 1, 2)), None, page(Some(1), Some(&cursor))).await),
        "invalid_input"
    );
    assert_eq!(
        code(list(None, None, page(Some(1), Some(&cursor))).await),
        "invalid_input"
    );
    assert_eq!(
        code(
            list(
                Some(d(2026, 1, 1)),
                Some(d(2026, 12, 31)),
                page(Some(1), Some(&cursor))
            )
            .await
        ),
        "invalid_input"
    );
    // A cursor from search_meals is not accepted by list_weeks.
    let found = queries::search_meals(&rp, Some("casserole"), None, None, page(Some(1), None))
        .await
        .unwrap();
    assert_eq!(
        code(list(None, None, page(None, found.next_cursor.as_deref())).await),
        "invalid_input"
    );
}

// ---- tests: search_meals ----

async fn search(
    pool: &SqlitePool,
    query: Option<&str>,
    from: Option<chrono::NaiveDate>,
    to: Option<chrono::NaiveDate>,
) -> Vec<String> {
    let p = queries::search_meals(pool, query, from, to, page(Some(100), None))
        .await
        .unwrap();
    assert!(p.next_cursor.is_none());
    ids(&p.meals, |m| &m.id)
}

#[tokio::test]
async fn search_meals_matches_literally_and_case_insensitively() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let w41 = (Some(d(2026, 10, 5)), Some(d(2026, 10, 11)));
    assert_eq!(
        search(&rp, Some("casserole"), w41.0, w41.1).await,
        [meal(2), meal(1)]
    );
    let unbounded = [meal(2), meal(1), meal(4)];
    assert_eq!(search(&rp, Some("casserole"), None, None).await, unbounded);
    assert_eq!(search(&rp, Some("CASSEROLE"), None, None).await, unbounded);
    assert_eq!(search(&rp, Some("äpfel"), None, None).await, [meal(5)]);
    assert_eq!(search(&rp, Some("KÄSE"), None, None).await, [meal(5)]);
    assert_eq!(search(&rp, Some("%"), None, None).await, [meal(3)]);
    assert_eq!(search(&rp, Some("_"), None, None).await, [meal(3)]);
    assert_eq!(search(&rp, Some("100% rye"), None, None).await, [meal(3)]);
    assert!(
        search(&rp, Some("no such dish"), None, None)
            .await
            .is_empty()
    );

    let none = queries::search_meals(&rp, Some("zzz"), None, None, page(None, None))
        .await
        .unwrap();
    assert!(none.meals.is_empty());
    assert!(to_json(&none)["nextCursor"].is_null());

    let hits = queries::search_meals(&rp, Some("casserole"), None, None, page(None, None))
        .await
        .unwrap();
    assert_eq!(
        to_json(&hits.meals[0]),
        json!({
            "id": meal(2), "name": "Leftover bowl", "description": "Reheated CASSEROLE with greens",
            "type": "Lunch", "date": "2026-10-09", "isoYear": 2026, "isoWeek": 41,
            "weekPath": "/2026/41",
        })
    );
}

#[tokio::test]
async fn search_meals_folds_sigma_variants() {
    let t = TestDb::migrated().await;
    let wp = t.write_pool().await;
    save_w45(
        &wp,
        None,
        edited(|v| {
            v["days"][0]["meals"][0]["name"] = json!("ΟΣ");
            v["days"][0]["meals"][1]["name"] = json!("μέλος");
        }),
    )
    .await
    .unwrap();
    let rp = t.reader().await;
    for q in ["σ", "ς", "Σ", "ος", "ΟΣ"] {
        let hits = search(&rp, Some(q), None, None).await;
        assert!(hits.contains(&uid(0xa2, 1)), "{q}: {hits:?}");
    }
    for q in ["ΜΈΛΟΣ", "μέλοσ", "μέλος"] {
        assert_eq!(
            search(&rp, Some(q), None, None).await,
            [uid(0xa2, 2)],
            "{q}"
        );
    }
}

#[tokio::test]
async fn search_meals_by_date_range_only() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    assert_eq!(
        search(&rp, None, Some(d(2026, 10, 5)), Some(d(2026, 10, 11))).await,
        [meal(2), meal(1), meal(3)]
    );
    // Same day: meal position ascending.
    assert_eq!(
        search(&rp, None, Some(d(2026, 9, 16)), Some(d(2026, 9, 16))).await,
        [meal(5), meal(4)]
    );
    // An empty query counts as no query.
    assert_eq!(
        search(&rp, Some(""), Some(d(2026, 9, 16)), Some(d(2026, 9, 16))).await,
        [meal(5), meal(4)]
    );
    // Boundaries are inclusive per day.
    assert_eq!(
        search(&rp, Some("casserole"), Some(d(2026, 10, 7)), None).await,
        [meal(2), meal(1)]
    );
    assert_eq!(
        search(&rp, Some("casserole"), Some(d(2026, 10, 8)), None).await,
        [meal(2)]
    );
    assert_eq!(
        search(&rp, Some("casserole"), None, Some(d(2026, 10, 7))).await,
        [meal(1), meal(4)]
    );
    assert_eq!(
        search(&rp, Some("casserole"), None, Some(d(2026, 10, 6))).await,
        [meal(4)]
    );
    // A date-only search reaches the W44 meals in descending date order.
    let all = search(&rp, None, Some(d(2026, 10, 5)), None).await;
    assert!(all.len() > 3);
    assert_eq!(&all[all.len() - 3..], [meal(2), meal(1), meal(3)]);
}

#[tokio::test]
async fn search_meals_requires_a_criterion() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    for query in [None, Some("")] {
        let r = queries::search_meals(&rp, query, None, None, page(None, None)).await;
        assert_eq!(code(r), "invalid_input", "{query:?}");
    }
    let r = queries::search_meals(&rp, Some("x"), None, None, page(Some(0), None)).await;
    assert_eq!(code(r), "invalid_input");
    let r = queries::search_meals(&rp, Some("x"), None, None, page(Some(101), None)).await;
    assert_eq!(code(r), "invalid_input");
    let r = queries::search_meals(
        &rp,
        Some("x"),
        Some(d(2026, 10, 2)),
        Some(d(2026, 10, 1)),
        page(None, None),
    )
    .await;
    assert_eq!(code(r), "invalid_input");
}

#[tokio::test]
async fn search_meals_pages_without_skips_or_repeats() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let mut seen = Vec::new();
    let mut pages = 0;
    let mut cursor: Option<String> = None;
    loop {
        let p = queries::search_meals(
            &rp,
            Some("casserole"),
            None,
            None,
            page(Some(1), cursor.as_deref()),
        )
        .await
        .unwrap();
        pages += 1;
        assert_eq!(p.meals.len(), 1);
        seen.extend(ids(&p.meals, |m| &m.id));
        cursor = p.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, [meal(2), meal(1), meal(4)]);

    // Paging splits a single day at its position order too.
    let day = (Some(d(2026, 9, 16)), Some(d(2026, 9, 16)));
    let first = queries::search_meals(&rp, None, day.0, day.1, page(Some(1), None))
        .await
        .unwrap();
    assert_eq!(ids(&first.meals, |m| &m.id), [meal(5)]);
    let second = queries::search_meals(
        &rp,
        None,
        day.0,
        day.1,
        page(Some(1), first.next_cursor.as_deref()),
    )
    .await
    .unwrap();
    assert_eq!(ids(&second.meals, |m| &m.id), [meal(4)]);
    assert!(second.next_cursor.is_none());

    // A page that exactly empties the matches has no cursor.
    let exact = queries::search_meals(&rp, Some("casserole"), None, None, page(Some(3), None))
        .await
        .unwrap();
    assert_eq!(exact.meals.len(), 3);
    assert!(exact.next_cursor.is_none());
}

#[tokio::test]
async fn search_meals_cursor_survives_inserted_meals() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let first = queries::search_meals(&rp, Some("casserole"), None, None, page(Some(1), None))
        .await
        .unwrap();
    assert_eq!(ids(&first.meals, |m| &m.id), [meal(2)]);
    // A newer matching meal and an older one appear between fetches.
    t.exec(&format!(
        "INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two) \
         VALUES ('{}', '000000c2-0000-4000-8000-000000000415', 5, 'Dinner', 'Newer casserole', 'd', 1, 1, 1, 1, 1, 0)",
        meal(90)
    ))
    .await;
    t.exec(&format!(
        "INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two) \
         VALUES ('{}', '000000c2-0000-4000-8000-000000000383', 5, 'Dinner', 'Older casserole', 'd', 1, 1, 1, 1, 1, 0)",
        meal(91)
    ))
    .await;
    let mut seen = ids(&first.meals, |m| &m.id);
    let mut cursor = first.next_cursor;
    while let Some(c) = cursor {
        let p = queries::search_meals(&rp, Some("casserole"), None, None, page(Some(1), Some(&c)))
            .await
            .unwrap();
        seen.extend(ids(&p.meals, |m| &m.id));
        cursor = p.next_cursor;
    }
    // The newer meal (Fri pos 5 after the Fri pos 0 first hit) is ahead of the cursor; the
    // older one sits on the Sep 16 day after pos 10 would not, so it follows position 5 < 10.
    assert_eq!(
        seen,
        [meal(2), meal(90), meal(1), meal(91), meal(4)].map(String::from)[..].to_vec()
    );
}

#[tokio::test]
async fn search_meals_rejects_foreign_cursors() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let cursor = queries::search_meals(&rp, Some("casserole"), None, None, page(Some(1), None))
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    let with = |query: Option<&'static str>, from, to, c: String| {
        let rp = rp.clone();
        async move { queries::search_meals(&rp, query, from, to, page(Some(1), Some(&c))).await }
    };
    assert!(
        with(Some("casserole"), None, None, cursor.clone())
            .await
            .is_ok()
    );
    assert_eq!(
        code(with(Some("CASSEROLE"), None, None, cursor.clone()).await),
        "invalid_input"
    );
    assert_eq!(
        code(with(Some("casserole"), Some(d(2026, 1, 1)), None, cursor.clone()).await),
        "invalid_input"
    );
    assert_eq!(
        code(
            with(
                Some("casserole"),
                None,
                Some(d(2026, 12, 1)),
                cursor.clone()
            )
            .await
        ),
        "invalid_input"
    );
    assert_eq!(
        code(with(None, Some(d(2026, 1, 1)), None, cursor.clone()).await),
        "invalid_input"
    );
    assert_eq!(
        code(with(Some("casserole"), None, None, "nonsense".into()).await),
        "invalid_input"
    );
    // A list_weeks cursor is the other tool's.
    let weeks_cursor = queries::list_weeks_page(&rp, None, None, page(Some(1), None))
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    assert_eq!(
        code(with(Some("casserole"), None, None, weeks_cursor).await),
        "invalid_input"
    );
}

// ---- tests: get_meal ----

#[tokio::test]
async fn get_meal_returns_every_field() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let record = queries::get_meal(&rp, &meal(1)).await.unwrap();
    let expected = json!({
        "id": meal(1), "type": "Dinner", "name": "Chicken & Broccoli Casserole",
        "description": "Per person: 150g chicken, broccoli, light cheese sauce",
        "kcal": 610.0, "proteinG": 45.0, "fatG": 21.5, "carbsG": 48.0, "fiberG": 7.0,
        "forTwo": true, "assemblyTime": "15 min", "cookTime": "35 min",
        "tip": "Rest 5 min before serving.",
        "steps": [
            {"id": "000000c4-0000-4000-8000-000000000001", "text": "Heat the oven to 190°C."},
            {"id": "000000c4-0000-4000-8000-000000000002", "text": "Brown the chicken."},
            {"id": "000000c4-0000-4000-8000-000000000003", "text": "Layer chicken, broccoli and sauce in the dish."},
            {"id": "000000c4-0000-4000-8000-000000000004", "text": "Bake 35 min until golden."},
        ],
        "date": "2026-10-07", "weekday": 3, "isoYear": 2026, "isoWeek": 41, "weekPath": "/2026/41",
    });
    assert_eq!(to_json(&record), expected);

    let document = to_json(&queries::load_week(&rp, 2026, 41).await.unwrap().unwrap());
    let in_week = document["days"][2]["meals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == meal(1))
        .unwrap();
    let mut from_record = to_json(&record);
    for key in ["date", "weekday", "isoYear", "isoWeek", "weekPath"] {
        from_record.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(&from_record, in_week);

    // A meal with no steps and a null tip keeps explicit nulls.
    let toast = to_json(&queries::get_meal(&rp, &meal(3)).await.unwrap());
    assert_eq!(toast["steps"], json!([]));
    assert!(toast["tip"].is_null() && toast["cookTime"].is_null());
    assert_eq!(toast["date"], "2026-10-05");
    assert_eq!(toast["weekday"], 1);
}

#[tokio::test]
async fn get_meal_validates_the_id() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    for bad in [
        "",
        "garbage",
        "000000C3-0000-4000-8000-000000000001",
        " 000000c3-0000-4000-8000-000000000001",
        "000000c3-0000-4000-8000-0000000000011",
        "000000c3000040008000000000000001",
        "000000g3-0000-4000-8000-000000000001",
        "000000c3-0000-4000-8000-000000000001' OR '1'='1",
    ] {
        assert_eq!(
            code(queries::get_meal(&rp, bad).await),
            "invalid_input",
            "{bad:?}"
        );
    }
    assert_eq!(
        code(queries::get_meal(&rp, "000000c3-0000-4000-8000-0000000000ff").await),
        "meal_not_found"
    );
}

// ---- tests: shopping_lists_on ----

fn list_labels(day: &sous::model::ShoppingDay) -> Vec<String> {
    ids(&day.lists, |l| &l.label)
}

#[tokio::test]
async fn shopping_lists_on_monday_and_wednesday() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    let monday = queries::shopping_lists_on(&rp, d(2026, 10, 26), "Europe/Berlin")
        .await
        .unwrap();
    assert_eq!(monday.date, "2026-10-26");
    assert_eq!(monday.timezone, "Europe/Berlin");
    assert!(monday.plan_exists);
    assert_eq!((monday.iso_year, monday.iso_week), (2026, 44));
    assert_eq!(monday.week_path, "/2026/44");
    assert_eq!(
        list_labels(&monday),
        ["🧴 Household", "🛒 Monday Shop → covers Mon–Wed"]
    );

    let document = to_json(&queries::load_week(&rp, 2026, 44).await.unwrap().unwrap());
    let by_label = |label: &str| {
        document["shoppingLists"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["label"] == label)
            .unwrap()
            .clone()
    };
    let lists = to_json(&monday)["lists"].clone();
    assert_eq!(lists[0], by_label("🧴 Household"));
    assert_eq!(lists[1], by_label("🛒 Monday Shop → covers Mon–Wed"));
    // Exact categories and items, in stored order.
    let shop = &lists[1];
    assert_eq!(
        shop["note"],
        "You have: salmon, oats, walnuts, almonds, chia, miso, spices."
    );
    let names: Vec<&str> = shop["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Produce", "Dairy / Chilled", "Bakery"]);
    let produce: Vec<&str> = shop["categories"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        produce,
        [
            "1 cucumber",
            "1 courgette",
            "1 bag baby spinach (200g)",
            "200g mixed berries",
            "Fresh ginger"
        ]
    );
    assert_eq!(lists[0]["note"], Value::Null);

    // Wednesday shares the Monday Shop list.
    let wednesday = queries::shopping_lists_on(&rp, d(2026, 10, 28), "Europe/Berlin")
        .await
        .unwrap();
    assert_eq!(list_labels(&wednesday), ["🛒 Monday Shop → covers Mon–Wed"]);
    assert_eq!(to_json(&wednesday)["lists"][0], lists[1]);
}

#[tokio::test]
async fn shopping_lists_on_days_without_lists() {
    let t = TestDb::full().await;
    let rp = t.reader().await;
    // Tuesday: day record, no links. Thursday: no day record. Sunday: record, no links.
    for date in [d(2026, 10, 27), d(2026, 10, 29), d(2026, 11, 1)] {
        let day = queries::shopping_lists_on(&rp, date, "Europe/Berlin")
            .await
            .unwrap();
        assert!(day.plan_exists, "{date}");
        assert!(day.lists.is_empty(), "{date}");
        assert_eq!(day.week_path, "/2026/44");
    }
    // The unassigned Thursday Top-Up list never appears on any day of the week.
    for offset in 0..7 {
        let date = d(2026, 10, 26) + chrono::Days::new(offset);
        let day = queries::shopping_lists_on(&rp, date, "UTC").await.unwrap();
        assert!(
            !list_labels(&day).iter().any(|l| l.contains("Thursday")),
            "{date}"
        );
        assert_eq!(day.timezone, "UTC");
    }
    let none = queries::shopping_lists_on(&rp, d(2026, 11, 2), "Europe/Berlin")
        .await
        .unwrap();
    assert!(!none.plan_exists);
    assert!(none.lists.is_empty());
    assert_eq!(none.week_path, "/2026/45");
    assert_eq!((none.iso_year, none.iso_week), (2026, 45));
    assert_eq!(none.date, "2026-11-02");
    assert_eq!(
        code(queries::shopping_lists_on(&rp, d(9999, 12, 31), "Europe/Berlin").await),
        "invalid_input"
    );
}

#[tokio::test]
async fn shopping_lists_on_keeps_empty_lists_and_categories() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    save_w45(&wp, None, sample()).await.unwrap();
    // Monday links the empty list first, then the list with an empty category.
    let monday = queries::shopping_lists_on(&rp, d(2026, 11, 2), "Europe/Berlin")
        .await
        .unwrap();
    assert!(monday.plan_exists);
    assert_eq!(
        to_json(&monday)["lists"],
        json!([
            {"id": uid(0xa4, 2), "label": "Empty", "note": null, "categories": []},
            sample()["shoppingLists"][0].clone(),
        ])
    );
    assert_eq!(monday.lists[1].categories[1].items.len(), 0);
    // Wednesday: only the second list's twin; Tuesday has no day record.
    let wednesday = queries::shopping_lists_on(&rp, d(2026, 11, 4), "Europe/Berlin")
        .await
        .unwrap();
    assert_eq!(list_labels(&wednesday), ["Weekly"]);
    let tuesday = queries::shopping_lists_on(&rp, d(2026, 11, 3), "Europe/Berlin")
        .await
        .unwrap();
    assert!(tuesday.plan_exists && tuesday.lists.is_empty());
}

// ---- tests: errors ----

#[tokio::test]
async fn database_errors_are_safe() {
    let (_dir, pool) = tableless_pool().await;
    let errors = [
        queries::get_week(&pool, 2026, 44).await.unwrap_err(),
        queries::list_weeks_page(&pool, None, None, page(None, None))
            .await
            .unwrap_err(),
        queries::search_meals(&pool, Some("x"), None, None, page(None, None))
            .await
            .unwrap_err(),
        queries::get_meal(&pool, &meal(1)).await.unwrap_err(),
        queries::shopping_lists_on(&pool, d(2026, 10, 26), "UTC")
            .await
            .unwrap_err(),
    ];
    for error in errors {
        assert!(matches!(error, QueryError::Database(_)), "{error:?}");
        assert_eq!(error.code(), "service_unavailable");
        let message = error.message();
        assert_eq!(message, "Meal plans are temporarily unavailable.");
        let lower = message.to_lowercase();
        for forbidden in ["sql", "table", ".db", "/", "no such"] {
            assert!(!lower.contains(forbidden), "{message}");
        }
    }
}

#[test]
fn error_codes_and_messages() {
    let cases = [
        (
            QueryError::InvalidInput("days[2].weekday must be 1–7".into()),
            "invalid_input",
            "days[2].weekday must be 1–7",
        ),
        (
            QueryError::WeekNotFound,
            "week_not_found",
            "No meal plan for this week.",
        ),
        (
            QueryError::MealNotFound,
            "meal_not_found",
            "No meal with this id.",
        ),
        (
            QueryError::RevisionConflict(Conflict::AlreadySaved),
            "revision_conflict",
            "A plan is already saved for this week. Fetch it with get_week and resubmit with its revision.",
        ),
        (
            QueryError::RevisionConflict(Conflict::NotSaved),
            "revision_conflict",
            "No plan is saved for this week. Omit expectedRevision to create it.",
        ),
        (
            QueryError::RevisionConflict(Conflict::Stale),
            "revision_conflict",
            "The week changed since it was read. Fetch it with get_week and resubmit.",
        ),
    ];
    for (error, code, message) in cases {
        assert_eq!(error.code(), code);
        assert_eq!(error.message(), message);
    }
}

// ---- tests: save_week ----

#[tokio::test]
async fn save_creates_a_week_and_round_trips() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    let before = counts(&rp).await;
    let saved = save_w45(&wp, None, sample()).await.unwrap();
    assert_eq!(
        to_json(&saved.document),
        expected_document(&sample(), 2026, 45)
    );
    let got = queries::get_week(&rp, 2026, 45).await.unwrap();
    assert_eq!(got.revision, saved.revision);
    assert_eq!(to_json(&got.document), to_json(&saved.document));
    assert_eq!(counts(&rp).await, add(&before, &input_counts(&sample())));

    // Days that were not submitted read back with null ids.
    let document = to_json(&saved.document);
    assert!(document["days"][1]["id"].is_null());
    assert_eq!(document["days"][1]["date"], "2026-11-03");

    // Creating again is a conflict, and nothing is duplicated.
    let again = dump(&rp).await;
    assert_eq!(
        conflict(save_w45(&wp, None, sample()).await),
        Conflict::AlreadySaved
    );
    assert_eq!(dump(&rp).await, again);
}

#[tokio::test]
async fn save_revises_with_reordering_drops_and_additions() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    let w44_before = queries::get_week(&rp, 2026, 44).await.unwrap();
    let baseline = counts(&rp).await;
    let created = save_w45(&wp, None, sample()).await.unwrap();
    let w45_only = counts(&rp).await;
    assert_eq!(w45_only, add(&baseline, &input_counts(&sample())));

    let revised = edited(|v| {
        v["week"]["subtitle"] = json!(null);
        v["week"]["prepIntro"] = json!("Prep on Sunday.");
        // Reorder meals and steps; drop a step; add a new step with a client UUID.
        let monday = &mut v["days"][0];
        monday["meals"].as_array_mut().unwrap().reverse();
        let oats = &mut monday["meals"][1];
        oats["steps"] = json!([step(2, "Wait"), step(9, "Serve")]);
        monday["shoppingListIds"] = json!([uid(0xa4, 1), uid(0xa4, 2)]);
        monday["reminder"] = json!(null);
        // Wednesday is dropped entirely.
        v["days"].as_array_mut().unwrap().remove(1);
        // Reorder lists, categories and items; add an item and a category.
        let lists = v["shoppingLists"].as_array_mut().unwrap();
        lists.reverse();
        let weekly = &mut lists[1];
        weekly["categories"].as_array_mut().unwrap().reverse();
        weekly["categories"][1]["items"]
            .as_array_mut()
            .unwrap()
            .reverse();
        weekly["categories"][1]["items"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": uid(0xa6, 9), "text": "Beets"}));
        weekly["categories"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": uid(0xa5, 9), "name": "Pantry", "items": []}));
        // Drop a task, add a card item.
        v["prepSessions"][0]["tasks"]
            .as_array_mut()
            .unwrap()
            .remove(0);
        v["strategyCards"][0]["items"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"id": uid(0xaa, 9), "text": "Plan ahead"}));
    });
    let saved = save_w45(&wp, Some(&created.revision), revised.clone())
        .await
        .unwrap();
    assert_ne!(saved.revision, created.revision);
    assert_eq!(
        to_json(&saved.document),
        expected_document(&revised, 2026, 45)
    );
    assert_eq!(counts(&rp).await, add(&baseline, &input_counts(&revised)));

    // Retained ids keep their values; omitted rows are gone.
    assert!(exists(&rp, "meals", &uid(0xa2, 1)).await);
    assert!(exists(&rp, "meal_steps", &uid(0xa3, 2)).await);
    assert!(!exists(&rp, "meal_steps", &uid(0xa3, 1)).await);
    assert!(!exists(&rp, "days", &uid(0xa1, 2)).await);
    assert!(!exists(&rp, "meals", &uid(0xa2, 3)).await);
    assert!(!exists(&rp, "prep_tasks", &uid(0xa8, 1)).await);
    assert!(exists(&rp, "prep_tasks", &uid(0xa8, 2)).await);
    assert!(exists(&rp, "shopping_items", &uid(0xa6, 9)).await);
    assert!(exists(&rp, "strategy_items", &uid(0xaa, 9)).await);

    // The request that already succeeded cannot be replayed.
    let after = dump(&rp).await;
    assert_eq!(
        conflict(save_w45(&wp, Some(&created.revision), revised.clone()).await),
        Conflict::Stale
    );
    assert_eq!(dump(&rp).await, after);

    // Unrelated weeks are untouched.
    let w44_after = queries::get_week(&rp, 2026, 44).await.unwrap();
    assert_eq!(w44_after.revision, w44_before.revision);
    assert_eq!(to_json(&w44_after.document), to_json(&w44_before.document));

    // Re-saving identical content with the new revision yields the same revision.
    let same = save_w45(&wp, Some(&saved.revision), revised).await.unwrap();
    assert_eq!(same.revision, saved.revision);
}

#[tokio::test]
async fn save_replaces_with_an_empty_week() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    let baseline = counts(&rp).await;
    let created = save_w45(&wp, None, sample()).await.unwrap();
    let empty = json!({
        "week": {"id": uid(0xa0, 1), "subtitle": null, "prepIntro": null},
        "days": [], "shoppingLists": [], "prepSessions": [], "strategyCards": [],
    });
    let saved = save_w45(&wp, Some(&created.revision), empty.clone())
        .await
        .unwrap();
    assert_eq!(
        to_json(&saved.document),
        expected_document(&empty, 2026, 45)
    );
    assert_eq!(counts(&rp).await, add(&baseline, &input_counts(&empty)));
}

fn assert_invalid(result: Result<RevisedWeek, QueryError>, what: &str) {
    match result {
        Err(QueryError::InvalidInput(message)) => {
            assert!(!message.is_empty(), "{what}");
            assert!(!message.to_lowercase().contains("sql"), "{what}: {message}");
        }
        other => panic!("{what}: expected invalid_input, got {other:?}"),
    }
}

/// Edits that pure validation rejects.
fn invalid_edits() -> Vec<(&'static str, Value)> {
    let meal_id = |v: &mut Value| v["days"][0]["meals"][0]["id"].clone();
    vec![
        (
            "bad uuid",
            edited(|v| v["days"][0]["id"] = json!("not-a-uuid")),
        ),
        (
            "uppercase uuid",
            edited(|v| v["days"][0]["meals"][0]["id"] = json!(uid(0xa2, 1).to_uppercase())),
        ),
        (
            "bad step uuid",
            edited(|v| v["days"][0]["meals"][0]["steps"][0]["id"] = json!("123")),
        ),
        (
            "duplicate id in one table",
            edited(|v| v["days"][0]["meals"][1]["id"] = meal_id(v)),
        ),
        (
            "duplicate id across tables",
            edited(|v| {
                v["prepSessions"][0]["tasks"][0]["id"] =
                    v["days"][0]["meals"][0]["steps"][0]["id"].clone()
            }),
        ),
        (
            "week id reused as a child",
            edited(|v| v["days"][0]["id"] = v["week"]["id"].clone()),
        ),
        (
            "duplicate weekday",
            edited(|v| v["days"][1]["weekday"] = json!(1)),
        ),
        ("weekday 0", edited(|v| v["days"][1]["weekday"] = json!(0))),
        ("weekday 8", edited(|v| v["days"][1]["weekday"] = json!(8))),
        (
            "unknown list reference",
            edited(|v| {
                v["days"][1]["shoppingListIds"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(uid(0xa4, 77)))
            }),
        ),
        (
            "list repeated within a day",
            edited(|v| v["days"][1]["shoppingListIds"] = json!([uid(0xa4, 1), uid(0xa4, 1)])),
        ),
        (
            "negative kcal",
            edited(|v| v["days"][0]["meals"][0]["kcal"] = json!(-1.0)),
        ),
        (
            "negative fiber",
            edited(|v| v["days"][0]["meals"][0]["fiberG"] = json!(-0.5)),
        ),
    ]
}

#[tokio::test]
async fn save_rejects_invalid_documents_and_leaves_the_database_unchanged() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;

    // Creating: nothing may be written.
    let fresh = dump(&rp).await;
    for (what, value) in invalid_edits() {
        assert_invalid(save_w45(&wp, None, value).await, what);
        assert_eq!(dump(&rp).await, fresh, "create: {what}");
    }
    assert_invalid(
        save::save_week(&wp, 2025, 53, None, input(sample())).await,
        "2025-W53",
    );
    assert_invalid(
        save::save_week(&wp, 2026, 54, None, input(sample())).await,
        "2026-W54",
    );
    let reused = edited(|v| v["week"]["id"] = json!(W44_ID));
    assert_invalid(
        save_w45(&wp, None, reused).await,
        "new week reusing another week's id",
    );
    let foreign = edited(|v| v["days"][0]["id"] = json!("00000002-0000-4000-8000-000000000002"));
    assert_invalid(
        save_w45(&wp, None, foreign).await,
        "child id of W44 on create",
    );
    assert_eq!(dump(&rp).await, fresh, "create: database unchanged");

    // Replacing: the old content stays whatever fails.
    let created = save_w45(&wp, None, sample()).await.unwrap();
    let saved = dump(&rp).await;
    for (what, value) in invalid_edits() {
        assert_invalid(save_w45(&wp, Some(&created.revision), value).await, what);
        assert_eq!(dump(&rp).await, saved, "replace: {what}");
    }
    let mismatched = edited(|v| v["week"]["id"] = json!(uid(0xa0, 2)));
    assert_invalid(
        save_w45(&wp, Some(&created.revision), mismatched).await,
        "week id mismatch on replace",
    );
    assert_eq!(dump(&rp).await, saved, "week id mismatch");
    let reused = edited(|v| v["week"]["id"] = json!(W44_ID));
    assert_invalid(
        save_w45(&wp, Some(&created.revision), reused).await,
        "replace with another week's id",
    );
    assert_eq!(dump(&rp).await, saved, "reused week id");
    assert_invalid(
        save::save_week(&wp, 2025, 53, Some(&created.revision), input(sample())).await,
        "2025-W53 with revision",
    );
    assert_eq!(dump(&rp).await, saved);

    // Foreign-week ids are only found after the old children were deleted: all of it rolls back.
    for (table_what, edit) in [
        (
            "W44 day id",
            Box::new(|v: &mut Value| {
                v["days"][0]["id"] = json!("00000002-0000-4000-8000-000000000002")
            }) as Box<dyn Fn(&mut Value)>,
        ),
        (
            "W41 meal id",
            Box::new(|v: &mut Value| v["days"][0]["meals"][0]["id"] = json!(meal(1))),
        ),
        (
            "W44 step id",
            Box::new(|v: &mut Value| {
                let id: String = sqlx_free_step_id();
                v["days"][0]["meals"][0]["steps"][0]["id"] = json!(id);
            }),
        ),
        (
            "W44 list item id",
            Box::new(|v: &mut Value| {
                v["shoppingLists"][0]["categories"][0]["items"][0]["id"] =
                    json!("00000007-0000-4000-8000-000000000028")
            }),
        ),
        (
            "W44 strategy item id",
            Box::new(|v: &mut Value| {
                v["strategyCards"][0]["items"][0]["id"] =
                    json!("0000000b-0000-4000-8000-00000000003a")
            }),
        ),
    ] {
        let mut value = sample();
        edit(&mut value);
        match save_w45(&wp, Some(&created.revision), value).await {
            Err(QueryError::InvalidInput(message)) => {
                assert!(
                    message.contains("belongs to another week"),
                    "{table_what}: {message}"
                );
            }
            other => panic!("{table_what}: {other:?}"),
        }
        assert_eq!(
            dump(&rp).await,
            saved,
            "foreign id rolled back: {table_what}"
        );
        let still = queries::get_week(&rp, 2026, 45).await.unwrap();
        assert_eq!(still.revision, created.revision, "{table_what}");
    }
    // The write pool is still usable after the rollbacks.
    assert!(
        save_w45(&wp, Some(&created.revision), sample())
            .await
            .is_ok()
    );
}

/// A step id that exists in the W44 fixture.
fn sqlx_free_step_id() -> String {
    // Fixture meal steps use the 00000004 prefix; this one is Monday breakfast step 1.
    "00000004-0000-4000-8000-000000000009".to_string()
}

#[tokio::test]
async fn save_rejects_non_finite_nutrition() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    let before = dump(&rp).await;
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for field in 0..5 {
            let mut doc = input(sample());
            let meal = &mut doc.days[0].meals[0];
            match field {
                0 => meal.kcal = value,
                1 => meal.protein_g = value,
                2 => meal.fat_g = value,
                3 => meal.carbs_g = value,
                _ => meal.fiber_g = value,
            }
            assert_invalid(
                save::save_week(&wp, 2026, 45, None, doc).await,
                &format!("{value} in field {field}"),
            );
        }
    }
    assert_eq!(dump(&rp).await, before);
}

#[test]
fn input_rejects_malformed_documents() {
    let parse = |v: Value| serde_json::from_value::<WeekInput>(v);
    assert!(parse(sample()).is_ok());

    let err = parse(edited(|v| v["extra"] = json!(1)))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown field"), "{err}");
    assert!(parse(edited(|v| v["days"][0]["meals"][0]["extra"] = json!(1))).is_err());
    assert!(parse(edited(|v| v["week"]["isoYear"] = json!(2026))).is_err());
    assert!(parse(edited(|v| v["days"][0]["date"] = json!("2026-11-02"))).is_err());

    let err = parse(edited(|v| {
        v["days"][0]["meals"][0]["type"] = json!("Brunch")
    }))
    .unwrap_err()
    .to_string();
    assert!(err.contains("Brunch"), "{err}");
    assert!(
        parse(edited(
            |v| v["days"][0]["meals"][0]["type"] = json!("dinner")
        ))
        .is_err()
    );
    assert!(parse(edited(|v| v["strategyCards"][0]["theme"] = json!("red"))).is_err());
    assert!(parse(edited(|v| v["strategyCards"][0]["theme"] = json!("Blue"))).is_err());

    // A missing nullable key is an error; an explicit null is required.
    let err = parse(edited(|v| {
        v["days"][0]["meals"][0]
            .as_object_mut()
            .unwrap()
            .remove("tip");
    }))
    .unwrap_err()
    .to_string();
    assert!(err.contains("missing field `tip`"), "{err}");
    for (pointer, key) in [
        ("/week", "subtitle"),
        ("/week", "prepIntro"),
        ("/days/0", "reminder"),
        ("/days/0/reminder", "body"),
        ("/days/0/reminder", "icon"),
        ("/days/0/meals/0", "assemblyTime"),
        ("/days/0/meals/0", "cookTime"),
        ("/shoppingLists/0", "note"),
        ("/prepSessions/0", "covers"),
        ("/strategyCards/0", "icon"),
    ] {
        let result = parse(edited(|v| {
            v.pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
        }));
        assert!(result.is_err(), "{pointer}/{key} may not be omitted");
    }
    // Other required keys too.
    assert!(
        parse(edited(|v| {
            v.as_object_mut().unwrap().remove("strategyCards");
        }))
        .is_err()
    );
    assert!(
        parse(edited(|v| {
            v["days"][0].as_object_mut().unwrap().remove("meals");
        }))
        .is_err()
    );
    // Wrong types.
    assert!(parse(edited(|v| v["days"][0]["weekday"] = json!("1"))).is_err());
    assert!(parse(edited(|v| v["days"][0]["weekday"] = json!(-1))).is_err());
    assert!(parse(edited(|v| v["days"][0]["meals"][0]["forTwo"] = json!(1))).is_err());
}

#[tokio::test]
async fn save_detects_stale_and_missing_revisions() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    // Some(revision) for a week that does not exist.
    let before = dump(&rp).await;
    for revision in ["deadbeef", "null", ""] {
        assert_eq!(
            conflict(save_w45(&wp, Some(revision), sample()).await),
            Conflict::NotSaved,
            "{revision:?}"
        );
    }
    assert_eq!(dump(&rp).await, before);

    let created = save_w45(&wp, None, sample()).await.unwrap();
    let second = save_w45(
        &wp,
        Some(&created.revision),
        edited(|v| v["week"]["subtitle"] = json!("Changed")),
    )
    .await
    .unwrap();
    let after = dump(&rp).await;
    // The first revision is now stale.
    for stale in [created.revision.as_str(), "", "null"] {
        assert_eq!(
            conflict(save_w45(&wp, Some(stale), sample()).await),
            Conflict::Stale,
            "{stale:?}"
        );
    }
    assert_eq!(
        conflict(save_w45(&wp, None, sample()).await),
        Conflict::AlreadySaved
    );
    assert_eq!(dump(&rp).await, after);
    let current = queries::get_week(&rp, 2026, 45).await.unwrap();
    assert_eq!(current.revision, second.revision);
}

#[tokio::test]
async fn save_detects_external_edits() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    let created = save_w45(&wp, None, sample()).await.unwrap();
    t.exec(&format!(
        "UPDATE meals SET name = 'Changed elsewhere' WHERE id = '{}'",
        uid(0xa2, 1)
    ))
    .await;
    let after_edit = dump(&rp).await;
    assert_eq!(
        conflict(save_w45(&wp, Some(&created.revision), sample()).await),
        Conflict::Stale
    );
    assert_eq!(dump(&rp).await, after_edit);
    // A fresh read gives a revision that works.
    let fresh = queries::get_week(&rp, 2026, 45).await.unwrap();
    assert!(save_w45(&wp, Some(&fresh.revision), sample()).await.is_ok());
}

#[tokio::test]
async fn save_is_atomic_with_concurrent_readers_and_writers() {
    let t = TestDb::full().await;
    let wp_a = t.write_pool().await;
    let wp_b = t.write_pool().await;
    let rp = t.reader().await;
    let created = save_w45(&wp_a, None, sample()).await.unwrap();
    let old = to_json(&created.document);

    let from_a = edited(|v| v["week"]["subtitle"] = json!("From A"));
    let from_b = edited(|v| v["week"]["subtitle"] = json!("From B"));
    let (paused_tx, paused_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let revision = created.revision.clone();
    let a = tokio::spawn({
        let wp = wp_a.clone();
        async move {
            save::save_week_with_pause(&wp, 2026, 45, Some(&revision), input(from_a), async move {
                let _ = paused_tx.send(());
                let _ = release_rx.await;
            })
            .await
        }
    });
    paused_rx.await.unwrap();

    // A is mid-write: its old children are deleted, the new ones are not inserted yet.
    let during = queries::load_week(&rp, 2026, 45).await.unwrap().unwrap();
    assert_eq!(
        to_json(&during),
        old,
        "readers see the complete old document"
    );
    let during = queries::get_week(&rp, 2026, 45).await.unwrap();
    assert_eq!(during.revision, created.revision);

    let revision = created.revision.clone();
    let b = tokio::spawn({
        let wp = wp_b.clone();
        async move { save::save_week(&wp, 2026, 45, Some(&revision), input(from_b)).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!b.is_finished(), "B waits for A's write lock");
    release_tx.send(()).unwrap();

    let saved_a = a.await.unwrap().unwrap();
    let result_b = b.await.unwrap();
    assert_eq!(conflict(result_b), Conflict::Stale);
    let last = queries::get_week(&rp, 2026, 45).await.unwrap();
    assert_eq!(last.revision, saved_a.revision);
    assert_eq!(to_json(&last.document)["week"]["subtitle"], "From A");
    assert_eq!(to_json(&last.document), to_json(&saved_a.document));
}

#[tokio::test]
async fn saved_history_is_independent_of_copies() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let rp = t.reader().await;
    let original = to_json(&queries::get_meal(&rp, &meal(1)).await.unwrap());

    // Copy the casserole into W45 under new ids.
    let mut copy = sample_meal(1, "Dinner", "Chicken & Broccoli Casserole", vec![]);
    copy["description"] = original["description"].clone();
    copy["kcal"] = original["kcal"].clone();
    copy["proteinG"] = original["proteinG"].clone();
    copy["fatG"] = original["fatG"].clone();
    copy["carbsG"] = original["carbsG"].clone();
    copy["fiberG"] = original["fiberG"].clone();
    copy["assemblyTime"] = original["assemblyTime"].clone();
    copy["cookTime"] = original["cookTime"].clone();
    copy["tip"] = original["tip"].clone();
    copy["steps"] = json!(
        original["steps"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, s)| step(10 + i as u32, s["text"].as_str().unwrap()))
            .collect::<Vec<_>>()
    );
    let first = edited(|v| v["days"][0]["meals"] = json!([copy.clone()]));
    let created = save_w45(&wp, None, first).await.unwrap();
    assert_eq!(
        to_json(&queries::get_meal(&rp, &meal(1)).await.unwrap()),
        original
    );

    // Edit the copy's steps with a second save.
    let mut edited_copy = copy.clone();
    edited_copy["steps"] = json!([step(10, "Totally different."), step(20, "New step.")]);
    let second = edited(|v| v["days"][0]["meals"] = json!([edited_copy.clone()]));
    save_w45(&wp, Some(&created.revision), second)
        .await
        .unwrap();
    assert_eq!(
        to_json(&queries::get_meal(&rp, &meal(1)).await.unwrap()),
        original
    );
    assert_eq!(
        queries::get_meal(&rp, &uid(0xa2, 1))
            .await
            .unwrap()
            .meal
            .steps
            .len(),
        2
    );
}

#[tokio::test]
async fn saved_content_is_visible_without_restarting() {
    let t = TestDb::full().await;
    let wp = t.write_pool().await;
    let early_reader = t.reader().await;
    assert!(
        queries::load_week(&early_reader, 2026, 45)
            .await
            .unwrap()
            .is_none()
    );
    let app = sous::app(t.reader().await, berlin_clock());
    let api = |app: axum::Router| async move {
        let res = app
            .oneshot(Request::get("/api/2026/45").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice::<Value>(&body).unwrap())
    };
    assert_eq!(api(app.clone()).await.0, StatusCode::NOT_FOUND);

    let saved = save_w45(&wp, None, sample()).await.unwrap();
    let expected = to_json(&saved.document);
    let (status, body) = api(app.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, expected);
    let fresh = queries::load_week(&t.reader().await, 2026, 45)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(to_json(&fresh), expected);
    let early = queries::load_week(&early_reader, 2026, 45)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(to_json(&early), expected);

    // A revision (and the API) follow later saves too.
    let again = save_w45(
        &wp,
        Some(&saved.revision),
        edited(|v| v["week"]["subtitle"] = json!("Later")),
    )
    .await
    .unwrap();
    assert_eq!(api(app).await.1, to_json(&again.document));
}

// ---- tests: JSON Schema of the writable document ----

#[test]
fn week_input_schema_is_strict() {
    let schema = serde_json::to_value(schemars::schema_for!(WeekInput)).unwrap();
    let uuid = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";
    let defs = schema["$defs"].as_object().unwrap();
    let object_names = [
        "WeekFieldsInput",
        "DayInput",
        "ReminderInput",
        "MealInput",
        "TextInput",
        "ShoppingListInput",
        "ShoppingCategoryInput",
        "PrepSessionInput",
        "PrepTaskInput",
        "StrategyCardInput",
    ];

    let mut objects: Vec<(&str, &Value)> = vec![("WeekInput", &schema)];
    for name in object_names {
        objects.push((
            name,
            defs.get(name).unwrap_or_else(|| panic!("{name} in $defs")),
        ));
    }
    for (name, object) in objects {
        assert_eq!(object["type"], "object", "{name}");
        assert_eq!(object["additionalProperties"], false, "{name}");
        let mut properties: Vec<&str> = object["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut required: Vec<&str> = object["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} has required"))
            .iter()
            .map(|r| r.as_str().unwrap())
            .collect();
        properties.sort();
        required.sort();
        assert_eq!(properties, required, "{name}: every field is required");
        if let Some(id) = object["properties"].get("id") {
            assert_eq!(id["type"], "string", "{name}");
            assert_eq!(id["pattern"], uuid, "{name}");
        }
    }

    let nullable = |name: &str, field: &str| {
        let property = &defs[name]["properties"][field];
        let text = property.to_string();
        assert!(text.contains("null"), "{name}.{field} is nullable: {text}");
    };
    for (name, field) in [
        ("WeekFieldsInput", "subtitle"),
        ("WeekFieldsInput", "prepIntro"),
        ("DayInput", "reminder"),
        ("ReminderInput", "body"),
        ("ReminderInput", "icon"),
        ("MealInput", "assemblyTime"),
        ("MealInput", "cookTime"),
        ("MealInput", "tip"),
        ("ShoppingListInput", "note"),
        ("PrepSessionInput", "covers"),
        ("StrategyCardInput", "icon"),
    ] {
        nullable(name, field);
    }
    assert_eq!(
        defs["MealInput"]["properties"]["tip"]["type"],
        json!(["string", "null"])
    );

    let weekday = &defs["DayInput"]["properties"]["weekday"];
    assert_eq!(
        (weekday["minimum"].as_f64(), weekday["maximum"].as_f64()),
        (Some(1.0), Some(7.0))
    );
    assert_eq!(
        defs["DayInput"]["properties"]["shoppingListIds"]["items"]["pattern"],
        uuid
    );
    for field in ["kcal", "proteinG", "fatG", "carbsG", "fiberG"] {
        let property = &defs["MealInput"]["properties"][field];
        assert_eq!(property["minimum"].as_f64(), Some(0.0), "{field}");
        assert_eq!(property["type"], "number", "{field}");
    }
    let enum_values = |def: &Value| -> Vec<String> {
        let text = def.to_string();
        [
            "Breakfast",
            "Lunch",
            "Dinner",
            "Snack",
            "green",
            "blue",
            "purple",
            "yellow",
        ]
        .iter()
        .filter(|v| text.contains(&format!("\"{v}\"")))
        .map(|v| v.to_string())
        .collect()
    };
    assert_eq!(
        enum_values(&defs["MealType"]),
        ["Breakfast", "Lunch", "Dinner", "Snack"]
    );
    assert_eq!(
        enum_values(&defs["Theme"]),
        ["green", "blue", "purple", "yellow"]
    );
}
