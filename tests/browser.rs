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

    // Following an entry opens that week; a stored future week selects Monday and the URL stays put.
    h.exec(&bare_weeks(&[(2027, 2)]));
    h.goto_page("/2027");
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
fn calendar_is_on_week_pages_only_and_all_weeks_is_gone() {
    let h = Harness::start("", 1280);
    let present = |h: &Harness| h.js("document.querySelectorAll('#calendar').length");
    let no_all_weeks = |h: &Harness, what: &str| {
        assert_eq!(
            h.js("document.body.textContent.includes('All weeks')"),
            false,
            "{what}"
        );
    };
    for (path, status) in [("/2026/44", 200), ("/2026/45", 404), ("/2026/54", 404)] {
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

    /// Every calendar row: [href or null, aria-label or null, aria-current or null, planned, cell texts].
    fn cal_rows(&self) -> Vec<serde_json::Value> {
        let v = self.js(
            "[...document.querySelectorAll('#calendar .cal-row')].map(r => [r.getAttribute('href'), r.getAttribute('aria-label'), r.getAttribute('aria-current'), r.classList.contains('cal-planned'), [...r.children].map(c => c.textContent)])",
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

/// Expected canonical hrefs of a month's rows, from chrono.
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
        let iso = monday.iso_week();
        hrefs.push(format!("/{:04}/{}", iso.year(), iso.week()));
        monday += chrono::Duration::days(7);
    }
    hrefs
}

fn row_hrefs(rows: &[serde_json::Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r[0].as_str().unwrap().to_string())
        .collect()
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

    // The invalid page opens on today's month, by the server's clock and not the browser's:
    // 2026-11-30 23:30Z is already December 1 in Berlin.
    let h = Opts::new()
        .clock(clock_at(2026, 11, 30, 23, 30, 0))
        .path("/2026/54")
        .launch();
    h.set_browser_timezone("America/Los_Angeles");
    h.goto_page("/2026/54");
    assert_eq!(h.status(), 404);
    assert_eq!(h.cal_heading(), "December 2026");
    assert_eq!(h.cal_today_cells(), ["1"]);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_rows_link_to_canonical_week_paths() {
    let h = Opts::new().launch();
    h.goto_page("/2026/1");
    assert_eq!(h.cal_heading(), "January 2026");
    let rows = h.cal_rows();
    assert_eq!(row_hrefs(&rows), expected_month_hrefs(2026, 1));
    // The row with December 29, 2025 is ISO 2026-W1.
    assert_eq!(rows[0][0], "/2026/1");
    assert_eq!(rows[0][4][1], "29");

    h.goto_page("/2026/44");
    assert_eq!(h.cal_heading(), "October 2026");
    assert_eq!(row_hrefs(&h.cal_rows()), expected_month_hrefs(2026, 10));
    // Each row shows its week number and seven dates, in the expected layout.
    let rows = h.cal_rows();
    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows[4][4],
        serde_json::json!(["44", "26", "27", "28", "29", "30", "31", "1"])
    );

    // December 2020, then January 2021, whose first row is ISO 2020-W53.
    h.goto_page("/2020/53");
    // Its Thursday is December 31, 2020.
    assert_eq!(h.cal_heading(), "December 2020");
    assert_eq!(row_hrefs(&h.cal_rows()), expected_month_hrefs(2020, 12));
    h.cal_click("Next month");
    assert_eq!(h.cal_heading(), "January 2021");
    let rows = h.cal_rows();
    assert_eq!(row_hrefs(&rows), expected_month_hrefs(2021, 1));
    assert_eq!(rows[0][0], "/2020/53");
    assert_eq!(rows[0][4][5], "1");

    // An unplanned row opens the missing-plan page; a planned row opens its plan.
    h.goto_page("/2026/44");
    h.cal_click("Next month");
    h.click("#calendar a[href='/2026/46']");
    h.wait_page("/2026/46");
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    h.goto_page("/2026/45");
    h.cal_click("Previous month");
    h.click("#calendar a[href='/2026/44']");
    h.wait_page("/2026/44");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_marks_planned_weeks_and_the_displayed_week() {
    let sql = bare_weeks(&[(2026, 46), (2026, 1)]);
    let h = Opts::new().sql(&sql).launch();
    let marked = |h: &Harness| -> Vec<String> {
        let v = h.js("[...document.querySelectorAll('#calendar .cal-planned')].map(r => r.querySelector('.cal-wk').textContent)");
        serde_json::from_value(v).unwrap()
    };
    let current = |h: &Harness| -> Vec<String> {
        let v = h.js("[...document.querySelectorAll('#calendar [aria-current]')].map(r => r.getAttribute('aria-current') + ':' + r.querySelector('.cal-wk').textContent)");
        serde_json::from_value(v).unwrap()
    };
    assert_eq!(marked(&h), ["44"]);
    assert_eq!(current(&h), ["page:44"]);
    // The mark is visible, also on the highlighted row.
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('.cal-planned .cal-dot')).backgroundColor !== 'rgba(0, 0, 0, 0)'"),
        true
    );
    h.cal_click("Next month");
    assert_eq!(h.cal_heading(), "November 2026");
    assert_eq!(marked(&h), ["44", "46"]);
    assert_eq!(current(&h), ["page:44"]);
    let labels: Vec<String> = h
        .cal_rows()
        .iter()
        .take(3)
        .map(|r| r[1].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        labels,
        [
            "Week 44, October 26–November 1, 2026, planned",
            "Week 45, November 2–8, 2026",
            "Week 46, November 9–15, 2026, planned"
        ]
    );
    // The highlighted row uses the selected-button gradient.
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('#calendar [aria-current]')).backgroundImage.includes('linear-gradient')"),
        true
    );

    h.goto_page("/2026/1");
    assert_eq!(
        h.cal_rows()[0][1],
        "Week 1, December 29, 2025–January 4, 2026, planned"
    );
    assert_eq!(marked(&h), ["1"]);

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
    // A non-Monday day and the Prep tab, selected before navigating.
    h.select_day(3);
    h.select_tab(1);
    let urls = h.requested_urls();
    let href = h.js("location.href");
    let state = || {
        h.js("[...document.querySelectorAll('.tab-button')].map(b => b.getAttribute('aria-pressed'))")
    };
    let tabs = state();
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
        "/2026/44"
    );

    assert_eq!(h.requested_urls(), urls);
    assert_eq!(h.js("location.href"), href);
    assert_eq!(state(), tabs);
    h.select_tab(0);
    assert_eq!(
        h.pressed(),
        vec![false, false, false, true, false, false, false]
    );
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
    let rows = h.cal_rows();
    assert_eq!(rows[0][0], "/0001/1");
    assert_eq!(rows[0][1], "Week 1, January 1–7, 0001");
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
    assert!(last[0].is_null() && last[1].is_null());
    assert_eq!(
        last[4],
        serde_json::json!(["52", "27", "28", "29", "30", "31", "1", "2"])
    );
    assert_eq!(
        h.js("document.querySelector('#calendar div.cal-row').tagName"),
        "DIV"
    );
    for r in others {
        assert!(r[0].is_string(), "{r}");
    }
    assert_eq!(others.last().unwrap()[0], "/9999/51");
    h.js("document.querySelector(\"#calendar button[aria-label='Next month']\").click()");
    assert_eq!(h.cal_heading(), "December 9999");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_reflects_committed_weeks_on_reload() {
    let h = Opts::new().launch();
    assert_eq!(
        h.js("[...document.querySelectorAll('#calendar .cal-planned')].map(r => r.getAttribute('href'))"),
        serde_json::json!(["/2026/44"])
    );
    h.exec(&bare_weeks(&[(2026, 43)]));
    h.goto_page("/2026/44");
    assert_eq!(
        h.js("[...document.querySelectorAll('#calendar .cal-planned')].map(r => r.getAttribute('href'))"),
        serde_json::json!(["/2026/43", "/2026/44"])
    );
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

    // Opens between the header and the tabs; closes again.
    h.click(".calendar-toggle");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    assert_eq!(h.expanded(), "true");
    assert_eq!(
        h.js("document.querySelector('#calendar').getBoundingClientRect().top >= document.querySelector('.header').getBoundingClientRect().bottom"),
        true
    );
    assert_eq!(
        h.js("document.querySelector('#calendar').getBoundingClientRect().bottom <= document.querySelector('.tabs').getBoundingClientRect().top"),
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
fn database_failure_shows_unavailable_not_missing() {
    for mode in [Db::NoTables, Db::ClosedPool] {
        let h = Opts::new().db(mode).path("/").launch();
        for path in ["/", "/2026/44", "/2026"] {
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
    for path in ["/2026", "/2026/52", "/2025/53", "/"] {
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
    /// Select a day (0 = Monday) by clicking its button.
    fn select_day(&self, index: usize) {
        self.click(&format!(".day-button:nth-child({})", index + 1));
        self.wait_for(&format!(
            "{}.getAttribute('aria-pressed') === 'true'",
            Harness::day_button(index)
        ));
    }

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

    h.select_day(2);
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
    for day in 0..7 {
        h.select_day(day);
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
    h.select_day(1);
    h.select_day(0);
    assert_eq!(h.count(".shop-body"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn shopping_keyboard_toggle() {
    let h = Harness::start("", 1280);
    // Tab from the last day button reaches the first shopping toggle.
    h.js(&format!("{}.focus()", Harness::day_button(6)));
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
            "INSERT INTO prep_sessions (id, week_id, position, title, covers) VALUES ('{}', '{}', 0, {}, {});\n\
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
    /// Activate a tab (0 = Daily Plan, 1 = Prep Sessions) by clicking its button.
    fn select_tab(&self, index: usize) {
        self.click(&format!(".tab-button:nth-child({})", index + 1));
        self.wait_for(&format!(
            "document.querySelectorAll('.tab-button')[{index}].getAttribute('aria-pressed') === 'true'"
        ));
    }

    fn pressed_tabs(&self) -> Vec<bool> {
        let v = self.js(
            "[...document.querySelectorAll('.tab-button')].map(b => b.getAttribute('aria-pressed') === 'true')",
        );
        serde_json::from_value(v).unwrap()
    }

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
        "Saturday",
        "Oily fish",
    ] {
        assert!(!src.contains(sample), "app.jsx hardcodes {sample:?}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn tabs_switch_and_keep_selected_day() {
    let h = Harness::start("", 1280);
    assert_eq!(h.texts(".tab-button"), ["Daily Plan", "Prep Sessions"]);
    assert_eq!(h.pressed_tabs(), [true, false]);
    let props = [
        "backgroundImage",
        "backgroundColor",
        "color",
        "fontSize",
        "fontWeight",
        "padding",
        "borderRadius",
    ];
    let on = h.js(&format!(
        "(() => {{ const s = getComputedStyle(document.querySelectorAll('.tab-button')[0]); return {props:?}.map(p => s[p]); }})()"
    ));
    assert_eq!(
        on[0],
        "linear-gradient(135deg, rgb(26, 26, 46), rgb(22, 33, 62))"
    );
    assert_eq!(on[2], "rgb(248, 250, 252)");
    assert_eq!(on[3], "12px");
    assert_eq!(on[4], "600");
    assert_eq!(on[5], "8px 16px");
    assert_eq!(on[6], "8px");
    let off = h.js(&format!(
        "(() => {{ const s = getComputedStyle(document.querySelectorAll('.tab-button')[1]); return {props:?}.map(p => s[p]); }})()"
    ));
    assert_eq!(off[0], "none");
    assert_eq!(off[1], "rgb(255, 255, 255)");
    assert_eq!(off[2], "rgb(100, 116, 139)");
    assert_eq!(off[3], "12px");
    assert_eq!(off[4], "600");
    assert_eq!(off[5], "8px 16px");

    h.select_day(2);
    h.select_tab(1);
    assert_eq!(h.pressed_tabs(), [false, true]);
    assert_eq!(h.count(".days"), 0);
    assert_eq!(h.count(".day-button"), 0);
    assert_eq!(h.count(".prep"), 2);
    assert_eq!(
        h.text(".prep-intro"),
        "Monday covers Mon–Wed. Wednesday is a smaller session for Thu–Sun."
    );

    h.select_tab(0);
    assert_eq!(h.pressed_tabs(), [true, false]);
    assert_eq!(h.count(".prep"), 0);
    assert_eq!(
        h.pressed(),
        vec![false, false, true, false, false, false, false]
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn prep_sessions_render_and_toggle() {
    let h = Harness::start("", 1280);
    h.select_tab(1);
    assert_eq!(
        h.texts(".prep-title"),
        [
            "🔪 Monday Prep Session (~1.5 hours)",
            "🔪 Wednesday Evening Prep (~45 min)"
        ]
    );
    // Covers: present for Monday, no element at all for Wednesday.
    assert_eq!(h.texts(".prep-covers"), ["Covers Monday through Wednesday"]);
    assert_eq!(
        h.js("document.querySelectorAll('.prep')[1].querySelector('.prep-covers')"),
        serde_json::Value::Null
    );
    assert_eq!(h.count(".prep-body"), 0);

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
    h.select_tab(1);
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
    for index in 0..2 {
        h.select_tab(index);
        check(&h, &format!("tab {index}"));
    }
    assert_eq!(h.count(".tab-button"), 2);
    let page = "JSON.parse(document.getElementById('sous-page').textContent).document";
    assert_eq!(h.js(&format!("'strategyCards' in {page}")), false);
    assert_eq!(h.js(&format!("{page}.schemaVersion")), 2);
    let selectors = h.js(
        "[...document.styleSheets].flatMap(s => [...s.cssRules]).map(r => r.selectorText || '')\
         .filter(s => s.toLowerCase().includes('strategy'))",
    );
    assert_eq!(selectors, serde_json::json!([]));
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_012() {
    for w in [375, 1280] {
        let h = Harness::start("", w);
        h.capture_file(&format!("012-plan-{w}.png"));
        h.select_tab(1);
        h.capture_file(&format!("012-prep-{w}.png"));
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn reminders_show_only_on_days_with_a_title() {
    let h = Harness::start("", 1280);
    // Monday has a body but no title: no reminder.
    assert_eq!(h.count(".reminder"), 0);

    // Wednesday: no meals, so the empty message comes first, then the reminder.
    h.select_day(2);
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
    h.select_day(6);
    assert_eq!(h.text(".reminder-title"), "Plan next week");
    assert_eq!(h.count(".reminder-icon"), 0);
    assert_eq!(h.count(".reminder-body"), 0);

    // Tuesday (no reminder columns), Saturday (no day row).
    for day in [1, 5] {
        h.select_day(day);
        assert_eq!(h.count(".reminder"), 0, "day {day}");
    }
    // Tuesday has meals: nothing reminder-like after them either.
    h.select_day(0);
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
    h.select_tab(1);
    assert_eq!(h.count(".prep-intro"), 0);
    assert_eq!(h.count(".prep-covers"), 0);
    assert_eq!(h.text(".prep-title"), "🔪 Bare session");
}

/// SQL adding prep sessions (title, has_task) at positions 0.. to the week made by `crafted_week(week, ..)`.
fn prep_sessions_sql(week: u32, sessions: &[(&str, bool)]) -> String {
    let base = 1000 * week;
    let mut sql = String::new();
    for (i, (title, has_task)) in sessions.iter().enumerate() {
        let sid = id(0xe1, base + i as u32);
        sql += &format!(
            "INSERT INTO prep_sessions (id, week_id, position, title) VALUES ('{sid}', '{}', {i}, '{title}');\n",
            id(0xa1, base)
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
                ("Session A", true),
                ("Session B", false),
                ("Session C", true),
            ],
        );
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/52");
    h.select_tab(1);
    assert_eq!(h.texts(".prep-title"), ["🔪 Session A", "🔪 Session C"]);
    assert_eq!(
        h.js("document.getElementById('root').textContent.includes('Session B')"),
        false
    );

    let sql = crafted_week(53, &[])
        + &prep_sessions_sql(53, &[("Taskless one", false), ("Taskless two", false)]);
    let h = Harness::start(&sql, 1280);
    h.goto("/2026/53");
    h.select_tab(1);
    assert_eq!(h.text(".empty"), "No prep sessions planned.");
    assert_eq!(h.count(".prep"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn empty_prep_shows_message() {
    let h = Harness::start(&crafted_week(49, &[]), 1280);
    h.goto("/2026/49");
    assert_eq!(h.texts(".tab-button").len(), 2);
    h.select_tab(1);
    assert_eq!(h.text(".empty"), "No prep sessions planned.");
    assert_eq!(h.count(".prep-intro"), 0);
    assert_eq!(h.count(".prep"), 0);
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
    h.select_tab(1);
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
    h.goto("/2026/44");
    h.select_day(2);
    assert_eq!(h.text(".reminder-title"), "Changed reminder");
    h.select_tab(1);
    assert_eq!(h.text(".prep-intro"), "Changed intro");
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
    h.select_tab(1);
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
    h.select_tab(0);
    assert_eq!(h.count(".app b"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn tabs_work_by_keyboard() {
    let h = Harness::start("", 1280);
    h.js("document.querySelector('#calendar a:last-of-type').focus()");
    h.press("Tab");
    assert_eq!(
        h.js("document.activeElement === document.querySelectorAll('.tab-button')[0]"),
        true
    );
    h.press("Tab");
    h.press("Enter");
    h.wait_for(
        "document.querySelectorAll('.tab-button')[1].getAttribute('aria-pressed') === 'true'",
    );
    assert_eq!(h.count(".prep"), 2);
    assert_eq!(h.count(".tab-button"), 2);
}

fn assert_weekly_layout(width: u32) {
    let h = Harness::start("", width);
    let long = "x".repeat(120);
    h.exec(&format!(
        "UPDATE prep_tasks SET time = '{long}', name = '{long}', instructions = '{long}';"
    ));
    h.goto("/2026/44");
    let fits = "document.documentElement.scrollWidth <= window.innerWidth";
    h.select_day(2);
    assert_eq!(h.js(fits), true, "plan tab");
    let one_row = h.js(
        "(() => { const b = [...document.querySelectorAll('.tab-button')]; \
          const app = document.querySelector('.app').getBoundingClientRect(); \
          return b.every(x => x.offsetTop === b[0].offsetTop) && b[1].getBoundingClientRect().right <= app.right; })()",
    );
    assert_eq!(one_row, true, "tab bar on one row");
    h.select_tab(1);
    h.js("document.querySelectorAll('.prep-toggle').forEach(b => b.click())");
    h.wait_for("document.querySelectorAll('.prep-body').length === 2");
    assert_eq!(h.js(fits), true, "prep tab");
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
    // Plan page: every tab, every card expanded.
    let h = Opts::new().launch();
    for day in 0..7 {
        h.select_day(day);
        if h.count(".meal-toggle") > 0 {
            h.expand_all(".meal-toggle");
        }
        if h.count(".shop-toggle") > 0 {
            h.expand_all(".shop-toggle");
        }
    }
    h.select_tab(1);
    h.expand_all(".prep-toggle");
    h.select_tab(0);
    h.wait_for("[...document.fonts].some(f => f.status === 'loaded')");
    let urls = h.requested_urls();
    assert!(
        urls.iter()
            .any(|u| u.starts_with(&format!("{}/assets/", h.base)) && u.ends_with(".woff2")),
        "no woff2 from /assets/: {urls:?}"
    );
    h.assert_same_origin_only("/2026/44");

    // Other pages of the same server.
    for path in ["/2026", "/2026/45", "/2025/53"] {
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
    // Current week: today (Wednesday 2026-10-28) is selected.
    let h = Opts::new()
        .width(w)
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/")
        .launch();
    assert_eq!(h.pathname(), "/");
    assert_eq!(
        h.pressed(),
        [false, false, true, false, false, false, false]
    );
    h.capture(w, "current-week-today");

    // Archived week (clock in W49): Monday is selected. Week 49 has no weekly content.
    let h = Opts::new().width(w).sql(&crafted_week(49, &[])).launch();
    assert_eq!(h.pathname(), "/2026/44");
    assert_eq!(
        h.pressed(),
        [true, false, false, false, false, false, false]
    );
    h.capture(w, "archived-week-monday");

    h.click(".meal-toggle");
    h.wait_for("document.querySelector('.meal-details') !== null");
    h.capture(w, "daily-meal-expanded");
    h.click(".meal-toggle");
    h.wait_for("document.querySelector('.meal-details') === null");

    h.click(".shop-toggle");
    h.wait_for("document.querySelector('.shop-body') !== null");
    h.capture(w, "daily-shopping-expanded");

    h.select_day(3);
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    h.capture(w, "daily-empty-day");

    h.select_tab(1);
    assert_eq!(h.count(".prep-body"), 0);
    h.capture(w, "prep-tab");
    h.click(".prep-toggle");
    h.wait_for("document.querySelector('.prep-body') !== null");
    h.capture(w, "prep-expanded");

    // A week without weekly content.
    h.goto("/2026/49");
    h.select_tab(1);
    assert_eq!(h.text(".empty"), "No prep sessions planned.");
    h.capture(w, "prep-empty");

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
        "prepSessions": [],
    });

    let h = Opts::new().db(Db::Empty).mcp().path("/").launch();
    mcp_save_week(
        &h,
        json!({"isoYear": 2026, "isoWeek": 45, "expectedRevision": null, "document": document}),
    );

    h.goto("/2026/45");
    h.js("(() => { document.querySelectorAll('.day-button')[0].click(); return true; })()");
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
