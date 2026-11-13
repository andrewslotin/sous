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
const WEEKLY_FIXTURE: &str = include_str!("../fixtures/2026-w44-weekly.sql");
const PREP_DAYS_FIXTURE: &str = include_str!("../fixtures/2026-w44-prep-days.sql");
/// The days of the fixture week 2026-W44, Monday to Sunday.
const W44_DATES: [&str; 7] = [
    "2026-10-26",
    "2026-10-27",
    "2026-10-28",
    "2026-10-29",
    "2026-10-30",
    "2026-10-31",
    "2026-11-01",
];
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
    /// Serve `/mcp` too (with a writable pool).
    mcp: bool,
}

impl Opts {
    fn new() -> Self {
        Opts {
            sql: String::new(),
            width: 1280,
            clock: test_clock(),
            db: Db::Fixture,
            path: "/2026/44",
            mcp: false,
        }
    }

    fn mcp(mut self) -> Self {
        self.mcp = true;
        self
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
            mcp,
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
                    sqlx::raw_sql(WEEKLY_FIXTURE).execute(&mut c).await.unwrap();
                    sqlx::raw_sql(PREP_DAYS_FIXTURE)
                        .execute(&mut c)
                        .await
                        .unwrap();
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
            let app = if mcp {
                sous::app_with_mcp(
                    pool,
                    clock,
                    sous::McpConfig {
                        write_pool: db::open_write_pool(&db_path).await.unwrap(),
                        allowed_hosts: vec![],
                        allowed_origins: vec![],
                    },
                )
            } else {
                sous::app(pool, clock)
            };
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            format!("http://{addr}")
        });
        // No path: headless_chrome uses $CHROME, then searches PATH and standard install locations.
        let browser = Browser::new(LaunchOptions {
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

    /// Navigate and wait for the day view (its heading).
    fn goto(&self, path: &str) {
        self.navigate(path);
        self.wait_for("document.querySelector('.day-heading') !== null");
    }

    /// The day view's heading, e.g. "Wednesday, October 28".
    fn heading(&self) -> String {
        self.text(".day-heading")
    }

    /// Follow the "Previous day" / "Next day" link and wait for `path`.
    fn step_day(&self, label: &str, path: &str) {
        self.click(&format!("a[aria-label='{label}']"));
        self.wait_page(path);
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
fn monday_shown_with_exact_fixture_totals() {
    let _ = IGNORE;
    let h = Harness::start("", 1280);
    assert_eq!(h.heading(), "Monday, October 26");
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
fn day_pages_show_empty_states_and_meals_again() {
    let h = Harness::start("", 1280);
    let monday_names = h.js("[...document.querySelectorAll('.meal-name')].map(e => e.textContent)");
    assert_eq!(monday_names.as_array().unwrap().len(), 4);

    // Wednesday has a day record but no meals.
    h.goto("/2026-10-28");
    assert_eq!(h.heading(), "Wednesday, October 28");
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    assert_eq!(h.js("document.querySelectorAll('.summary').length"), 0);
    assert_eq!(h.js("document.querySelectorAll('.meal').length"), 0);

    // Friday has no day record at all.
    h.goto("/2026-10-30");
    assert_eq!(h.heading(), "Friday, October 30");
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    assert_eq!(h.js("document.querySelectorAll('.summary').length"), 0);
    assert_eq!(h.js("document.querySelectorAll('.meal').length"), 0);

    // Monday shows its meals and summary again.
    h.goto("/2026-10-26");
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
fn keyboard_follows_day_link_and_expands_meal() {
    let h = Harness::start("", 1280);

    // Enter on the focused "Next day" link opens Tuesday.
    h.js("document.querySelector(\"a[aria-label='Next day']\").focus()");
    h.press("Enter");
    h.wait_page("/2026-10-27");
    assert_eq!(h.heading(), "Tuesday, October 27");

    // Back to Monday, then Space on a focused meal toggle expands it.
    h.goto("/2026-10-26");
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
fn plan_header_range_and_no_all_weeks_link() {
    let h = Harness::start("", 1280);
    assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
    assert_eq!(h.js("document.querySelectorAll('a.all-weeks').length"), 0);
    assert_eq!(
        h.js("document.body.textContent.includes('All weeks')"),
        false
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn year_page_orders_weeks_and_links_navigate() {
    let sql = bare_weeks(&[(2026, 1), (2027, 1), (2025, 52), (2026, 53)]);
    let h = Harness::start(&sql, 1280);
    h.goto_page("/2026");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "Meal Plan: 2026");
    assert_eq!(h.js("document.title"), "2026 — Meal Plan");
    // Only 2026 weeks, newest first, with the week that starts in 2025 (W1) included.
    assert_eq!(
        h.texts(".week-list > li > a.week-link .week-link-title"),
        ["Week 53 · 2026", "Week 44 · 2026", "Week 1 · 2026"]
    );
    assert_eq!(
        h.texts(".week-link-range"),
        [
            "Dec 28, 2026 – Jan 3, 2027",
            "Oct 26 – Nov 1, 2026",
            "Dec 29, 2025 – Jan 4, 2026",
        ]
    );
    assert_eq!(
        h.js("[...document.querySelectorAll('.week-link')].map(a => a.getAttribute('href'))"),
        serde_json::json!(["/2026/53", "/2026/44", "/2026/1"])
    );

    // A change made by another connection shows up on reload; a future week is allowed.
    h.exec(&bare_weeks(&[(2026, 52)]));
    h.goto_page("/2026");
    let titles = h.texts(".week-link-title");
    assert_eq!(titles.len(), 4);
    assert_eq!(titles[0], "Week 53 · 2026");
    assert_eq!(titles[1], "Week 52 · 2026");

    // Following an entry opens that week; a stored future week shows its Monday and the URL stays put.
    h.exec(&bare_weeks(&[(2027, 2)]));
    h.goto_page("/2027");
    h.click(".week-link[href='/2027/2']");
    h.wait_page("/2027/2");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "Meal Plan: Jan 11 – Jan 17");
    assert_eq!(h.heading(), "Monday, January 11");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn year_header_links_in_order_same_style_one_row_right_aligned() {
    let sql = bare_weeks(&[(2025, 52), (2027, 1)]);
    let h = Harness::start(&sql, 1280);
    h.goto_page("/2026");
    assert_eq!(h.texts("a.all-weeks"), ["← 2025", "This week", "2027 →"]);
    assert_eq!(
        h.js("[...document.querySelectorAll('a.all-weeks')].map(a => a.getAttribute('href'))"),
        serde_json::json!(["/2025", "/", "/2027"])
    );
    assert_eq!(
        h.js("document.querySelectorAll('.header a.all-weeks').length"),
        3
    );
    let style = |prop: &str| {
        h.js(&format!(
            "new Set([...document.querySelectorAll('a.all-weeks')].map(a => getComputedStyle(a).{prop})).size"
        ))
    };
    assert_eq!(style("fontSize"), 1);
    assert_eq!(style("color"), 1);
    // One row: baseline alignment may shift boxes by a pixel or two when the arrow glyph
    // comes from a fallback font, but never by a line.
    assert_eq!(
        h.js("(() => { const t = [...document.querySelectorAll('a.all-weeks')].map(a => a.getBoundingClientRect().top); return Math.max(...t) - Math.min(...t) < 4; })()"),
        true
    );
    // Right-aligned: the last link ends at the right edge of the header, the first is not at the left edge.
    assert_eq!(
        h.js("Math.abs(document.querySelectorAll('a.all-weeks')[2].getBoundingClientRect().right - document.querySelector('.header').getBoundingClientRect().right) < 1"),
        true
    );
    assert_eq!(
        h.js("document.querySelectorAll('a.all-weeks')[0].getBoundingClientRect().left > document.querySelector('.header').getBoundingClientRect().left + 100"),
        true
    );

    h.click("a.all-weeks[href='/2025']");
    h.wait_page("/2025");
    assert_eq!(h.text(".title"), "Meal Plan: 2025");
    assert_eq!(h.texts("a.all-weeks"), ["This week", "2026 →"]);
    h.click("a.all-weeks[href='/']");
    h.wait_page("/");
    assert_eq!(h.text(".title"), "Meal Plan: Nov 30 – Dec 6, 2026");
    h.goto_page("/2026");
    h.click("a.all-weeks[href='/2027']");
    h.wait_page("/2027");
    assert_eq!(h.text(".title"), "Meal Plan: 2027");
    assert_eq!(h.texts("a.all-weeks"), ["← 2026", "This week"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn year_links_skip_gap_years_and_empty_year_keeps_links() {
    let sql = bare_weeks(&[(2024, 10)]);
    let h = Opts::new().sql(&sql).launch();
    // Plans exist in 2024 and 2026 only: neither is adjacent to the other.
    h.goto_page("/2026");
    assert_eq!(h.status(), 200);
    assert_eq!(h.texts("a.all-weeks"), ["This week"]);
    h.goto_page("/2024");
    assert_eq!(h.texts("a.all-weeks"), ["This week"]);

    // An empty year (404) still shows its adjacent links and the empty message.
    let sql = bare_weeks(&[(2024, 10), (2026, 1)]);
    let h = Opts::new().db(Db::Empty).sql(&sql).launch();
    h.goto_page("/2025");
    assert_eq!(h.status(), 404);
    assert_eq!(h.text(".title"), "Meal Plan: 2025");
    assert_eq!(h.texts(".empty"), ["No meal plans available yet."]);
    assert_eq!(h.texts("a.all-weeks"), ["← 2024", "This week", "2026 →"]);
    assert_eq!(h.js("document.querySelectorAll('.week-link').length"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_is_on_plan_pages_only_and_all_weeks_is_gone() {
    let h = Harness::start("", 1280);
    let present = |h: &Harness| h.js("document.querySelectorAll('#calendar').length");
    let no_all_weeks = |h: &Harness, what: &str| {
        assert_eq!(
            h.js("document.body.textContent.includes('All weeks')"),
            false,
            "{what}"
        );
    };
    for (path, status) in [
        ("/2026/44", 200),
        ("/2026/45", 404),
        ("/2026/54", 404),
        ("/2026-10-28", 200),
        ("/2026-11-11", 404),
        ("/2026-02-30", 404),
    ] {
        h.goto_page(path);
        assert_eq!(h.status(), status, "{path}");
        assert_eq!(present(&h), 1, "{path}");
        no_all_weeks(&h, path);
    }
    assert_eq!(h.texts("a.nav-link"), ["This week"]);

    // `/` with an empty database is a 200 missing-plan page with the calendar.
    let e = Opts::new().db(Db::Empty).path("/").launch();
    assert_eq!(e.status(), 200);
    assert_eq!(present(&e), 1);
    no_all_weeks(&e, "/ (empty)");

    // Year pages and the 503 page have no calendar.
    h.goto_page("/2026");
    assert_eq!(present(&h), 0);
    let u = Opts::new().db(Db::ClosedPool).path("/").launch();
    assert_eq!(u.status(), 503);
    assert_eq!(present(&u), 0);
    no_all_weeks(&u, "503");
}

impl Harness {
    fn cal_heading(&self) -> String {
        self.text(".cal-title")
    }

    /// The text of every cell of every calendar row: the week number, then the seven day numbers.
    fn cal_rows(&self) -> Vec<Vec<String>> {
        let v = self.js(
            "[...document.querySelectorAll('#calendar .cal-row')].map(r => [...r.children].map(c => c.textContent))",
        );
        serde_json::from_value(v).unwrap()
    }

    /// The `href` of every calendar date link, in DOM order.
    fn cal_hrefs(&self) -> Vec<String> {
        let v = self.js(
            "[...document.querySelectorAll('#calendar a.cal-cell')].map(a => a.getAttribute('href'))",
        );
        serde_json::from_value(v).unwrap()
    }

    /// `[href, aria-label]` of every date link carrying at least one mark.
    fn cal_marked(&self) -> Vec<(String, String)> {
        let v = self.js(
            "[...document.querySelectorAll('#calendar a.cal-cell')].filter(a => a.querySelector('.cal-mark')).map(a => [a.getAttribute('href'), a.getAttribute('aria-label')])",
        );
        serde_json::from_value(v).unwrap()
    }

    /// The `href` of every date link with `aria-current`.
    fn cal_current(&self) -> Vec<String> {
        let v = self.js(
            "[...document.querySelectorAll('#calendar [aria-current]')].map(a => a.getAttribute('aria-current') + ':' + a.getAttribute('href'))",
        );
        serde_json::from_value(v).unwrap()
    }

    /// The `[x, y]` day-of-month texts of the cells carrying the today mark.
    fn cal_today_cells(&self) -> Vec<String> {
        let v = self.js("[...document.querySelectorAll('#calendar .cal-today')].map(c => c.textContent + (c.classList.contains('cal-out') ? ' out' : ''))");
        serde_json::from_value(v).unwrap()
    }

    fn cal_click(&self, label: &str) {
        self.click(&format!("#calendar button[aria-label='{label}']"));
    }
}

/// Expected date-link hrefs of a month's grid (whole Monday-first rows), from chrono.
fn expected_month_hrefs(year: i32, month: u32) -> Vec<String> {
    use chrono::{Datelike, NaiveDate};
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .unwrap();
    let mut monday = first - chrono::Duration::days(first.weekday().num_days_from_monday() as i64);
    let mut hrefs = vec![];
    while monday < next {
        for i in 0..7 {
            let date = monday + chrono::Duration::days(i);
            hrefs.push(format!(
                "/{:04}-{:02}-{:02}",
                date.year(),
                date.month(),
                date.day()
            ));
        }
        monday += chrono::Duration::days(7);
    }
    hrefs
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_opens_on_the_expected_month() {
    let sql = bare_weeks(&[(2026, 1)]);
    let h = Opts::new().sql(&sql).launch();
    assert_eq!(h.cal_heading(), "October 2026");
    h.goto_page("/2026/1");
    assert_eq!(h.cal_heading(), "January 2026");
    h.goto_page("/2026/45");
    assert_eq!(h.cal_heading(), "November 2026");

    // A date page opens on the month of its date, also when the week's Thursday is in another month:
    // 2025-12-29 is in 2026-W1, whose Thursday is January 1, 2026.
    h.goto_page("/2025-12-29");
    assert_eq!(h.status(), 200);
    assert_eq!(h.cal_heading(), "December 2025");
    h.goto_page("/2026-01-01");
    assert_eq!(h.cal_heading(), "January 2026");
    h.goto_page("/2026-11-01");
    assert_eq!(h.status(), 200);
    assert_eq!(h.cal_heading(), "November 2026");
    // `/2026/44` and `/` keep the Thursday's month (October 29).
    h.goto_page("/2026/44");
    assert_eq!(h.cal_heading(), "October 2026");

    // The missing-plan page of a date opens on the date's month too.
    let h = Opts::new().launch();
    h.goto_page("/2025-12-29");
    assert_eq!(h.status(), 404);
    assert_eq!(h.cal_heading(), "December 2025");
    h.goto_page("/2026/1");
    assert_eq!(h.status(), 404);
    assert_eq!(h.cal_heading(), "January 2026");

    // The invalid date page and the invalid week page open on today's month (December 2, 2026).
    h.goto_page("/2026-02-30");
    assert_eq!(h.status(), 404);
    assert_eq!(h.cal_heading(), "December 2026");
    h.goto_page("/2026/54");
    assert_eq!(h.cal_heading(), "December 2026");

    // Today's month follows the server's clock and not the browser's:
    // 2026-11-30 23:30Z is already December 1 in Berlin.
    let h = Opts::new()
        .clock(clock_at(2026, 11, 30, 23, 30, 0))
        .path("/2026/54")
        .launch();
    h.set_browser_timezone("America/Los_Angeles");
    for path in ["/2026/54", "/2026-02-30"] {
        h.goto_page(path);
        assert_eq!(h.status(), 404);
        assert_eq!(h.cal_heading(), "December 2026", "{path}");
        assert_eq!(h.cal_today_cells(), ["1"], "{path}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_dates_link_to_date_paths_and_rows_are_not_links() {
    let h = Opts::new().launch();
    h.goto_page("/2026/1");
    assert_eq!(h.cal_heading(), "January 2026");
    // Every date of the grid is a link, including the dimmed ones of December 2025 and February 2026.
    let expected = expected_month_hrefs(2026, 1);
    assert_eq!(expected.len(), 35);
    assert_eq!(h.cal_hrefs(), expected);
    assert_eq!(h.cal_hrefs()[0], "/2025-12-29");
    assert_eq!(h.cal_hrefs()[34], "/2026-02-01");
    // Week rows are plain elements.
    assert_eq!(h.count("#calendar a.cal-row"), 0);
    assert_eq!(h.count("#calendar div.cal-row"), 5);
    assert_eq!(h.count("#calendar .cal-row a:not(.cal-cell)"), 0);
    let rows = h.cal_rows();
    assert_eq!(rows[0][..3], ["1", "29", "30"]);

    h.goto_page("/2026/44");
    assert_eq!(h.cal_heading(), "October 2026");
    assert_eq!(h.cal_hrefs(), expected_month_hrefs(2026, 10));
    // Each row shows its week number and seven dates, in the expected layout.
    let rows = h.cal_rows();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[4], ["44", "26", "27", "28", "29", "30", "31", "1"]);

    // December 2020, then January 2021, whose first row is ISO 2020-W53.
    h.goto_page("/2020/53");
    // Its Thursday is December 31, 2020.
    assert_eq!(h.cal_heading(), "December 2020");
    assert_eq!(h.cal_hrefs(), expected_month_hrefs(2020, 12));
    h.cal_click("Next month");
    assert_eq!(h.cal_heading(), "January 2021");
    assert_eq!(h.cal_hrefs(), expected_month_hrefs(2021, 1));
    assert_eq!(h.cal_rows()[0][0], "53");
    assert_eq!(h.cal_hrefs()[4], "/2021-01-01");

    // A date of an unplanned week opens the missing-plan page; a date of a planned week opens its plan,
    // also from a dimmed cell.
    h.goto_page("/2026/44");
    h.cal_click("Next month");
    h.click("#calendar a[href='/2026-11-11']");
    h.wait_page("/2026-11-11");
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    assert_eq!(h.count(".day-row"), 0);
    h.goto_page("/2026/45");
    h.cal_click("Previous month");
    h.click("#calendar a[href='/2026-10-28']");
    h.wait_page("/2026-10-28");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
    assert_eq!(h.heading(), "Wednesday, October 28");
    // Sunday, November 1 is a dimmed cell of the November grid but a date of the displayed week.
    h.goto_page("/2026-10-28");
    h.click("#calendar a[href='/2026-11-01']");
    h.wait_page("/2026-11-01");
    assert_eq!(h.heading(), "Sunday, November 1");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_marks_planned_weeks_and_the_displayed_week() {
    let sql = bare_weeks(&[(2026, 46), (2026, 1)]);
    let h = Opts::new().sql(&sql).launch();
    let planned = |h: &Harness| -> Vec<String> {
        let v = h.js("[...document.querySelectorAll('#calendar .cal-planned')].map(r => r.querySelector('.cal-wk').textContent)");
        serde_json::from_value(v).unwrap()
    };
    assert_eq!(planned(&h), ["44"]);
    // `/2026/44` displays its Monday (the clock is outside the week).
    assert_eq!(h.cal_current(), ["page:/2026-10-26"]);
    // The planned dot is visible.
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('.cal-planned .cal-dot')).backgroundColor !== 'rgba(0, 0, 0, 0)'"),
        true
    );
    h.cal_click("Next month");
    assert_eq!(h.cal_heading(), "November 2026");
    assert_eq!(planned(&h), ["44", "46"]);
    // Only the displayed date is current, in the dimmed part of the grid.
    assert_eq!(h.cal_current(), ["page:/2026-10-26"]);
    assert_eq!(
        h.js("document.querySelector('#calendar [aria-current]').classList.contains('cal-out')"),
        true
    );
    // Week rows are not links and carry no label of their own.
    assert_eq!(h.count("#calendar .cal-row[aria-label]"), 0);
    // The displayed date uses the selected-button gradient.
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('#calendar [aria-current]')).backgroundImage.includes('linear-gradient')"),
        true
    );
    assert_eq!(h.count("#calendar .cal-selected"), 1);

    h.goto_page("/2026/1");
    assert_eq!(h.cal_rows()[0][0], "1");
    assert_eq!(planned(&h), ["1"]);
    assert_eq!(h.cal_current(), ["page:/2025-12-29"]);

    // The date page of another day of the week marks that day.
    h.goto_page("/2026-01-03");
    assert_eq!(h.cal_current(), ["page:/2026-01-03"]);

    // Today follows the server clock (Berlin Dec 2), not the browser (Kiritimati: Dec 3).
    let h = Opts::new()
        .clock(clock_at(2026, 12, 2, 10, 30, 0))
        .path("/2026/49")
        .launch();
    h.set_browser_timezone("Pacific/Kiritimati");
    h.goto_page("/2026/49");
    assert_eq!(h.cal_heading(), "December 2026");
    assert_eq!(h.cal_today_cells(), ["2"]);
    h.cal_click("Previous month");
    assert_eq!(h.cal_heading(), "November 2026");
    assert_eq!(h.cal_today_cells(), ["2 out"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_month_navigation_is_local() {
    let h = Opts::new().launch();
    // An expanded meal, before navigating.
    h.click(".meal-toggle");
    h.wait_for("document.querySelectorAll('.meal-details').length === 1");
    let urls = h.requested_urls();
    let href = h.js("location.href");
    h.cal_click("Previous month");
    h.cal_click("Previous month");
    assert_eq!(h.cal_heading(), "August 2026");
    // No row of August 2026 is the displayed week.
    assert_eq!(
        h.js("document.querySelectorAll('#calendar [aria-current]').length"),
        0
    );
    // Keyboard: focus the next button and press Enter and Space.
    h.js("document.querySelector(\"#calendar button[aria-label='Next month']\").focus()");
    h.press("Enter");
    h.press(" ");
    assert_eq!(h.cal_heading(), "October 2026");
    h.cal_click("Next month");
    assert_eq!(h.cal_heading(), "November 2026");
    assert_eq!(
        h.js("document.querySelectorAll('#calendar [aria-current]').length"),
        1
    );
    h.cal_click("Previous month");
    assert_eq!(h.cal_heading(), "October 2026");
    assert_eq!(
        h.js("document.querySelector('#calendar [aria-current]').getAttribute('href')"),
        "/2026-10-26"
    );

    assert_eq!(h.requested_urls(), urls);
    assert_eq!(h.js("location.href"), href);
    // The expanded meal stays expanded.
    assert_eq!(h.js("document.querySelectorAll('.meal-details').length"), 1);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_range_limits() {
    let h = Opts::new().path("/0001/1").launch();
    assert_eq!(h.status(), 404);
    assert_eq!(h.cal_heading(), "January 0001");
    assert_eq!(
        h.js("document.querySelector(\"#calendar button[aria-label='Previous month']\").disabled"),
        true
    );
    assert_eq!(
        h.js("document.querySelector(\"#calendar button[aria-label='Next month']\").disabled"),
        false
    );
    assert_eq!(h.cal_hrefs()[0], "/0001-01-01");
    assert_eq!(h.cal_rows()[0][..3], ["1", "1", "2"]);
    h.js("document.querySelector(\"#calendar button[aria-label='Previous month']\").click()");
    assert_eq!(h.cal_heading(), "January 0001");
    assert_eq!(h.pathname(), "/0001/1");

    h.goto_page("/9999/51");
    assert_eq!(h.status(), 404);
    assert_eq!(h.cal_heading(), "December 9999");
    assert_eq!(
        h.js("document.querySelector(\"#calendar button[aria-label='Next month']\").disabled"),
        true
    );
    assert_eq!(
        h.js("document.querySelector(\"#calendar button[aria-label='Previous month']\").disabled"),
        false
    );
    let rows = h.cal_rows();
    let (last, others) = rows.split_last().unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(last, &["52", "27", "28", "29", "30", "31", "1", "2"]);
    // The last week ends in year 10000: its dates are plain text, the other four weeks are links.
    assert_eq!(h.count("#calendar .cal-row:last-child a"), 0);
    assert_eq!(h.count("#calendar .cal-row:last-child .cal-cell"), 7);
    assert_eq!(h.count("#calendar a.cal-cell"), 28);
    assert_eq!(h.cal_hrefs().last().unwrap(), "/9999-12-26");
    assert_eq!(others.len(), 4);
    h.js("document.querySelector(\"#calendar button[aria-label='Next month']\").click()");
    assert_eq!(h.cal_heading(), "December 9999");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_reflects_committed_weeks_on_reload() {
    let h = Opts::new().launch();
    let planned =
        "[...document.querySelectorAll('#calendar .cal-planned .cal-wk')].map(r => r.textContent)";
    assert_eq!(h.js(planned), serde_json::json!(["44"]));
    h.exec(&bare_weeks(&[(2026, 43)]));
    h.goto_page("/2026/44");
    assert_eq!(h.js(planned), serde_json::json!(["43", "44"]));
}

/// The calendar sidebar beside the 540 px column, centred as a group, without a toggle.
fn assert_sidebar_layout(width: u32) {
    let h = Opts::new().width(width).launch();
    assert_eq!(h.js("window.innerWidth"), width);
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('#calendar')).display"),
        "block"
    );
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('.calendar-toggle')).display"),
        "none"
    );
    assert_eq!(
        h.js("document.querySelector('.app').getBoundingClientRect().width"),
        540
    );
    assert_eq!(
        h.js("document.querySelector('#calendar').getBoundingClientRect().right <= document.querySelector('.app').getBoundingClientRect().left"),
        true
    );
    assert_eq!(
        h.js("Math.abs(document.querySelector('#calendar').getBoundingClientRect().left - (window.innerWidth - document.querySelector('.app').getBoundingClientRect().right)) <= 1"),
        true
    );
    assert_eq!(
        h.js("document.querySelector('#calendar').getBoundingClientRect().left >= 0"),
        true
    );
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= window.innerWidth"),
        true
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_is_a_sidebar_at_1280px() {
    assert_sidebar_layout(1280);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_is_a_sidebar_at_900px() {
    assert_sidebar_layout(900);
}

impl Harness {
    fn calendar_shown(&self) -> bool {
        self.js("getComputedStyle(document.querySelector('#calendar')).display !== 'none'")
            .as_bool()
            .unwrap()
    }

    fn expanded(&self) -> String {
        self.js("document.querySelector('.calendar-toggle').getAttribute('aria-expanded')")
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Open and close once with the pointer, asserting `aria-expanded` and that the page still fits.
    fn toggle_once(&self) {
        assert!(!self.calendar_shown());
        assert_eq!(self.expanded(), "false");
        self.click(".calendar-toggle");
        self.wait_for("document.querySelector('#calendar').offsetParent !== null");
        assert_eq!(self.expanded(), "true");
        assert_eq!(
            self.js("document.documentElement.scrollWidth <= window.innerWidth"),
            true
        );
        self.click(".calendar-toggle");
        self.wait_for("document.querySelector('#calendar').offsetParent === null");
        assert_eq!(self.expanded(), "false");
    }
}

fn assert_toggle_layout(width: u32) {
    let h = Opts::new().width(width).launch();
    assert!(!h.calendar_shown());
    assert_eq!(h.expanded(), "false");
    assert_eq!(
        h.js("document.querySelector('.calendar-toggle').getAttribute('aria-controls')"),
        "calendar"
    );
    assert_eq!(h.text(".calendar-toggle"), "Calendar");
    assert_eq!(
        h.js("document.querySelectorAll('.header .header-meta > .calendar-toggle').length"),
        1
    );
    // Right-aligned in the header row.
    assert_eq!(
        h.js("Math.abs(document.querySelector('.calendar-toggle').getBoundingClientRect().right - document.querySelector('.header').getBoundingClientRect().right) < 1"),
        true
    );
    let style = "(() => { const s = getComputedStyle(document.querySelector(SEL)); return [s.fontSize, s.color, s.fontFamily, s.fontWeight, s.textDecorationLine]; })()";
    let toggle = h.js(&style.replace("SEL", "'.calendar-toggle'"));

    // Opens between the header and the day row; closes again.
    h.click(".calendar-toggle");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    assert_eq!(h.expanded(), "true");
    assert_eq!(
        h.js("document.querySelector('#calendar').getBoundingClientRect().top >= document.querySelector('.header').getBoundingClientRect().bottom"),
        true
    );
    assert_eq!(
        h.js("document.querySelector('#calendar').getBoundingClientRect().bottom <= document.querySelector('.day-row').getBoundingClientRect().top"),
        true
    );
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= window.innerWidth"),
        true
    );
    h.click(".calendar-toggle");
    h.wait_for("document.querySelector('#calendar').offsetParent === null");
    assert_eq!(h.expanded(), "false");

    // Enter on the focused button toggles too, and a reload starts closed.
    h.js("document.querySelector('.calendar-toggle').focus()");
    h.press("Enter");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    assert_eq!(h.expanded(), "true");
    h.goto("/2026/44");
    assert!(!h.calendar_shown());
    assert_eq!(h.expanded(), "false");

    // Same look as the year page's links.
    h.goto_page("/2026");
    let link = h.js(&style.replace("SEL", "'a.all-weeks'"));
    assert_eq!(toggle, link);

    // The missing-plan and invalid pages toggle too, and fit when open.
    h.goto_page("/2026/45");
    h.toggle_once();
    h.goto_page("/2026/54");
    h.toggle_once();
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_toggle_at_899px() {
    assert_toggle_layout(899);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_toggle_at_375px() {
    assert_toggle_layout(375);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn year_link_appears_after_another_connection_commits_adjacent_plan() {
    let h = Harness::start("", 1280);
    h.goto_page("/2026");
    assert_eq!(h.texts("a.all-weeks"), ["This week"]);
    h.exec(&bare_weeks(&[(2027, 1)]));
    h.goto_page("/2026");
    assert_eq!(h.texts("a.all-weeks"), ["This week", "2027 →"]);
    h.exec(&bare_weeks(&[(2025, 1)]));
    h.goto_page("/2026");
    assert_eq!(h.texts("a.all-weeks"), ["← 2025", "This week", "2027 →"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn past_week_shows_monday_and_keeps_url() {
    // The default clock is in W49, so W44 is in the past.
    let h = Harness::start("", 1280);
    assert_eq!(h.pathname(), "/2026/44");
    assert_eq!(h.heading(), "Monday, October 26");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn missing_week_pages_have_the_calendar_and_no_links() {
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
    assert_eq!(h.js("document.querySelectorAll('a.all-weeks').length"), 0);
    assert_eq!(h.cal_heading(), "December 2026");
    assert_eq!(h.count(".day-row, .day-heading, .day-step"), 0);

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
    assert_eq!(h.js("document.querySelectorAll('a.all-weeks').length"), 0);
    assert_eq!(h.cal_heading(), "December 2026");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn empty_database_pages() {
    let h = Opts::new().db(Db::Empty).path("/").launch();
    assert_eq!(h.status(), 200);
    // Missing page without any stored weeks: the message and a calendar without planned weeks.
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    assert_eq!(h.js("document.querySelectorAll('.week-list').length"), 0);
    assert_eq!(h.js("document.querySelectorAll('a.all-weeks').length"), 0);
    assert_eq!(h.cal_heading(), "December 2026");
    assert_eq!(h.js("document.querySelectorAll('.cal-planned').length"), 0);
    // The current year page is a 200 without plans.
    h.goto_page("/2026");
    assert_eq!(h.status(), 200);
    assert_eq!(h.texts(".empty"), ["No meal plans available yet."]);
    assert_eq!(h.js("document.querySelectorAll('.week-link').length"), 0);
    assert_eq!(h.texts("a.all-weeks"), ["This week"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn invalid_week_page_and_zero_padded_redirect() {
    let h = Harness::start("", 1280);
    h.goto_page("/2025/53");
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["Invalid week."]);
    assert_eq!(h.texts("a.nav-link"), ["This week"]);
    assert_eq!(h.text(".title"), "Meal Plan");

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
fn invalid_date_page() {
    let h = Harness::start("", 1280);
    for path in ["/2026-02-30", "/0000-01-01", "/9999-12-31"] {
        h.goto_page(path);
        assert_eq!(h.status(), 404, "{path}");
        assert_eq!(h.pathname(), path);
        assert_eq!(h.texts(".empty"), ["Invalid date."], "{path}");
        assert_eq!(h.texts("a.nav-link"), ["This week"], "{path}");
        assert_eq!(h.text(".title"), "Meal Plan", "{path}");
        assert_eq!(h.js("document.title"), "Invalid date — Meal Plan", "{path}");
        assert_eq!(h.count(".day-row, .day-heading, .day-step"), 0, "{path}");
        // The calendar is there, on today's month (December 2, 2026).
        assert_eq!(h.cal_heading(), "December 2026", "{path}");
        assert_eq!(h.cal_today_cells(), ["2"], "{path}");
    }
    h.click("a.nav-link[href='/']");
    h.wait_page("/");
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);

    // A malformed date is not a date path at all: a plain 404 without the app.
    h.navigate("/2026-1-5");
    assert_eq!(h.status(), 404);
    assert_eq!(h.count("#sous-page"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn database_failure_shows_unavailable_not_missing() {
    for mode in [Db::NoTables, Db::ClosedPool] {
        let h = Opts::new().db(mode).path("/").launch();
        for path in ["/", "/2026/44", "/2026", "/2026-10-28", "/2026-02-30"] {
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
    // Monday (UTC+14) and Sunday (UTC-11) at that instant; neither may influence the shown day.
    let h = Opts::new()
        .clock(clock_at(2026, 11, 1, 22, 30, 0))
        .path("/")
        .launch();
    for tz in ["Pacific/Kiritimati", "Pacific/Pago_Pago"] {
        h.set_browser_timezone(tz);
        h.goto("/");
        assert_eq!(h.pathname(), "/");
        assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
        assert_eq!(h.heading(), "Sunday, November 1", "{tz}");
        // The same week at its permanent URL shows today too.
        h.goto("/2026/44");
        assert_eq!(h.heading(), "Sunday, November 1", "{tz}");
        assert_eq!(h.count(".tabs, .tab-button, .days, .day-button"), 0);
    }

    // The day changes at Berlin midnight (2026-10-27 23:00Z -> Wednesday), not at the browser's.
    for (clock, expected) in [
        (clock_at(2026, 10, 27, 22, 59, 59), "Tuesday, October 27"),
        (clock_at(2026, 10, 27, 23, 0, 0), "Wednesday, October 28"),
    ] {
        let h = Opts::new().clock(clock).path("/").launch();
        for tz in ["Pacific/Kiritimati", "Pacific/Pago_Pago"] {
            h.set_browser_timezone(tz);
            h.goto("/");
            assert_eq!(h.heading(), expected, "{tz}");
        }
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn year_boundary_week_shows_saturday() {
    // 2027-01-02 is a Saturday in ISO 2026-W53.
    let sql = bare_weeks(&[(2026, 53)]);
    let h = Opts::new()
        .sql(&sql)
        .clock(clock_at(2027, 1, 2, 12, 0, 0))
        .path("/")
        .launch();
    assert_eq!(h.pathname(), "/");
    assert_eq!(h.text(".title"), "Meal Plan: Dec 28 – Jan 3");
    assert_eq!(h.heading(), "Saturday, January 2");
    // The day links cross the year boundary.
    assert_eq!(
        h.js("document.querySelector(\"a[aria-label='Previous day']\").getAttribute('href')"),
        "/2027-01-01"
    );
    assert_eq!(
        h.js("document.querySelector(\"a[aria-label='Next day']\").getAttribute('href')"),
        "/2027-01-03"
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn list_pages_fit_375px() {
    let sql = bare_weeks(&[(2026, 53), (2027, 1)]);
    let h = Opts::new().sql(&sql).width(375).launch();
    for path in ["/2026", "/2026/52", "/2025/53", "/", "/2026-02-30"] {
        h.goto_page(path);
        assert_eq!(h.js("window.innerWidth"), 375);
        assert_eq!(
            h.js("document.documentElement.scrollWidth <= window.innerWidth"),
            true,
            "{path}"
        );
    }
    // A year page with all three header links fits too.
    h.exec(&bare_weeks(&[(2025, 1), (2027, 3)]));
    h.goto_page("/2026");
    assert_eq!(h.texts("a.all-weeks").len(), 3);
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= window.innerWidth"),
        true
    );
    // The plan page fits closed and open.
    h.goto("/2026/44");
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= window.innerWidth"),
        true
    );
    h.click(".calendar-toggle");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
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

// ---- Feature 3: shopping lists -------------------------------------------------------------

const L1_LABEL: &str = "🛒 Monday Shop → covers Mon–Wed";
const L1_NOTE: &str = "You have: salmon, oats, walnuts, almonds, chia, miso, spices.";
const L2_LABEL: &str = "🧴 Household";
const L1_CATEGORIES: [&str; 3] = ["Produce", "Dairy / Chilled", "Bakery"];
const L1_ITEMS: [&str; 8] = [
    "1 cucumber",
    "1 courgette",
    "1 bag baby spinach (200g)",
    "200g mixed berries",
    "Fresh ginger",
    "2L oat milk",
    "Skyr (400g)",
    "1 loaf dark rye bread",
];
const L1_ID: &str = "00000005-0000-4000-8000-00000000001f";

/// SQL for ISO 2026-W`week` with a Monday linked to one shopping list:
/// (label, note, [(category, [item])]).
fn shop_week(week: u32, label: &str, note: Option<&str>, cats: &[(&str, &[&str])]) -> String {
    let q = |s: &str| s.replace('\'', "''");
    let base = 1000 * week;
    let list = id(0xc5, base);
    let mut sql = format!(
        "INSERT INTO weeks (id, iso_year, iso_week) VALUES ('{}', 2026, {week});\n\
         INSERT INTO days (id, week_id, weekday) VALUES ('{}', '{}', 1);\n\
         INSERT INTO shopping_lists (id, week_id, position, label, note) VALUES ('{list}', '{}', 0, '{}', {});\n\
         INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) VALUES ('{}', '{list}', 0);\n",
        id(0xa1, base),
        id(0xa2, base),
        id(0xa1, base),
        id(0xa1, base),
        q(label),
        note.map_or("NULL".to_string(), |n| format!("'{}'", q(n))),
        id(0xa2, base),
    );
    for (ci, (name, items)) in cats.iter().enumerate() {
        let cat = id(0xc6, base + ci as u32);
        sql += &format!(
            "INSERT INTO shopping_categories (id, shopping_list_id, position, name) VALUES ('{cat}', '{list}', {ci}, '{}');\n",
            q(name)
        );
        for (ii, item) in items.iter().enumerate() {
            sql += &format!(
                "INSERT INTO shopping_items (id, category_id, position, text) VALUES ('{}', '{cat}', {ii}, '{}');\n",
                id(0xc7, base + (ci * 100 + ii) as u32),
                q(item)
            );
        }
    }
    sql
}

impl Harness {
    fn expand_all_shops(&self) {
        self.js("document.querySelectorAll('.shop-toggle[aria-expanded=false]').forEach(b => b.click())");
        self.wait_for(
            "[...document.querySelectorAll('.shop-toggle')].every(b => b.getAttribute('aria-expanded') === 'true')",
        );
    }

    fn count(&self, selector: &str) -> u64 {
        self.js(&format!("document.querySelectorAll('{selector}').length"))
            .as_u64()
            .unwrap()
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn monday_shows_two_shopping_cards_before_summary() {
    let h = Harness::start("", 1280);
    assert_eq!(h.texts(".shop-label"), [L2_LABEL, L1_LABEL]);
    assert_eq!(h.count(".shop"), 2);
    assert_eq!(
        h.js(
            "[...document.querySelectorAll('.shop')].every(s => \
             (s.compareDocumentPosition(document.querySelector('.summary')) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0)"
        ),
        true
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shared_list_appears_on_wednesday_and_unassigned_nowhere() {
    let h = Harness::start("", 1280);
    h.expand_all_shops();
    let monday_l1 = h.js(&format!(
        "document.getElementById('shop-{L1_ID}').textContent"
    ));

    h.goto("/2026-10-28");
    assert_eq!(h.texts(".shop-label"), [L1_LABEL]);
    assert_eq!(h.texts(".shop-note"), [L1_NOTE]);
    h.click(".shop-toggle");
    h.wait_for("document.querySelector('.shop-body') !== null");
    assert_eq!(h.texts(".shop-cat-name"), L1_CATEGORIES);
    assert_eq!(h.texts(".shop-items li"), L1_ITEMS);
    assert_eq!(
        h.js(&format!(
            "document.getElementById('shop-{L1_ID}').textContent"
        )),
        monday_l1
    );
    // The card comes before the empty-day message.
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    assert_eq!(
        h.js(
            "(document.querySelector('.shop').compareDocumentPosition(document.querySelector('.empty')) \
             & Node.DOCUMENT_POSITION_FOLLOWING) !== 0"
        ),
        true
    );

    // Tuesday and Sunday have no cards, and no day shows the unassigned list.
    for (day, date) in W44_DATES.iter().enumerate() {
        h.goto(&format!("/{date}"));
        if day == 1 || day == 6 {
            assert_eq!(h.count(".shop"), 0, "day {day}");
        }
        assert!(
            !h.texts(".shop-label")
                .iter()
                .any(|t| t.contains("Thursday Top-Up")),
            "day {day}"
        );
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_content_matches_fixture_and_reloads_changes() {
    let h = Harness::start("", 1280);
    // Collapsed: the note is exactly the stored note; the note-less card has no note element.
    assert_eq!(h.texts(".shop-note"), [L1_NOTE]);
    assert_eq!(
        h.js("document.querySelectorAll('.shop')[0].querySelector('.shop-note')"),
        serde_json::Value::Null
    );
    assert_eq!(h.count(".shop-body"), 0);

    h.expand_all_shops();
    assert_eq!(
        h.texts(".shop-cat-name"),
        ["Cleaning", "Spices", "Produce", "Dairy / Chilled", "Bakery"]
    );
    let mut items = vec!["Dish soap"];
    items.extend(L1_ITEMS);
    assert_eq!(h.texts(".shop-items li"), items);

    h.exec(&format!(
        "UPDATE shopping_items SET text = 'Fresh ginger root' WHERE text = 'Fresh ginger';\n\
         UPDATE shopping_lists SET note = 'Changed note.' WHERE id = '{L1_ID}';"
    ));
    h.goto("/2026/44");
    assert_eq!(h.texts(".shop-note"), ["Changed note."]);
    h.expand_all_shops();
    let items = h.texts(".shop-items li");
    assert!(items.contains(&"Fresh ginger root".to_string()));
    assert!(!items.contains(&"Fresh ginger".to_string()));
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_items_are_plain_bullets() {
    let h = Harness::start("", 1280);
    h.expand_all_shops();
    assert_eq!(h.count("input[type=checkbox], [role=checkbox]"), 0);
    assert_eq!(
        h.js("/\\d+\\/\\d+ items/.test(document.body.textContent)"),
        false
    );
    assert_eq!(
        h.js("[...document.querySelectorAll('.shop li')].every(li => li.parentElement.tagName === 'UL' \
              && getComputedStyle(li).listStyleType === 'disc')"),
        true
    );
    assert_eq!(
        h.js("[...document.querySelectorAll('.shop li')].some(li => getComputedStyle(li).textDecorationLine.includes('line-through'))"),
        false
    );
    let probe = "(() => { const s = getComputedStyle(document.querySelector('.shop li')); \
                 return [s.backgroundColor, s.textDecorationLine, s.cursor]; })()";
    let before = h.js(probe);
    h.click(".shop li");
    assert_eq!(h.js(probe), before);
    assert_eq!(before[2], "auto");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_empty_states() {
    let sql = shop_week(45, "Empty list", None, &[]);
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/45");
    assert_eq!(h.count(".shop-body"), 0);
    h.click(".shop-toggle");
    h.wait_for("document.querySelector('.shop-body') !== null");
    assert_eq!(h.text(".shop-empty"), "No shopping items.");
    assert_eq!(h.count(".shop-cat"), 0);

    // The fixture's empty `Spices` category shows only its heading.
    h.goto("/2026/44");
    h.expand_all_shops();
    assert_eq!(
        h.js("(() => { const c = [...document.querySelectorAll('.shop-cat')].find(c => c.querySelector('.shop-cat-name').textContent === 'Spices'); \
              return [c.querySelectorAll('li').length, c.children.length]; })()"),
        serde_json::json!([0, 1])
    );
    assert_eq!(h.count(".shop-empty"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_pointer_toggle() {
    let h = Harness::start("", 1280);
    let expanded = "document.querySelector('.shop-toggle').getAttribute('aria-expanded')";
    let indicator = "document.querySelector('.shop-indicator').textContent";
    assert_eq!(h.js(expanded), "false");
    assert_eq!(h.js(indicator), "+");
    assert_eq!(h.count(".shop-body"), 0);

    h.click(".shop-toggle");
    h.wait_for(&format!("{expanded} === 'true'"));
    assert_eq!(h.js(indicator), "−");
    assert_eq!(h.count(".shop-body"), 1);
    assert_eq!(
        h.js("document.querySelector('.shop-toggle').getAttribute('aria-controls') === document.querySelector('.shop-body').id"),
        true
    );
    assert_eq!(
        h.js("document.querySelector('.shop-body').getBoundingClientRect().height > 0"),
        true
    );

    h.click(".shop-toggle");
    h.wait_for(&format!("{expanded} === 'false'"));
    assert_eq!(h.js(indicator), "+");
    assert_eq!(h.count(".shop-body"), 0);

    // Expansion resets when the day changes.
    h.click(".shop-toggle");
    h.wait_for(&format!("{expanded} === 'true'"));
    h.step_day("Next day", "/2026-10-27");
    h.step_day("Previous day", "/2026-10-26");
    assert_eq!(h.count(".shop-body"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_keyboard_toggle() {
    let h = Harness::start("", 1280);
    // Tab from the "Next day" link reaches the first shopping toggle.
    h.js("document.querySelector(\"a[aria-label='Next day']\").focus()");
    h.press("Tab");
    assert_eq!(
        h.js("document.activeElement === document.querySelector('.shop-toggle')"),
        true
    );
    let expanded = "document.querySelector('.shop-toggle').getAttribute('aria-expanded')";
    h.press("Enter");
    h.wait_for(&format!("{expanded} === 'true'"));
    h.press("Enter");
    h.wait_for(&format!("{expanded} === 'false'"));
    h.press(" ");
    h.wait_for(&format!("{expanded} === 'true'"));
    h.press(" ");
    h.wait_for(&format!("{expanded} === 'false'"));
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_text_renders_literally() {
    let sql = shop_week(46, XSS_NAME, Some(XSS_NAME), &[(XSS_NAME, &[XSS_NAME])]);
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/46");
    h.click(".shop-toggle");
    h.wait_for("document.querySelector('.shop-body') !== null");
    assert_eq!(h.text(".shop-label"), XSS_NAME);
    assert_eq!(h.text(".shop-note"), XSS_NAME);
    assert_eq!(h.text(".shop-cat-name"), XSS_NAME);
    assert_eq!(h.text(".shop li"), XSS_NAME);
    assert_eq!(h.js("typeof window.pwned"), "undefined");
    assert_eq!(h.count(".shop b"), 0);
}

fn assert_shop_layout(width: u32) {
    let h = Harness::start("", width);
    h.expand_all_shops();
    assert_eq!(h.js("window.innerWidth"), width);
    let report = h.js(
        "(() => { const app = document.querySelector('.app').getBoundingClientRect(); \
          return { fits: [...document.querySelectorAll('.shop')].every(s => { const r = s.getBoundingClientRect(); \
            return r.left >= app.left - 0.5 && r.right <= app.right + 0.5 && r.width <= 540; }), \
            appWidth: app.width, \
            noScroll: document.documentElement.scrollWidth <= window.innerWidth, \
            bg: getComputedStyle(document.querySelector('.shop')).backgroundColor, \
            border: getComputedStyle(document.querySelector('.shop')).borderTopColor }; })()",
    );
    assert_eq!(report["fits"], true);
    assert!(report["appWidth"].as_f64().unwrap() <= 540.0);
    assert_eq!(report["noScroll"], true);
    assert_eq!(report["bg"], "rgb(255, 240, 246)");
    assert_eq!(report["border"], "rgb(251, 182, 206)");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_layout_at_375px() {
    assert_shop_layout(375);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_layout_at_1280px() {
    assert_shop_layout(1280);
}

// ---- Feature 4: prep, reminders --------------------------------------------------

/// Content for a crafted week (ISO 2026-W`week`) whose Monday carries a reminder.
struct Content<'a> {
    subtitle: Option<&'a str>,
    intro: Option<&'a str>,
    /// (title, body, icon)
    reminder: Option<(&'a str, Option<&'a str>, Option<&'a str>)>,
    /// (title, covers, [(name, time, instructions)])
    session: Option<(&'a str, Option<&'a str>, [&'a str; 3])>,
}

fn content_week(week: u32, c: &Content) -> String {
    let lit = |s: Option<&str>| {
        s.map_or("NULL".to_string(), |s| {
            format!("'{}'", s.replace('\'', "''"))
        })
    };
    let base = 1000 * week;
    let (rt, rb, ri) = c
        .reminder
        .map_or((None, None, None), |(t, b, i)| (Some(t), b, i));
    let mut sql = format!(
        "INSERT INTO weeks (id, iso_year, iso_week, subtitle, prep_intro) VALUES ('{}', 2026, {week}, {}, {});\n\
         INSERT INTO days (id, week_id, weekday, reminder_title, reminder_body, reminder_icon) \
         VALUES ('{}', '{}', 1, {}, {}, {});\n",
        id(0xa1, base),
        lit(c.subtitle),
        lit(c.intro),
        id(0xa2, base),
        id(0xa1, base),
        lit(rt),
        lit(rb),
        lit(ri),
    );
    if let Some((title, covers, [name, time, instructions])) = c.session {
        sql += &format!(
            "INSERT INTO prep_sessions (id, week_id, position, title, covers, weekday) VALUES ('{}', '{}', 0, {}, {}, 1);\n\
             INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) \
             VALUES ('{}', '{}', 0, {}, {}, {});\n",
            id(0xd1, base),
            id(0xa1, base),
            lit(Some(title)),
            lit(covers),
            id(0xd2, base),
            id(0xd1, base),
            lit(Some(name)),
            lit(Some(time)),
            lit(Some(instructions)),
        );
    }
    sql
}

impl Harness {
    /// Computed styles of the first element matching `selector`, as `[property, ...]` values.
    fn styles(&self, selector: &str, props: &[&str]) -> Vec<String> {
        let v = self.js(&format!(
            "(() => {{ const s = getComputedStyle(document.querySelector('{selector}')); \
             return {props:?}.map(p => s[p]); }})()"
        ));
        serde_json::from_value(v).unwrap()
    }
}

#[test]
fn app_jsx_has_no_sample_content() {
    let src = include_str!("../web/app.jsx");
    for sample in [
        "Salmon week",
        "Sunday covers",
        "Weekly Variety",
        "Air Fryer Features",
        "Salmon Plan",
        "This Week",
        "Time for next week",
        "Ask Claude",
        "Oily fish",
    ] {
        assert!(!src.contains(sample), "app.jsx hardcodes {sample:?}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn no_tabs_or_day_bar() {
    let h = Harness::start("", 1280);
    for path in ["/2026-10-26", "/2026-10-28", "/2026-10-30", "/2026/44"] {
        h.goto(path);
        assert_eq!(
            h.count(".tabs, .tab-button, .days, .day-button"),
            0,
            "{path}"
        );
        assert_eq!(h.count("button[aria-pressed]"), 0, "{path}");
        let text = h.text(".app");
        for banned in ["Daily Plan", "Prep Sessions", "No prep sessions planned"] {
            assert!(!text.contains(banned), "{path}: {text}");
        }
    }
}

#[test]
fn app_sources_have_no_tabs_or_day_bar() {
    let jsx = include_str!("../web/app.jsx");
    for banned in [
        "tab-button",
        "day-button",
        "aria-pressed",
        "Daily Plan",
        "Prep Sessions",
        "No prep sessions planned",
        "selectedWeekday",
    ] {
        assert!(!jsx.contains(banned), "app.jsx still has {banned:?}");
    }
    let css = include_str!("../web/styles.css");
    for banned in [".tabs", ".tab-button", ".days", ".day-button"] {
        assert!(!css.contains(banned), "styles.css still has {banned:?}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn plan_page_order_is_day_row_shopping_prep_summary_meals_reminder() {
    let h = Harness::start("", 1280);
    let order = "[...document.querySelector('.app').children].map(e => e.className.split(' ')[0])";
    let monday = [
        "header",
        "calendar",
        "day-row",
        "shop",
        "shop",
        "prep-intro",
        "prep",
        "summary",
        "meal",
        "meal",
        "meal",
        "meal",
    ];
    h.goto("/2026-10-26");
    assert_eq!(h.js(order), serde_json::json!(monday));
    // The reminder of Monday has no title and is not shown.
    h.goto("/2026-10-28");
    assert_eq!(
        h.js(order),
        serde_json::json!([
            "header",
            "calendar",
            "day-row",
            "shop",
            "prep-intro",
            "prep",
            "empty",
            "reminder"
        ])
    );
    assert_eq!(h.text(".reminder-title"), "Shop day");

    // The prep card of Monday expands and collapses with the pointer and the keyboard,
    // and shows every task's name, time and instructions.
    h.goto("/2026-10-26");
    let expanded = "document.querySelector('.prep-toggle').getAttribute('aria-expanded')";
    assert_eq!(h.js(expanded), "false");
    assert_eq!(h.count(".prep-body"), 0);
    h.click(".prep-toggle");
    h.wait_for(&format!("{expanded} === 'true'"));
    assert_eq!(h.count(".prep-task"), 3);
    assert_eq!(h.count(".prep-task-name"), 3);
    assert_eq!(h.count(".prep-task-time"), 3);
    assert_eq!(h.count(".prep-task-instructions"), 3);
    h.click(".prep-toggle");
    h.wait_for(&format!("{expanded} === 'false'"));
    assert_eq!(h.count(".prep-body"), 0);
    h.js("document.querySelector('.prep-toggle').focus()");
    h.press("Enter");
    h.wait_for(&format!("{expanded} === 'true'"));
    h.press(" ");
    h.wait_for(&format!("{expanded} === 'false'"));
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn prep_sessions_render_and_toggle() {
    let h = Harness::start("", 1280);
    assert_eq!(
        h.texts(".prep-title"),
        ["🔪 Monday Prep Session (~1.5 hours)"]
    );
    // Covers: present for Monday.
    assert_eq!(h.texts(".prep-covers"), ["Covers Monday through Wednesday"]);
    assert_eq!(h.count(".prep-body"), 0);
    // Wednesday shows its own session, which has no covers element at all.
    h.goto("/2026-10-28");
    assert_eq!(
        h.texts(".prep-title"),
        ["🔪 Wednesday Evening Prep (~45 min)"]
    );
    assert_eq!(h.count(".prep-covers"), 0);
    assert_eq!(
        h.text(".prep-intro"),
        "Monday covers Mon–Wed. Wednesday is a smaller session for Thu–Sun."
    );
    h.click(".prep-toggle");
    h.wait_for("document.querySelectorAll('.prep-task').length === 2");
    h.goto("/2026-10-26");

    let toggle = ".prep-toggle";
    let expanded = "document.querySelector('.prep-toggle').getAttribute('aria-expanded')";
    assert_eq!(h.js(expanded), "false");
    h.click(toggle);
    h.wait_for(&format!("{expanded} === 'true'"));
    assert_eq!(
        h.texts(".prep-task-name"),
        [
            "Start the Gravlax",
            "Air-Fryer Roasted Chickpeas",
            "Cook Brown Rice"
        ]
    );
    assert_eq!(h.texts(".prep-task-time"), ["15 min", "20 min", "25 min"]);
    assert_eq!(
        h.texts(".prep-task-instructions")[1],
        "Toss chickpeas with oil and paprika; air-fry at 200°C for 12–15 min."
    );
    assert_eq!(
        h.js("document.querySelector('.prep-toggle').getAttribute('aria-controls') === document.querySelector('.prep-body').id"),
        true
    );

    assert_eq!(
        h.styles(
            ".prep",
            &[
                "backgroundColor",
                "borderTopColor",
                "borderRadius",
                "padding",
                "marginBottom"
            ]
        ),
        [
            "rgb(255, 251, 235)",
            "rgb(253, 230, 138)",
            "10px",
            "16px",
            "12px"
        ]
    );
    let title = h.styles(
        ".prep-title",
        &["fontSize", "fontWeight", "color", "fontFamily"],
    );
    assert_eq!(title[..3], ["16px", "700", "rgb(146, 64, 14)"]);
    assert!(title[3].contains("Source Serif 4"));
    assert_eq!(
        h.styles(".prep-covers", &["fontSize", "color"]),
        ["12px", "rgb(180, 83, 9)"]
    );
    let badge = h.styles(
        ".prep-task-time",
        &[
            "fontSize",
            "fontWeight",
            "color",
            "backgroundColor",
            "padding",
            "borderRadius",
            "fontFamily",
        ],
    );
    assert_eq!(
        badge[..6],
        [
            "10px",
            "600",
            "rgb(146, 64, 14)",
            "rgb(254, 243, 199)",
            "2px 6px",
            "4px"
        ]
    );
    assert!(badge[6].contains("JetBrains Mono"));
    assert_eq!(
        h.styles(
            ".prep-task",
            &["backgroundColor", "borderTopColor", "borderRadius"]
        ),
        ["rgb(255, 255, 255)", "rgb(254, 243, 199)", "8px"]
    );
    assert_eq!(
        h.styles(".prep-task-name", &["fontSize", "fontWeight", "color"]),
        ["13px", "600", "rgb(30, 41, 59)"]
    );
    assert_eq!(
        h.styles(
            ".prep-task-instructions",
            &["fontSize", "color", "lineHeight"]
        ),
        ["12px", "rgb(71, 85, 105)", "19.2px"]
    );

    h.click(toggle);
    h.wait_for(&format!("{expanded} === 'false'"));
    assert_eq!(h.count(".prep-body"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn prep_card_keyboard_toggle() {
    let h = Harness::start("", 1280);
    let expanded = "document.querySelector('.prep-toggle').getAttribute('aria-expanded')";
    h.js("document.querySelector('.prep-toggle').focus()");
    h.press("Enter");
    h.wait_for(&format!("{expanded} === 'true'"));
    h.press("Enter");
    h.wait_for(&format!("{expanded} === 'false'"));
    h.press(" ");
    h.wait_for(&format!("{expanded} === 'true'"));
    h.press(" ");
    h.wait_for(&format!("{expanded} === 'false'"));
}

#[test]
fn app_sources_have_no_strategy_text() {
    for (name, src) in [
        ("app.jsx", include_str!("../web/app.jsx")),
        ("styles.css", include_str!("../web/styles.css")),
    ] {
        assert!(
            !src.to_lowercase().contains("strateg"),
            "{name} still mentions strategy"
        );
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn page_has_no_strategy_remnants() {
    let h = Harness::start("", 1280);
    let check = |h: &Harness, what: &str| {
        assert_eq!(h.count("[class*=strategy]"), 0, "{what}: strategy element");
        let text = h.text(".app");
        assert!(!text.to_lowercase().contains("strategy"), "{what}: {text}");
    };
    for date in W44_DATES {
        h.goto(&format!("/{date}"));
        check(&h, date);
    }
    let page = "JSON.parse(document.getElementById('sous-page').textContent).document";
    assert_eq!(h.js(&format!("'strategyCards' in {page}")), false);
    assert_eq!(h.js(&format!("{page}.schemaVersion")), 3);
    let selectors = h.js(
        "[...document.styleSheets].flatMap(s => [...s.cssRules]).map(r => r.selectorText || '')\
         .filter(s => s.toLowerCase().includes('strategy'))",
    );
    assert_eq!(selectors, serde_json::json!([]));
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_013() {
    let h = Opts::new().width(375).path("/2026-10-26").launch();
    h.capture_file("013-day-375-closed.png");
    h.click(".calendar-toggle");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    h.capture_file("013-day-375-open.png");
    let h = Opts::new().width(1280).path("/2026-10-26").launch();
    h.capture_file("013-day-1280.png");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn reminders_show_only_on_days_with_a_title() {
    let h = Harness::start("", 1280);
    // Monday has a body but no title: no reminder.
    assert_eq!(h.count(".reminder"), 0);

    // Wednesday: no meals, so the empty message comes first, then the reminder.
    h.goto("/2026-10-28");
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    assert_eq!(h.text(".reminder-icon"), "🧾");
    assert_eq!(h.text(".reminder-title"), "Shop day");
    assert_eq!(
        h.text(".reminder-body"),
        "Pick up the Thursday top-up list after work."
    );
    assert_eq!(
        h.js("!!(document.querySelector('.empty').compareDocumentPosition(document.querySelector('.reminder')) & Node.DOCUMENT_POSITION_FOLLOWING)"),
        true
    );
    let r = h.styles(
        ".reminder",
        &[
            "backgroundImage",
            "borderTopColor",
            "borderRadius",
            "padding",
            "marginTop",
            "display",
        ],
    );
    assert_eq!(
        r[0],
        "linear-gradient(135deg, rgb(239, 246, 255), rgb(219, 234, 254))"
    );
    assert_eq!(r[1], "rgb(147, 197, 253)");
    assert_eq!(r[2], "10px");
    assert_eq!(r[3], "14px 16px");
    assert_eq!(r[4], "8px");
    assert_eq!(r[5], "flex");
    assert_eq!(h.styles(".reminder-icon", &["fontSize"]), ["20px"]);
    assert_eq!(
        h.styles(".reminder-title", &["fontSize", "fontWeight", "color"]),
        ["13px", "600", "rgb(30, 64, 175)"]
    );
    assert_eq!(
        h.styles(".reminder-body", &["fontSize", "color", "marginTop"]),
        ["12px", "rgb(59, 130, 246)", "2px"]
    );

    // Sunday: title only, no icon or body elements.
    h.goto("/2026-11-01");
    assert_eq!(h.text(".reminder-title"), "Plan next week");
    assert_eq!(h.count(".reminder-icon"), 0);
    assert_eq!(h.count(".reminder-body"), 0);

    // Tuesday (no reminder columns), Saturday (no day row).
    for date in ["2026-10-27", "2026-10-31"] {
        h.goto(&format!("/{date}"));
        assert_eq!(h.count(".reminder"), 0, "{date}");
    }
    // Monday has meals: nothing reminder-like after them either.
    h.goto("/2026-10-26");
    assert_eq!(h.count(".reminder"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn optional_text_is_omitted_when_null() {
    let sql = content_week(
        48,
        &Content {
            subtitle: None,
            intro: None,
            reminder: Some(("Only a title", None, None)),
            session: Some(("Bare session", None, ["Task", "5 min", "Do it."])),
        },
    );
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/48");
    assert_eq!(h.text(".reminder-title"), "Only a title");
    assert_eq!(h.count(".reminder-body"), 0);
    assert_eq!(h.count(".reminder-icon"), 0);
    assert_eq!(h.count(".prep-intro"), 0);
    assert_eq!(h.count(".prep-covers"), 0);
    assert_eq!(h.text(".prep-title"), "🔪 Bare session");
}

/// SQL adding prep sessions (title, has_task, weekday) at positions 0.. to the week made by
/// `crafted_week(week, ..)`.
fn prep_sessions_sql(week: u32, sessions: &[(&str, bool, Option<u32>)]) -> String {
    let base = 1000 * week;
    let mut sql = String::new();
    for (i, (title, has_task, weekday)) in sessions.iter().enumerate() {
        let sid = id(0xe1, base + i as u32);
        sql += &format!(
            "INSERT INTO prep_sessions (id, week_id, position, title, weekday) VALUES ('{sid}', '{}', {i}, '{title}', {});\n",
            id(0xa1, base),
            weekday.map_or("NULL".to_string(), |d| d.to_string())
        );
        if *has_task {
            sql += &format!(
                "INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) \
                 VALUES ('{}', '{sid}', 0, 'Task {i}', '5 min', 'Do it.');\n",
                id(0xe2, base + i as u32)
            );
        }
    }
    sql
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn prep_sessions_without_tasks_are_hidden() {
    let sql = crafted_week(52, &[])
        + &prep_sessions_sql(
            52,
            &[
                ("Session A", true, Some(1)),
                ("Session B", false, Some(1)),
                ("Session C", true, Some(1)),
            ],
        );
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/52");
    assert_eq!(h.texts(".prep-title"), ["🔪 Session A", "🔪 Session C"]);
    assert_eq!(
        h.js("document.getElementById('root').textContent.includes('Session B')"),
        false
    );

    // A day whose only sessions have no tasks shows neither the sessions nor the week's intro.
    let sql = crafted_week(53, &[])
        + &prep_sessions_sql(
            53,
            &[
                ("Taskless one", false, Some(1)),
                ("Taskless two", false, Some(1)),
            ],
        )
        + "UPDATE weeks SET prep_intro = 'An intro' WHERE iso_week = 53;";
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/53");
    assert_eq!(h.count(".prep"), 0);
    assert_eq!(h.count(".prep-intro"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn prep_intro_only_on_days_with_shown_sessions() {
    let h = Harness::start("", 1280);
    for (i, date) in W44_DATES.iter().enumerate() {
        h.goto(&format!("/{date}"));
        let shown = i == 0 || i == 2;
        assert_eq!(h.count(".prep-intro"), u64::from(shown), "{date}");
        assert_eq!(h.count(".prep") > 0, shown, "{date}");
        assert!(
            !h.text(".app").contains("No prep sessions planned"),
            "{date}"
        );
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn sessions_without_a_weekday_or_tasks_are_never_shown_and_marks_match() {
    let sql = crafted_week(47, &[])
        + &prep_sessions_sql(
            47,
            &[
                ("No day", true, None),
                ("No tasks", false, Some(1)),
                ("Tuesday prep", true, Some(2)),
            ],
        );
    let h = Harness::start(&sql, 1280);
    for date in [
        "2026-11-16",
        "2026-11-17",
        "2026-11-18",
        "2026-11-19",
        "2026-11-20",
        "2026-11-21",
        "2026-11-22",
    ] {
        h.goto(&format!("/{date}"));
        let text = h.text(".app");
        assert!(
            !text.contains("No day") && !text.contains("No tasks"),
            "{date}"
        );
        if date == "2026-11-17" {
            assert_eq!(h.texts(".prep-title"), ["🔪 Tuesday prep"]);
        } else {
            assert_eq!(h.count(".prep"), 0, "{date}");
        }
    }
    // The November calendar marks the fixture's two days and the Tuesday.
    let marked: Vec<String> = h.cal_marked().into_iter().map(|(href, _)| href).collect();
    assert_eq!(marked, ["/2026-10-26", "/2026-10-28", "/2026-11-17"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn crafted_week_content_is_what_renders() {
    let sql = content_week(
        50,
        &Content {
            subtitle: Some("Crafted subtitle"),
            intro: Some("Crafted intro"),
            reminder: Some(("Crafted reminder", Some("Crafted body"), Some("🔔"))),
            session: Some((
                "Crafted session",
                Some("Crafted covers"),
                ["Crafted task", "9 min", "Crafted steps."],
            )),
        },
    );
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/50");
    assert_eq!(h.text(".subtitle"), "Crafted subtitle");
    assert_eq!(h.text(".reminder-title"), "Crafted reminder");
    assert_eq!(h.text(".reminder-body"), "Crafted body");
    assert_eq!(h.text(".reminder-icon"), "🔔");
    assert_eq!(h.text(".prep-intro"), "Crafted intro");
    assert_eq!(h.text(".prep-title"), "🔪 Crafted session");
    assert_eq!(h.text(".prep-covers"), "Crafted covers");
    h.click(".prep-toggle");
    h.wait_for("document.querySelector('.prep-body') !== null");
    assert_eq!(h.text(".prep-task-name"), "Crafted task");
    assert_eq!(h.text(".prep-task-time"), "9 min");
    assert_eq!(h.text(".prep-task-instructions"), "Crafted steps.");
    let body = h.text(".app");
    for sample in ["Sunday covers", "Weekly Variety", "Salmon"] {
        assert!(!body.contains(sample), "{sample}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn weekly_content_reloads_committed_changes() {
    let h = Harness::start("", 1280);
    h.exec(
        "UPDATE weeks SET prep_intro = 'Changed intro' WHERE iso_week = 44;\n\
         UPDATE prep_tasks SET name = 'Start the Gravlax today' WHERE name = 'Start the Gravlax';\n\
         UPDATE days SET reminder_title = 'Changed reminder' WHERE reminder_title = 'Shop day';",
    );
    h.goto("/2026-10-28");
    assert_eq!(h.text(".reminder-title"), "Changed reminder");
    assert_eq!(h.text(".prep-intro"), "Changed intro");
    h.goto("/2026-10-26");
    h.click(".prep-toggle");
    h.wait_for("document.querySelector('.prep-body') !== null");
    assert_eq!(h.texts(".prep-task-name")[0], "Start the Gravlax today");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn weekly_text_renders_literally() {
    let x = XSS_NAME;
    let sql = content_week(
        51,
        &Content {
            subtitle: Some(x),
            intro: Some(x),
            reminder: Some((x, Some(x), Some(x))),
            session: Some((x, Some(x), [x, x, x])),
        },
    );
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/51");
    assert_eq!(h.text(".reminder-title"), x);
    assert_eq!(h.text(".reminder-body"), x);
    assert_eq!(h.text(".reminder-icon"), x);
    assert_eq!(h.text(".prep-intro"), x);
    assert_eq!(h.text(".prep-title"), format!("🔪 {x}"));
    assert_eq!(h.text(".prep-covers"), x);
    h.click(".prep-toggle");
    h.wait_for("document.querySelector('.prep-body') !== null");
    assert_eq!(h.text(".prep-task-name"), x);
    assert_eq!(h.text(".prep-task-time"), x);
    assert_eq!(h.text(".prep-task-instructions"), x);
    assert_eq!(h.js("typeof window.pwned"), "undefined");
    assert_eq!(h.count(".app b"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn day_links_work_by_keyboard() {
    let h = Harness::start("", 1280);
    h.js("[...document.querySelectorAll('#calendar a')].pop().focus()");
    h.press("Tab");
    assert_eq!(
        h.js("document.activeElement === document.querySelector(\"a[aria-label='Previous day']\")"),
        true
    );
    h.press("Tab");
    assert_eq!(
        h.js("document.activeElement === document.querySelector(\"a[aria-label='Next day']\")"),
        true
    );
    h.press("Enter");
    h.wait_page("/2026-10-27");
    assert_eq!(h.heading(), "Tuesday, October 27");
}

fn assert_weekly_layout(width: u32) {
    let h = Harness::start("", width);
    let long = "x".repeat(120);
    h.exec(&format!(
        "UPDATE prep_tasks SET time = '{long}', name = '{long}', instructions = '{long}';"
    ));
    let fits = "document.documentElement.scrollWidth <= window.innerWidth";
    for date in ["2026-10-26", "2026-10-28"] {
        h.goto(&format!("/{date}"));
        assert_eq!(h.js(fits), true, "{date} closed");
        h.js("document.querySelectorAll('.prep-toggle').forEach(b => b.click())");
        h.wait_for("document.querySelectorAll('.prep-body').length === 1");
        assert_eq!(h.js(fits), true, "{date} prep expanded");
    }

    // The day row of the longest heading fits: one line, with both steps inside the column.
    h.exec(&bare_weeks(&[(2026, 40)]));
    h.goto("/2026-09-30");
    assert_eq!(h.heading(), "Wednesday, September 30");
    assert_eq!(h.js(fits), true, "longest heading");
    assert_eq!(
        h.js("(() => { const app = document.querySelector('.app').getBoundingClientRect(); \
              const row = [...document.querySelectorAll('.day-row > *')].map(e => e.getBoundingClientRect()); \
              const h = document.querySelector('.day-heading'); \
              return row.every(r => r.left >= app.left - 0.5 && r.right <= app.right + 0.5) \
                && h.getBoundingClientRect().height < 2 * parseFloat(getComputedStyle(h).fontSize); })()"),
        true
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn weekly_layout_at_375px() {
    assert_weekly_layout(375);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn weekly_layout_at_1280px() {
    assert_weekly_layout(1280);
}

// ---- Feature 5: no third-party requests, visual capture -----------------------------------

impl Harness {
    /// Every URL the current document requested (navigation + subresources).
    fn requested_urls(&self) -> Vec<String> {
        let v = self.js(
            "[...performance.getEntriesByType('navigation'), ...performance.getEntriesByType('resource')].map(e => e.name)",
        );
        serde_json::from_value(v).unwrap()
    }

    /// Assert the current document made only same-origin requests and embeds no foreign URL.
    fn assert_same_origin_only(&self, what: &str) {
        let base = self.base.as_str();
        for url in self.requested_urls() {
            assert!(url.starts_with(base), "{what}: third-party request {url}");
        }
        let html = self
            .js("document.documentElement.outerHTML")
            .as_str()
            .unwrap()
            .to_string();
        for (i, _) in html.match_indices("http") {
            let rest = &html[i..];
            if rest.starts_with("http://") || rest.starts_with("https://") {
                assert!(
                    rest.starts_with(base),
                    "{what}: foreign URL in HTML: {}",
                    rest.chars().take(60).collect::<String>()
                );
            }
        }
    }

    /// Click every collapsed toggle matching `selector`.
    fn expand_all(&self, selector: &str) {
        self.js(&format!(
            "document.querySelectorAll('{selector}[aria-expanded=false]').forEach(b => b.click())"
        ));
        self.wait_for(&format!(
            "[...document.querySelectorAll('{selector}')].every(b => b.getAttribute('aria-expanded') === 'true')"
        ));
    }

    /// Wait for fonts, then write a full-page PNG to `target/visual/<width>-<name>.png`.
    fn capture(&self, width: u32, name: &str) {
        self.capture_file(&format!("{width}-{name}.png"));
    }

    /// Wait for fonts, then write a full-page PNG to `target/visual/<file>`.
    fn capture_file(&self, file: &str) {
        use headless_chrome::protocol::cdp::Page::{
            CaptureScreenshot, CaptureScreenshotFormatOption,
        };
        self.tab
            .evaluate("document.fonts.ready.then(() => true)", true)
            .unwrap();
        let data = self
            .tab
            .call_method(CaptureScreenshot {
                format: Some(CaptureScreenshotFormatOption::Png),
                quality: None,
                clip: None,
                from_surface: Some(true),
                capture_beyond_viewport: Some(true),
                optimize_for_speed: None,
            })
            .unwrap()
            .data;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/visual");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, base64_decode(&data)).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 0, "{file}");
    }
}

/// Minimal base64 decoder (the crate has no base64 dev-dependency).
fn base64_decode(s: &str) -> Vec<u8> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn no_third_party_requests_on_any_page() {
    // Plan page: every day, every card expanded.
    let h = Opts::new().launch();
    for date in W44_DATES {
        h.goto(&format!("/{date}"));
        for toggle in [".meal-toggle", ".shop-toggle", ".prep-toggle"] {
            if h.count(toggle) > 0 {
                h.expand_all(toggle);
            }
        }
    }
    h.wait_for("[...document.fonts].some(f => f.status === 'loaded')");
    let urls = h.requested_urls();
    assert!(
        urls.iter()
            .any(|u| u.starts_with(&format!("{}/assets/", h.base)) && u.ends_with(".woff2")),
        "no woff2 from /assets/: {urls:?}"
    );
    h.assert_same_origin_only("/2026/44");

    // Other pages of the same server.
    for path in [
        "/2026",
        "/2026/45",
        "/2025/53",
        "/2026-11-11",
        "/2026-02-30",
    ] {
        h.goto_page(path);
        h.assert_same_origin_only(path);
    }

    // `/` with the clock inside W44.
    let h = Opts::new()
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/")
        .launch();
    h.assert_same_origin_only("/");

    // Unavailable page.
    let h = Opts::new().db(Db::ClosedPool).path("/").launch();
    assert_eq!(h.status(), 503);
    h.assert_same_origin_only("unavailable");
}

/// Full-page screenshots at `width` for human review, with the selections and texts asserted.
fn visual_capture(w: u32) {
    // Current week: today (Wednesday 2026-10-28) is shown.
    let h = Opts::new()
        .width(w)
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/")
        .launch();
    assert_eq!(h.pathname(), "/");
    assert_eq!(h.heading(), "Wednesday, October 28");
    h.capture(w, "current-week-today");

    // Archived week (clock in W49): Monday is shown. Week 49 has no weekly content.
    let h = Opts::new().width(w).sql(&crafted_week(49, &[])).launch();
    assert_eq!(h.pathname(), "/2026/44");
    assert_eq!(h.heading(), "Monday, October 26");
    h.capture(w, "archived-week-monday");

    h.click(".meal-toggle");
    h.wait_for("document.querySelector('.meal-details') !== null");
    h.capture(w, "daily-meal-expanded");
    h.click(".meal-toggle");
    h.wait_for("document.querySelector('.meal-details') === null");

    h.click(".shop-toggle");
    h.wait_for("document.querySelector('.shop-body') !== null");
    h.capture(w, "daily-shopping-expanded");

    h.goto("/2026-10-29");
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    h.capture(w, "daily-empty-day");

    h.goto("/2026-10-28");
    assert_eq!(h.count(".prep-body"), 0);
    h.capture(w, "prep-day");
    h.click(".prep-toggle");
    h.wait_for("document.querySelector('.prep-body') !== null");
    h.capture(w, "prep-expanded");

    // A week without weekly content.
    h.goto("/2026/49");
    assert_eq!(h.count(".prep"), 0);
    h.capture(w, "no-prep-day");

    // A year page with both adjacent-year links.
    h.exec(&bare_weeks(&[(2025, 52), (2027, 1)]));
    h.goto_page("/2026");
    assert_eq!(h.text(".title"), "Meal Plan: 2026");
    assert_eq!(h.texts("a.all-weeks"), ["← 2025", "This week", "2027 →"]);
    h.capture(w, "year");
    h.goto_page("/2026/50");
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    h.capture(w, "missing-week");
    h.goto_page("/2025/53");
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["Invalid week."]);
    h.capture(w, "invalid-week");
    if w < 900 {
        h.click(".calendar-toggle");
        h.wait_for("document.querySelector('#calendar').offsetParent !== null");
        h.capture(w, "invalid-week-calendar-open");
    }

    // The calendar beside the plan page: closed and open at phone width, always shown at desktop width.
    let sql = bare_weeks(&[(2026, 43), (2026, 46)]);
    let h = Opts::new().width(w).sql(&sql).launch();
    assert_eq!(h.cal_heading(), "October 2026");
    if w < 900 {
        h.capture(w, "calendar-closed");
        h.click(".calendar-toggle");
        h.wait_for("document.querySelector('#calendar').offsetParent !== null");
        h.capture(w, "calendar-open");
    } else {
        h.capture(w, "calendar");
    }
    h.goto_page("/2026/45");
    assert_eq!(h.status(), 404);
    if w < 900 {
        h.click(".calendar-toggle");
        h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    }
    h.capture(w, "missing-week-calendar");

    let h = Opts::new().width(w).db(Db::ClosedPool).path("/").launch();
    assert_eq!(h.status(), 503);
    assert_eq!(
        h.texts(".empty"),
        ["Meal plans are temporarily unavailable."]
    );
    h.capture(w, "unavailable");

    let h = Opts::new().width(w).db(Db::Empty).path("/2026").launch();
    assert_eq!(h.texts(".empty"), ["No meal plans available yet."]);
    h.capture(w, "empty-year");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_at_375px() {
    visual_capture(375);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_at_1280px() {
    visual_capture(1280);
}

/// Save a week through the real MCP endpoint of the in-process server.
fn mcp_save_week(h: &Harness, args: serde_json::Value) {
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let serde_json::Value::Object(arguments) = args else {
        panic!("arguments must be an object")
    };
    let url = format!("{}/mcp", h.base);
    let result = h.rt.block_on(async move {
        let client = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("sous-browser-test", "0.1.0"),
        )
        .serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url),
        ))
        .await
        .expect("connect");
        let result = client
            .call_tool(CallToolRequestParams::new("save_week").with_arguments(arguments))
            .await
            .expect("save_week");
        client.cancel().await.ok();
        result
    });
    assert_ne!(result.is_error, Some(true), "{result:?}");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn week_saved_through_mcp_renders_in_the_browser() {
    use serde_json::json;
    let u = |n: u32| format!("e6000000-0000-4000-8000-{n:012x}");
    let meal = |n: u32, kind: &str, name: &str, steps: [&str; 2]| {
        json!({
            "id": u(n), "type": kind, "name": name, "description": "Per person",
            "kcal": 400, "proteinG": 30, "fatG": 10, "carbsG": 40, "fiberG": 8, "forTwo": true,
            "assemblyTime": "10 min", "cookTime": null, "tip": "Taste before serving.",
            "steps": [
                {"id": u(n + 1), "text": steps[0]},
                {"id": u(n + 2), "text": steps[1]},
            ],
        })
    };
    let document = json!({
        "week": {"id": u(1), "subtitle": "Saved by an assistant", "prepIntro": "Prep on Sunday."},
        "days": [{
            "id": u(2), "weekday": 1,
            "meals": [
                meal(10, "Breakfast", "Overnight Oats", ["Stir oats and milk.", "Chill overnight."]),
                meal(20, "Dinner", "Red Lentil Soup", ["Sweat the onion.", "Simmer the lentils."]),
            ],
            "shoppingListIds": [u(30)],
            "reminder": {"title": "Soak the oats", "body": null, "icon": "🌙"},
        }],
        "shoppingLists": [{
            "id": u(30), "label": "Monday Shop", "note": null,
            "categories": [{"id": u(40), "name": "Produce", "items": [
                {"id": u(41), "text": "2 onions"}, {"id": u(42), "text": "1 lemon"},
            ]}],
        }],
        "prepSessions": [{
            "id": u(50), "title": "Sunday prep", "covers": null, "weekday": 1,
            "tasks": [{"id": u(51), "name": "Soak oats", "time": "5 min", "instructions": "Cover with milk."}],
        }],
    });

    let h = Opts::new().db(Db::Empty).mcp().path("/").launch();
    mcp_save_week(
        &h,
        json!({"isoYear": 2026, "isoWeek": 45, "expectedRevision": null, "document": document}),
    );

    h.goto("/2026/45");
    assert_eq!(h.heading(), "Monday, November 2");
    assert_eq!(h.texts(".prep-title"), ["🔪 Sunday prep"]);
    assert_eq!(h.texts(".meal-name"), ["Overnight Oats", "Red Lentil Soup"]);
    assert_eq!(h.texts(".subtitle"), ["Saved by an assistant"]);
    assert_eq!(h.texts(".reminder-title"), ["Soak the oats"]);
    assert_eq!(h.texts(".shop-label"), ["Monday Shop"]);

    h.js("(() => { document.querySelectorAll('.meal-toggle').forEach(b => b.click()); return true; })()");
    h.wait_for("document.querySelectorAll('.steps').length === 2");
    assert_eq!(
        h.texts(".steps li"),
        [
            "Stir oats and milk.",
            "Chill overnight.",
            "Sweat the onion.",
            "Simmer the lentils."
        ]
    );

    h.click(".shop-toggle");
    h.wait_for("document.querySelector('.shop-items') !== null");
    assert_eq!(h.texts(".shop-cat-name"), ["Produce"]);
    assert_eq!(h.texts(".shop-items li"), ["2 onions", "1 lemon"]);
}

// ---- Feature 13: day view ------------------------------------------------------------------

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn root_and_week_pages_show_one_day_and_date_pages_show_that_day() {
    // Clock inside W44: `/` and `/2026/44` show today; the date page shows its own day.
    let h = Opts::new()
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/")
        .launch();
    assert_eq!(h.heading(), "Wednesday, October 28");
    assert_eq!(h.count(".day-heading"), 1);
    h.goto("/2026/44");
    assert_eq!(h.heading(), "Wednesday, October 28");
    h.goto("/2026-10-27");
    assert_eq!(h.heading(), "Tuesday, October 27");
    assert_eq!(h.pathname(), "/2026-10-27");

    // Clock outside W44: `/2026/44` shows its Monday, the date page still shows its own day.
    let h = Opts::new().launch();
    assert_eq!(h.heading(), "Monday, October 26");
    h.goto("/2026-10-28");
    assert_eq!(h.heading(), "Wednesday, October 28");
    assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn day_links_cross_week_month_and_year_boundaries() {
    let sql = bare_weeks(&[(2025, 52), (2026, 1), (2026, 2), (2026, 53)]);
    let h = Opts::new().sql(&sql).launch();
    let href = |label: &str| {
        h.js(&format!(
            "document.querySelector(\"a[aria-label='{label}']\").getAttribute('href')"
        ))
    };

    // Week to week, and back across the year boundary.
    h.goto("/2026-01-04");
    assert_eq!(href("Next day"), "/2026-01-05");
    h.step_day("Next day", "/2026-01-05");
    assert_eq!(h.heading(), "Monday, January 5");
    h.goto("/2025-12-29");
    assert_eq!(href("Previous day"), "/2025-12-28");
    h.step_day("Previous day", "/2025-12-28");
    assert_eq!(h.heading(), "Sunday, December 28");

    // Inside the ISO week 2026-W53, across the new year.
    h.goto("/2026-12-31");
    assert_eq!(href("Previous day"), "/2026-12-30");
    h.step_day("Next day", "/2027-01-01");
    assert_eq!(h.heading(), "Friday, January 1");
    assert_eq!(h.status(), 200);

    // Into a week without a stored plan: the missing-plan page.
    h.goto("/2026-11-01");
    h.step_day("Next day", "/2026-11-02");
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    assert_eq!(h.count(".day-row"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn day_step_is_disabled_at_the_ends_of_the_calendar() {
    let sql = bare_weeks(&[(1, 1), (9999, 51)]);
    let h = Opts::new().sql(&sql).path("/0001-01-01").launch();
    assert_eq!(h.status(), 200);
    assert_eq!(h.heading(), "Monday, January 1");
    let prev = "document.querySelector(\"a[aria-label='Previous day']\")";
    assert_eq!(h.js(&format!("{prev}.tagName")), "A");
    assert_eq!(h.js(&format!("{prev}.hasAttribute('href')")), false);
    assert_eq!(
        h.js(&format!("{prev}.getAttribute('aria-disabled')")),
        "true"
    );
    assert_eq!(
        h.js("document.querySelector(\"a[aria-label='Next day']\").getAttribute('href')"),
        "/0001-01-02"
    );
    // It looks like the calendar's disabled month button.
    let look = |sel: &str| {
        h.js(&format!(
            "(() => {{ const s = getComputedStyle({sel}); return [s.color, s.cursor]; }})()"
        ))
    };
    assert_eq!(
        look(prev),
        look("document.querySelector(\"#calendar button[aria-label='Previous month']\")")
    );
    // Activating it does nothing.
    h.js(&format!("{prev}.click()"));
    h.js(&format!("{prev}.focus()"));
    h.press("Enter");
    assert_eq!(h.pathname(), "/0001-01-01");

    h.goto("/9999-12-26");
    assert_eq!(h.heading(), "Sunday, December 26");
    let next = "document.querySelector(\"a[aria-label='Next day']\")";
    assert_eq!(h.js(&format!("{next}.tagName")), "A");
    assert_eq!(h.js(&format!("{next}.hasAttribute('href')")), false);
    assert_eq!(
        h.js(&format!("{next}.getAttribute('aria-disabled')")),
        "true"
    );
    assert_eq!(
        h.js("document.querySelector(\"a[aria-label='Previous day']\").getAttribute('href')"),
        "/9999-12-25"
    );
    assert_eq!(
        look(next),
        look("document.querySelector(\"#calendar button[aria-label='Next month']\")")
    );
    h.js(&format!("{next}.click()"));
    assert_eq!(h.pathname(), "/9999-12-26");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_marks_have_names_shapes_and_colors() {
    // The clock (Wednesday Oct 28) is today; the displayed day is Monday Oct 26.
    let h = Opts::new()
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/2026-10-26")
        .launch();
    assert_eq!(
        h.cal_marked(),
        [
            (
                "/2026-10-26".to_string(),
                "Monday, October 26, 2026, shopping, prep".to_string()
            ),
            (
                "/2026-10-28".to_string(),
                "Wednesday, October 28, 2026, shopping, prep".to_string()
            ),
        ]
    );
    assert_eq!(
        h.js(
            "document.querySelector(\"#calendar a[href='/2026-10-27']\").getAttribute('aria-label')"
        ),
        "Tuesday, October 27, 2026"
    );
    assert_eq!(h.cal_current(), ["page:/2026-10-26"]);
    assert_eq!(h.cal_today_cells(), ["28"]);
    assert_eq!(h.count("#calendar .cal-marks[aria-hidden=true]"), 2);

    let mark = |cell: &str, kind: &str| {
        h.js(&format!(
            "(() => {{ const m = document.querySelector(\"#calendar a[href='{cell}'] .cal-mark-{kind}\"); \
              const s = getComputedStyle(m); const r = m.getBoundingClientRect(); \
              return [s.backgroundColor, s.borderRadius, r.width, r.height]; }})()"
        ))
    };
    // Shopping is a round dot, prep a small square; on the selected cell they are lighter but visible.
    assert_eq!(
        mark("/2026-10-28", "shopping"),
        serde_json::json!(["rgb(157, 23, 77)", "50%", 5, 5])
    );
    assert_eq!(
        mark("/2026-10-28", "prep"),
        serde_json::json!(["rgb(180, 83, 9)", "1px", 5, 5])
    );
    assert_eq!(
        mark("/2026-10-26", "shopping"),
        serde_json::json!(["rgb(251, 182, 206)", "50%", 5, 5])
    );
    assert_eq!(
        mark("/2026-10-26", "prep"),
        serde_json::json!(["rgb(253, 230, 138)", "1px", 5, 5])
    );
    // The today ring is drawn on the cell, and the selected cell has the gradient.
    assert_eq!(
        h.js(
            "getComputedStyle(document.querySelector('#calendar .cal-today')).boxShadow !== 'none'"
        ),
        true
    );
    assert_eq!(h.count("#calendar .cal-selected"), 1);
    // Marks do not change the row height.
    assert_eq!(
        h.js("new Set([...document.querySelectorAll('#calendar .cal-row')].map(r => r.getBoundingClientRect().height)).size"),
        1
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_marks_follow_committed_changes() {
    let h = Opts::new().launch();
    assert_eq!(h.cal_marked().len(), 2);
    h.exec(&format!(
        "UPDATE prep_sessions SET weekday = 5 WHERE title LIKE 'Monday Prep%';\n\
         DELETE FROM prep_tasks WHERE session_id IN (SELECT id FROM prep_sessions WHERE title LIKE 'Wednesday Evening%');\n\
         INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) \
         SELECT d.id, '{L1_ID}', 0 FROM days d JOIN weeks w ON w.id = d.week_id \
         WHERE w.iso_year = 2026 AND w.iso_week = 44 AND d.weekday = 2;"
    ));
    h.goto("/2026-10-26");
    let label = |href: &str| {
        h.cal_marked()
            .into_iter()
            .find(|(h, _)| h == href)
            .map(|(_, l)| l)
    };
    assert_eq!(
        label("/2026-10-26").as_deref(),
        Some("Monday, October 26, 2026, shopping")
    );
    assert_eq!(
        label("/2026-10-27").as_deref(),
        Some("Tuesday, October 27, 2026, shopping")
    );
    assert_eq!(
        label("/2026-10-28").as_deref(),
        Some("Wednesday, October 28, 2026, shopping")
    );
    assert_eq!(
        label("/2026-10-30").as_deref(),
        Some("Friday, October 30, 2026, prep")
    );
    assert_eq!(h.cal_marked().len(), 4);
    // The prep session moved to Friday, and Monday no longer shows it.
    assert_eq!(h.count(".prep"), 0);
    h.goto("/2026-10-30");
    assert_eq!(
        h.texts(".prep-title"),
        ["🔪 Monday Prep Session (~1.5 hours)"]
    );
}
