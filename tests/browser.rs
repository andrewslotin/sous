//! Browser tests against the compiled, embedded web assets (headless Google Chrome).
//!
//! Run with: `cargo test --test browser -- --ignored`

use chrono::TimeZone;
use headless_chrome::protocol::cdp::Emulation::{SetDeviceMetricsOverride, SetTimezoneOverride};
use headless_chrome::{Browser, LaunchOptions, Tab};
use sous::calendar::Clock;
use sous::db;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, SqliteConnection};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const FIXTURE: &str = include_str!("../fixtures/2026-w44.sql");
const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const IGNORE: &str = "requires Google Chrome; run: cargo test --test browser -- --ignored";
const XSS_NAME: &str = "</script><script>window.pwned=1</script> <b>x</b>";

/// Deterministic UUID-shaped id: `kind` distinguishes tables, `n` the row.
fn id(kind: u32, n: u32) -> String {
    format!("{kind:08x}-0000-4000-8000-{n:012x}")
}

/// SQL for a week (ISO 2026-W`week`) whose Monday has the given meals:
/// (name, [kcal, protein, fat, carbs, fiber]).
fn crafted_week(week: u32, meals: &[(&str, [f64; 5])]) -> String {
    let base = 1000 * week;
    let mut sql = format!(
        "INSERT INTO weeks (id, iso_year, iso_week) VALUES ('{}', 2026, {week});\n\
         INSERT INTO days (id, week_id, weekday) VALUES ('{}', '{}', 1);\n",
        id(0xa1, base),
        id(0xa2, base),
        id(0xa1, base)
    );
    for (i, (name, v)) in meals.iter().enumerate() {
        sql += &format!(
            "INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two) \
             VALUES ('{}', '{}', {i}, 'Lunch', '{}', 'd', {}, {}, {}, {}, {}, 0);\n",
            id(0xa3, base + i as u32),
            id(0xa2, base),
            name.replace('\'', "''"),
            v[0],
            v[1],
            v[2],
            v[3],
            v[4]
        );
    }
    sql
}

/// Bare weeks (no days, no meals) for the given (ISO year, week) pairs.
fn bare_weeks(weeks: &[(i32, u32)]) -> String {
    weeks
        .iter()
        .map(|(y, w)| {
            format!(
                "INSERT INTO weeks (id, iso_year, iso_week) VALUES ('{}', {y}, {w});\n",
                id(0xb1, (*y as u32) * 100 + w)
            )
        })
        .collect()
}

/// What the server's database looks like for a test.
#[derive(Clone, Copy, PartialEq)]
enum Db {
    /// Migrated, with the 2026-W44 fixture.
    Fixture,
    /// Migrated, no rows.
    Empty,
    /// A valid SQLite file without the application tables (queries fail).
    NoTables,
    /// Fixture loaded, but the read pool is closed before serving (queries fail).
    ClosedPool,
}

/// Harness configuration. Defaults: fixture DB, `test_clock()`, 1280 px wide, start at `/2026/44`.
struct Opts {
    sql: String,
    width: u32,
    clock: Clock,
    db: Db,
    path: &'static str,
}

impl Opts {
    fn new() -> Self {
        Opts {
            sql: String::new(),
            width: 1280,
            clock: test_clock(),
            db: Db::Fixture,
            path: "/2026/44",
        }
    }

    fn sql(mut self, sql: &str) -> Self {
        self.sql = sql.to_string();
        self
    }

    fn width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    fn clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    fn db(mut self, db: Db) -> Self {
        self.db = db;
        self
    }

    fn path(mut self, path: &'static str) -> Self {
        self.path = path;
        self
    }

    /// Fresh DB, in-process server and Chrome, then load `path` and wait for any rendered page.
    fn launch(self) -> Harness {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let Opts {
            sql,
            width,
            clock,
            db: mode,
            path,
        } = self;
        let base = rt.block_on(async {
            let pool = if mode == Db::NoTables {
                let mut c = SqliteConnection::connect_with(&writer_options(&db_path))
                    .await
                    .unwrap();
                sqlx::raw_sql("CREATE TABLE unrelated (x INTEGER)")
                    .execute(&mut c)
                    .await
                    .unwrap();
                c.close().await.unwrap();
                db::open_read_pool(&db_path).await.unwrap()
            } else {
                db::migrate(&db_path).await.unwrap();
                let mut c = SqliteConnection::connect_with(&writer_options(&db_path))
                    .await
                    .unwrap();
                if mode != Db::Empty {
                    sqlx::raw_sql(FIXTURE).execute(&mut c).await.unwrap();
                }
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(&mut c)
                    .await
                    .unwrap();
                c.close().await.unwrap();
                let pool = db::open_read_pool(&db_path).await.unwrap();
                if mode == Db::ClosedPool {
                    pool.close().await;
                }
                pool
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(
                async move { axum::serve(listener, sous::app(pool, clock)).await.unwrap() },
            );
            format!("http://{addr}")
        });
        let browser = Browser::new(LaunchOptions {
            path: Some(CHROME.into()),
            headless: true,
            sandbox: false,
            ..Default::default()
        })
        .expect("launch Chrome");
        let tab = browser.new_tab().unwrap();
        tab.call_method(SetDeviceMetricsOverride {
            width,
            height: 900,
            device_scale_factor: 1.0,
            mobile: false,
            scale: None,
            screen_width: None,
            screen_height: None,
            position_x: None,
            position_y: None,
            dont_set_visible_size: None,
            screen_orientation: None,
            viewport: None,
            display_feature: None,
            device_posture: None,
        })
        .unwrap();
        let h = Harness {
            rt,
            _dir: dir,
            db_path,
            _browser: browser,
            tab,
            base,
        };
        h.goto_page(path);
        h
    }
}

fn writer_options(path: &std::path::Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
}

struct Harness {
    rt: tokio::runtime::Runtime,
    _dir: TempDir,
    db_path: PathBuf,
    _browser: Browser,
    tab: Arc<Tab>,
    base: String,
}

impl Harness {
    /// Fresh DB (migrated, fixture + `extra_sql`), in-process server, Chrome at `width` px.
    fn start(extra_sql: &str, width: u32) -> Self {
        Opts::new().sql(extra_sql).width(width).launch()
    }

    /// Run SQL through a separate writer connection (a change made by "another process").
    fn exec(&self, sql: &str) {
        self.rt.block_on(async {
            let mut c = SqliteConnection::connect_with(&writer_options(&self.db_path))
                .await
                .unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
                .execute(&mut c)
                .await
                .unwrap();
            c.close().await.unwrap();
        });
    }

    /// Override the browser's timezone (CDP `Emulation.setTimezoneOverride`).
    fn set_browser_timezone(&self, tz: &str) {
        self.tab
            .call_method(SetTimezoneOverride {
                timezone_id: tz.to_string(),
            })
            .unwrap();
        assert_eq!(
            self.js("Intl.DateTimeFormat().resolvedOptions().timeZone"),
            tz
        );
    }

    /// Navigate and wait for the plan page (day buttons).
    fn goto(&self, path: &str) {
        self.navigate(path);
        self.wait_for("document.querySelector('.day-button') !== null");
    }

    /// Navigate and wait for any rendered page.
    fn goto_page(&self, path: &str) {
        self.navigate(path);
        self.wait_for("document.querySelector('#root > *') !== null");
    }

    fn navigate(&self, path: &str) {
        self.tab
            .navigate_to(&format!("{}{path}", self.base))
            .unwrap()
            .wait_until_navigated()
            .unwrap();
    }

    /// Wait until the browser is on `path` and a page has rendered (after a click or key press).
    fn wait_page(&self, path: &str) {
        self.wait_for(&format!(
            "location.pathname === '{path}' && document.querySelector('#root > *') !== null"
        ));
    }

    fn pathname(&self) -> String {
        self.js("location.pathname").as_str().unwrap().to_string()
    }

    /// HTTP status of the current document's response.
    fn status(&self) -> u64 {
        self.js("performance.getEntriesByType('navigation')[0].responseStatus")
            .as_u64()
            .unwrap()
    }

    /// Text content of every element matching `selector`, in DOM order.
    fn texts(&self, selector: &str) -> Vec<String> {
        let v = self.js(&format!(
            "[...document.querySelectorAll('{selector}')].map(e => e.textContent)"
        ));
        serde_json::from_value(v).unwrap()
    }

    /// Evaluate a JS expression and return its JSON-serialised result.
    fn js(&self, expr: &str) -> serde_json::Value {
        let wrapped = format!("JSON.stringify(({expr}) ?? null)");
        let v = self.tab.evaluate(&wrapped, false).unwrap().value.unwrap();
        serde_json::from_str(v.as_str().unwrap_or("null")).unwrap()
    }

    fn wait_for(&self, cond: &str) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if self.js(cond) == serde_json::Value::Bool(true) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for: {cond}");
    }

    fn click(&self, selector: &str) {
        self.tab.find_element(selector).unwrap().click().unwrap();
    }

    fn day_button(index: usize) -> String {
        format!("document.querySelectorAll('.day-button')[{index}]")
    }

    fn pressed(&self) -> Vec<bool> {
        let v = self.js(
            "[...document.querySelectorAll('.day-button')].map(b => b.getAttribute('aria-pressed') === 'true')",
        );
        serde_json::from_value(v).unwrap()
    }

    /// Summary values as displayed, in order: kcal, protein, fat, carbs, fiber (unit stripped).
    fn summary(&self) -> Vec<String> {
        let v = self.js(
            "[...document.querySelectorAll('.summary-value')].map(e => e.firstChild.textContent)",
        );
        serde_json::from_value(v).unwrap()
    }

    fn text(&self, selector: &str) -> String {
        self.js(&format!("document.querySelector('{selector}').textContent"))
            .as_str()
            .unwrap()
            .to_string()
    }

    fn press(&self, key: &str) {
        self.tab.press_key(key).unwrap();
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn monday_selected_with_exact_fixture_totals() {
    let _ = IGNORE;
    let h = Harness::start("", 1280);
    assert_eq!(
        h.pressed(),
        vec![true, false, false, false, false, false, false]
    );
    assert_eq!(h.summary(), ["1570.5", "73.3", "74.4", "149", "28.5"]);

    // FOR TWO meals (Dinner 560 kcal, Lunch 380.5 kcal) are counted once, not doubled:
    // raw sums of the fixture rows vs. what is displayed.
    let raw_kcal = 200.0 + 560.0 + 380.5 + 430.0;
    assert_eq!(raw_kcal, 1570.5);
    let doubled_kcal = raw_kcal + 560.0 + 380.5;
    assert_ne!(h.summary()[0], format!("{doubled_kcal}"));
    assert_eq!(h.summary()[0], format!("{raw_kcal}"));
    let for_two = h.js("document.querySelectorAll('.badge-two').length");
    assert_eq!(for_two, 2);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn sums_before_rounding_and_trims_trailing_zero() {
    let sql = crafted_week(
        45,
        &[
            ("A", [0.04, 0.04, 0.5, 10.04, 0.04]),
            ("B", [0.04, 0.04, 0.5, 9.96, 0.04]),
        ],
    );
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/45");
    // Per-meal rounding would give 0 for kcal/protein/fiber; summing first gives 0.08 -> 0.1.
    // Fat 1.0 and carbs 20.0 are whole numbers and show without ".0".
    assert_eq!(h.summary(), ["0.1", "0.1", "1", "20", "0.1"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn day_switching_empty_states_and_restore() {
    let h = Harness::start("", 1280);
    let monday_names = h.js("[...document.querySelectorAll('.meal-name')].map(e => e.textContent)");
    assert_eq!(monday_names.as_array().unwrap().len(), 4);

    // Wednesday (index 2) has a day row but no meals.
    h.click(".day-button:nth-child(3)");
    h.wait_for(&format!(
        "{}.getAttribute('aria-pressed') === 'true'",
        Harness::day_button(2)
    ));
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    assert_eq!(h.js("document.querySelectorAll('.summary').length"), 0);
    assert_eq!(h.js("document.querySelectorAll('.meal').length"), 0);

    // Friday (index 4) has no day row at all.
    h.click(".day-button:nth-child(5)");
    h.wait_for(&format!(
        "{}.getAttribute('aria-pressed') === 'true'",
        Harness::day_button(4)
    ));
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    assert_eq!(h.js("document.querySelectorAll('.summary').length"), 0);
    assert_eq!(h.js("document.querySelectorAll('.meal').length"), 0);

    // Back to Monday restores its meals and summary.
    h.click(".day-button:nth-child(1)");
    h.wait_for("document.querySelectorAll('.meal').length === 4");
    assert_eq!(
        h.js("[...document.querySelectorAll('.meal-name')].map(e => e.textContent)"),
        monday_names
    );
    assert_eq!(h.summary(), ["1570.5", "73.3", "74.4", "149", "28.5"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn pointer_expands_and_collapses_meal() {
    let h = Harness::start("", 1280);
    let expanded = "document.querySelector('.meal-toggle').getAttribute('aria-expanded')";
    assert_eq!(h.js(expanded), "false");
    assert_eq!(h.js("document.querySelectorAll('.meal-details').length"), 0);

    h.click(".meal-toggle");
    h.wait_for(&format!("{expanded} === 'true'"));
    assert_eq!(h.js("document.querySelectorAll('.meal-details').length"), 1);
    assert_eq!(
        h.js("document.querySelector('.meal-details').getBoundingClientRect().height > 0"),
        true
    );

    h.click(".meal-toggle");
    h.wait_for(&format!("{expanded} === 'false'"));
    assert_eq!(h.js("document.querySelectorAll('.meal-details').length"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn keyboard_selects_day_and_expands_meal() {
    let h = Harness::start("", 1280);

    // Enter on a focused day button selects Wednesday.
    h.js(&format!("{}.focus()", Harness::day_button(2)));
    h.press("Enter");
    h.wait_for(&format!(
        "{}.getAttribute('aria-pressed') === 'true'",
        Harness::day_button(2)
    ));
    assert_eq!(h.text(".empty"), "No meals planned for this day.");

    // Back to Monday, then Space on a focused meal toggle expands it.
    h.click(".day-button:nth-child(1)");
    h.wait_for("document.querySelectorAll('.meal-toggle').length === 4");
    let expanded = "document.querySelector('.meal-toggle').getAttribute('aria-expanded')";
    h.js("document.querySelector('.meal-toggle').focus()");
    h.press(" ");
    h.wait_for(&format!("{expanded} === 'true'"));
    assert_eq!(h.js("document.querySelectorAll('.meal-details').length"), 1);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn script_terminating_names_render_as_text() {
    let sql = crafted_week(46, &[(XSS_NAME, [100.0, 1.0, 1.0, 1.0, 1.0])]);
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/46");
    assert_eq!(h.text(".meal-name"), XSS_NAME);
    assert_eq!(h.js("typeof window.pwned"), "undefined");
    assert_eq!(h.js("document.querySelectorAll('.meal-name b').length"), 0);
    assert_eq!(h.js("document.querySelectorAll('.meal-name *').length"), 0);
}

/// No overlapping summary items, values fit their boxes, no horizontal page overflow.
fn assert_summary_layout(h: &Harness) {
    let report = h.js(
        r#"(() => {
          const items = [...document.querySelectorAll('.summary-item')];
          const r = items.map(e => e.getBoundingClientRect());
          const overlaps = [];
          for (let i = 0; i < r.length; i++) for (let j = i + 1; j < r.length; j++) {
            const w = Math.min(r[i].right, r[j].right) - Math.max(r[i].left, r[j].left);
            const hh = Math.min(r[i].bottom, r[j].bottom) - Math.max(r[i].top, r[j].top);
            if (w > 0.5 && hh > 0.5) overlaps.push([i, j]);
          }
          const overflowing = items.map((item, i) => {
            const v = item.querySelector('.summary-value');
            const range = document.createRange();
            range.selectNodeContents(v);
            const t = range.getBoundingClientRect();
            return (v.scrollWidth > v.clientWidth + 0.5) || t.left < r[i].left - 0.5 || t.right > r[i].right + 0.5;
          });
          return {
            count: items.length,
            overlaps,
            overflowing,
            pageOverflow: document.documentElement.scrollWidth > window.innerWidth,
            innerWidth: window.innerWidth,
          };
        })()"#,
    );
    assert_eq!(report["count"], 5, "{report}");
    assert_eq!(report["overlaps"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["overflowing"],
        serde_json::json!([false, false, false, false, false]),
        "{report}"
    );
    assert_eq!(report["pageOverflow"], false, "{report}");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn summary_layout_at_375px() {
    let h = Harness::start("", 375);
    assert_eq!(h.js("window.innerWidth"), 375);
    assert_summary_layout(&h);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn summary_layout_at_1280px() {
    let h = Harness::start("", 1280);
    assert_eq!(h.js("window.innerWidth"), 1280);
    assert_summary_layout(&h);
}

fn clock_at(y: i32, mo: u32, d: u32, h: u32, mi: u32, sec: u32) -> Clock {
    Clock::fixed(
        sous::calendar::DEFAULT_TIMEZONE,
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, sec).unwrap(),
    )
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn plan_header_range_and_all_weeks_link_by_keyboard() {
    let h = Harness::start("", 1280);
    assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
    assert_eq!(h.js("document.querySelectorAll('a.all-weeks').length"), 1);
    assert_eq!(
        h.js("document.querySelector('a.all-weeks').getAttribute('href')"),
        "/weeks"
    );
    assert_eq!(h.text("a.all-weeks"), "All weeks");

    // The first tab stop is the link; it shows a focus ring; Enter follows it.
    h.press("Tab");
    assert_eq!(h.js("document.activeElement.className"), "all-weeks");
    assert_eq!(
        h.js("getComputedStyle(document.activeElement).outlineStyle"),
        "solid"
    );
    h.press("Enter");
    h.wait_page("/weeks");
    assert_eq!(h.text(".title"), "All weeks");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn archive_orders_weeks_and_links_navigate() {
    let sql = bare_weeks(&[(2026, 1), (2027, 1), (2025, 52), (2026, 53)]);
    let h = Harness::start(&sql, 1280);
    h.goto_page("/weeks");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "All weeks");
    assert_eq!(
        h.texts(".week-list > li > a.week-link .week-link-title"),
        [
            "Week 1 · 2027",
            "Week 53 · 2026",
            "Week 44 · 2026",
            "Week 1 · 2026",
            "Week 52 · 2025"
        ]
    );
    assert_eq!(
        h.texts(".week-link-range"),
        [
            "Jan 4 – Jan 10, 2027",
            "Dec 28, 2026 – Jan 3, 2027",
            "Oct 26 – Nov 1, 2026",
            "Dec 29, 2025 – Jan 4, 2026",
            "Dec 22 – Dec 28, 2025"
        ]
    );
    assert_eq!(
        h.js("[...document.querySelectorAll('.week-link')].map(a => a.getAttribute('href'))"),
        serde_json::json!(["/2027/1", "/2026/53", "/2026/44", "/2026/1", "/2025/52"])
    );
    assert_eq!(h.text("a.all-weeks"), "This week");

    // A change made by another connection shows up on reload; a future week is allowed.
    h.exec(&bare_weeks(&[(2027, 2)]));
    h.goto_page("/weeks");
    let titles = h.texts(".week-link-title");
    assert_eq!(titles.len(), 6);
    assert_eq!(titles[0], "Week 2 · 2027");

    // Following an entry opens that week; a stored future week selects Monday and the URL stays put.
    h.click(".week-link[href='/2027/2']");
    h.wait_page("/2027/2");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "Meal Plan: Jan 11 – Jan 17");
    assert_eq!(
        h.pressed(),
        vec![true, false, false, false, false, false, false]
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn past_week_selects_monday_and_keeps_url() {
    // The default clock is in W49, so W44 is in the past.
    let h = Harness::start("", 1280);
    assert_eq!(h.pathname(), "/2026/44");
    assert_eq!(
        h.pressed(),
        vec![true, false, false, false, false, false, false]
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn missing_week_pages_link_to_archive_only() {
    let h = Harness::start("", 1280);

    // Explicit missing week, across an ISO year boundary: 404 with the full range and years.
    h.goto_page("/2026/53");
    assert_eq!(h.status(), 404);
    assert_eq!(h.pathname(), "/2026/53");
    assert_eq!(h.text(".title"), "Meal Plan: Dec 28, 2026 – Jan 3, 2027");
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    assert_eq!(
        h.js("document.querySelectorAll('.week-list, .week-link').length"),
        0
    );
    assert_eq!(h.js("document.querySelectorAll('a.nav-link').length"), 0);
    assert_eq!(h.text("a.all-weeks"), "All weeks");
    assert_eq!(
        h.js("document.querySelector('a.all-weeks').getAttribute('href')"),
        "/weeks"
    );
    assert_eq!(
        h.js("document.querySelector('.header a.all-weeks').getBoundingClientRect().top < document.querySelector('.empty').getBoundingClientRect().top"),
        true
    );

    // `/` with the current week (W49: Nov 30 - Dec 6) not stored is a 200 and stays at `/`.
    h.goto_page("/");
    assert_eq!(h.status(), 200);
    assert_eq!(h.pathname(), "/");
    assert_eq!(h.text(".title"), "Meal Plan: Nov 30 – Dec 6, 2026");
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    assert_eq!(
        h.js("document.querySelectorAll('.week-list, .week-link').length"),
        0
    );

    // The "All weeks" link goes to the archive.
    h.click("a.all-weeks");
    h.wait_page("/weeks");
    assert_eq!(h.text(".title"), "All weeks");
    assert_eq!(h.texts(".week-link-title"), ["Week 44 · 2026"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn empty_database_pages() {
    let h = Opts::new().db(Db::Empty).path("/weeks").launch();
    assert_eq!(h.status(), 200);
    assert_eq!(h.texts(".empty"), ["No meal plans available yet."]);
    assert_eq!(h.js("document.querySelectorAll('.week-link').length"), 0);
    assert_eq!(h.text("a.all-weeks"), "This week");
    h.click("a.all-weeks");
    h.wait_page("/");
    // Missing page without any stored weeks: just the message, no list.
    assert_eq!(h.status(), 200);
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    assert_eq!(h.js("document.querySelectorAll('.week-list').length"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn invalid_week_page_and_zero_padded_redirect() {
    let h = Harness::start("", 1280);
    h.goto_page("/2025/53");
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["Invalid week."]);
    assert_eq!(h.texts("a.nav-link"), ["This week", "All weeks"]);
    h.click("a.nav-link[href='/weeks']");
    h.wait_page("/weeks");
    assert_eq!(h.text(".title"), "All weeks");

    h.goto_page("/2025/53");
    h.click("a.nav-link[href='/']");
    h.wait_page("/");
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);

    // Zero-padded paths redirect to the canonical path.
    h.goto_page("/2026/044");
    assert_eq!(h.pathname(), "/2026/44");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn database_failure_shows_unavailable_not_missing() {
    for mode in [Db::NoTables, Db::ClosedPool] {
        let h = Opts::new().db(mode).path("/").launch();
        for path in ["/", "/2026/44", "/weeks"] {
            h.goto_page(path);
            assert_eq!(h.status(), 503, "{path}");
            assert_eq!(h.pathname(), path);
            assert_eq!(
                h.texts(".empty"),
                ["Meal plans are temporarily unavailable."],
                "{path}"
            );
            assert_eq!(h.text(".title"), "Meal Plan");
            assert_eq!(h.js("document.querySelectorAll('.week-link').length"), 0);
        }
        // The retry link goes to `/`, which is still unavailable.
        h.click("a.nav-link");
        h.wait_page("/");
        assert_eq!(h.status(), 503);
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn current_day_comes_from_server_clock_not_browser_timezone() {
    // 2026-11-01T22:30Z is Sunday 23:30 in Berlin (W44). The browser zones below are on
    // Monday (UTC+14) and Sunday (UTC-11) at that instant; neither may influence the selection.
    let h = Opts::new()
        .clock(clock_at(2026, 11, 1, 22, 30, 0))
        .path("/")
        .launch();
    for tz in ["Pacific/Kiritimati", "Pacific/Pago_Pago"] {
        h.set_browser_timezone(tz);
        h.goto("/");
        assert_eq!(h.pathname(), "/");
        assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
        assert_eq!(
            h.pressed(),
            vec![false, false, false, false, false, false, true],
            "{tz}"
        );
        // The same week at its permanent URL selects today too.
        h.goto("/2026/44");
        assert!(h.pressed()[6], "{tz}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn year_boundary_week_selects_saturday() {
    // 2027-01-02 is a Saturday in ISO 2026-W53.
    let sql = bare_weeks(&[(2026, 53)]);
    let h = Opts::new()
        .sql(&sql)
        .clock(clock_at(2027, 1, 2, 12, 0, 0))
        .path("/")
        .launch();
    assert_eq!(h.pathname(), "/");
    assert_eq!(h.text(".title"), "Meal Plan: Dec 28 – Jan 3");
    assert_eq!(
        h.pressed(),
        vec![false, false, false, false, false, true, false]
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn list_pages_fit_375px() {
    let sql = bare_weeks(&[(2026, 53), (2027, 1)]);
    let h = Opts::new().sql(&sql).width(375).launch();
    for path in ["/weeks", "/2026/52", "/2025/53", "/"] {
        h.goto_page(path);
        assert_eq!(h.js("window.innerWidth"), 375);
        assert_eq!(
            h.js("document.documentElement.scrollWidth <= window.innerWidth"),
            true,
            "{path}"
        );
    }
    // The plan page with the "All weeks" link also fits.
    h.goto("/2026/44");
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= window.innerWidth"),
        true
    );
}

/// Feature 2 stub: fixed clock in ISO 2026-W49 (Wed), outside the fixture week.
fn test_clock() -> sous::calendar::Clock {
    use chrono::TimeZone;
    sous::calendar::Clock::fixed(
        sous::calendar::DEFAULT_TIMEZONE,
        chrono::Utc.with_ymd_and_hms(2026, 12, 2, 12, 0, 0).unwrap(),
    )
}
