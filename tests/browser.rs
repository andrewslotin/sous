//! Browser tests against the compiled, embedded web assets (headless Google Chrome).
//!
//! Run with: `cargo test --test browser -- --ignored`

use chrono::TimeZone;
use headless_chrome::protocol::cdp::Emulation::{
    MediaFeature, SetDeviceMetricsOverride, SetEmulatedMedia, SetTimezoneOverride,
};
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
        // The page follows the device's colour scheme; start from a known one.
        emulate_color_scheme(&tab, "light");
        let h = Harness {
            rt,
            _dir: dir,
            db_path,
            _browser: browser,
            tab,
            base,
        };
        h.goto_page(path);
        // Every launch is a fresh profile on its own origin; clear the theme key anyway so none can leak.
        h.js("localStorage.removeItem('sous-theme')");
        h
    }
}

/// Emulate `prefers-color-scheme` ("light" or "dark") in the tab (CDP `Emulation.setEmulatedMedia`).
fn emulate_color_scheme(tab: &Tab, scheme: &str) {
    tab.call_method(SetEmulatedMedia {
        media: None,
        features: Some(vec![MediaFeature {
            name: "prefers-color-scheme".to_string(),
            value: scheme.to_string(),
        }]),
    })
    .unwrap();
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

    /// Switch the emulated colour scheme to "light" or "dark".
    fn set_color_scheme(&self, scheme: &str) {
        emulate_color_scheme(&self.tab, scheme);
        self.wait_for(&format!(
            "matchMedia('(prefers-color-scheme: {scheme})').matches"
        ));
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
    // The only header link is the Today link (the harness's today is another week).
    assert_eq!(h.texts("a.all-weeks"), ["Today"]);
    assert_eq!(h.count("a.all-weeks.today-link"), 1);
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

    /// Click one of the calendar's month buttons, opening the calendar first when it is closed.
    fn cal_click(&self, label: &str) {
        self.open_calendar();
        self.click(&format!("#calendar button[aria-label='{label}']"));
    }

    /// Open the calendar with the day heading's button when the page has one and the calendar is closed.
    fn open_calendar(&self) {
        self.js("(() => { const t = document.querySelector('.day-toggle'); \
                 if (t && t.getAttribute('aria-expanded') === 'false') t.click(); })()");
        self.wait_for("document.querySelector('#calendar').offsetParent !== null");
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
    // The missing-plan page has the day row of its date, with the calendar closed.
    assert_eq!(h.count(".day-row"), 1);
    assert_eq!(h.heading(), "Wednesday, November 11");
    assert!(!h.calendar_shown());
    h.goto_page("/2026/45");
    h.cal_click("Previous month");
    h.click("#calendar a[href='/2026-10-28']");
    h.wait_page("/2026-10-28");
    assert_eq!(h.status(), 200);
    assert_eq!(h.text(".title"), "Meal Plan: Oct 26 – Nov 1");
    assert_eq!(h.heading(), "Wednesday, October 28");
    // Sunday, November 1 is a dimmed cell of the November grid but a date of the displayed week.
    h.goto_page("/2026-10-28");
    h.open_calendar();
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
    // The displayed date is the blue fill.
    assert_eq!(
        h.js(
            "getComputedStyle(document.querySelector('#calendar [aria-current]')).backgroundColor"
        ),
        LIGHT.rgb("blue")
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

impl Harness {
    fn calendar_shown(&self) -> bool {
        self.js("getComputedStyle(document.querySelector('#calendar')).display !== 'none'")
            .as_bool()
            .unwrap()
    }

    /// The day heading button's `aria-expanded`.
    fn expanded(&self) -> String {
        self.js("document.querySelector('.day-toggle').getAttribute('aria-expanded')")
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Open and close once with the pointer, asserting `aria-expanded` and that the page still fits.
    fn toggle_once(&self) {
        assert!(!self.calendar_shown());
        assert_eq!(self.expanded(), "false");
        self.click(".day-toggle");
        self.wait_for("document.querySelector('#calendar').offsetParent !== null");
        assert_eq!(self.expanded(), "true");
        assert_eq!(
            self.js("document.documentElement.scrollWidth <= window.innerWidth"),
            true
        );
        self.click(".day-toggle");
        self.wait_for("document.querySelector('#calendar').offsetParent === null");
        assert_eq!(self.expanded(), "false");
    }

    /// `[left, right, top, bottom]` of the first element matching `selector`.
    fn rect(&self, selector: &str) -> [f64; 4] {
        let v = self.js(&format!(
            "(() => {{ const r = document.querySelector('{selector}').getBoundingClientRect(); \
             return [r.left, r.right, r.top, r.bottom]; }})()"
        ));
        serde_json::from_value(v).unwrap()
    }
}

/// One centred column of at most 560px on every page kind, with no sidebar and no header "Calendar" button.
fn assert_one_column(width: u32) {
    let h = Opts::new().width(width).launch();
    for path in [
        "/2026/44",
        "/2026-10-28",
        "/2026-11-11",
        "/2026/54",
        "/2026-02-30",
        "/2026",
    ] {
        h.goto_page(path);
        // Classic scrollbars consume layout width on Linux; overlay scrollbars do not.
        let viewport = h
            .js("document.documentElement.clientWidth")
            .as_f64()
            .unwrap();
        let column = viewport.min(560.0);
        let [left, right, _, _] = h.rect(".app");
        assert!(right - left <= 560.0, "{path}: column {}", right - left);
        assert_eq!(right - left, column, "{path}: column width");
        assert!((left - (viewport - right)).abs() <= 1.0, "{path}: centred");
        assert_eq!(h.count(".calendar-toggle"), 0, "{path}: header button");
        assert_eq!(h.count(".sidebar"), 0, "{path}: sidebar");
        assert_eq!(
            h.js("document.documentElement.scrollWidth <= window.innerWidth"),
            true,
            "{path}: overflow"
        );
        if h.count("#calendar") == 1 {
            // The calendar is inside the column, never beside it.
            h.open_calendar();
            let [cl, cr, _, _] = h.rect("#calendar");
            assert!(cl >= left - 0.5 && cr <= right + 0.5, "{path}: calendar");
            assert_eq!(
                h.js("getComputedStyle(document.querySelector('#calendar')).position"),
                "static",
                "{path}"
            );
        }
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn every_page_is_one_column_of_at_most_560px_at_1280px() {
    assert_one_column(1280);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn every_page_is_one_column_of_at_most_560px_at_900px() {
    assert_one_column(900);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn every_page_is_one_column_of_at_most_560px_at_375px() {
    assert_one_column(375);
}

/// The calendar is hidden by default on a day page; the day heading's button shows and hides it.
fn assert_calendar_toggle(width: u32) {
    let h = Opts::new().width(width).launch();
    assert!(!h.calendar_shown());
    assert_eq!(h.count("#calendar:not(.is-open)"), 1);
    assert_eq!(h.expanded(), "false");
    assert_eq!(h.count(".calendar-toggle"), 0);
    // The heading is a button with the heading text, controlling the calendar.
    assert_eq!(
        h.count("h2.day-heading > button.day-toggle[type=button]"),
        1
    );
    assert_eq!(
        h.js("document.querySelector('.day-toggle').getAttribute('aria-controls')"),
        "calendar"
    );
    assert_eq!(
        h.js("document.querySelector('.day-toggle').getAttribute('aria-label')"),
        format!("{}, calendar", h.heading())
    );
    // A dashed underline marks it as a link.
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('.day-toggle')).textDecorationStyle"),
        "dashed"
    );

    // Pointer: opens below the day row and above the content; closes again.
    h.click(".day-toggle");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    assert_eq!(h.expanded(), "true");
    assert_eq!(h.count("#calendar.is-open"), 1);
    assert!(h.rect("#calendar")[2] >= h.rect(".day-row")[3]);
    assert!(h.rect("#calendar")[3] <= h.rect(".summary")[2]);
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= window.innerWidth"),
        true
    );
    h.click(".day-toggle");
    h.wait_for("document.querySelector('#calendar').offsetParent === null");
    assert_eq!(h.expanded(), "false");

    // Keyboard: Enter and Space on the focused button toggle too.
    h.js("document.querySelector('.day-toggle').focus()");
    h.press("Enter");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    assert_eq!(h.expanded(), "true");
    h.press(" ");
    h.wait_for("document.querySelector('#calendar').offsetParent === null");
    assert_eq!(h.expanded(), "false");
    // Picking a date opens that page with the calendar closed, as does a reload.
    h.press("Enter");
    h.wait_for("document.querySelector('#calendar').offsetParent !== null");
    h.click("#calendar a[href='/2026-10-28']");
    h.wait_page("/2026-10-28");
    assert!(!h.calendar_shown());
    assert_eq!(h.expanded(), "false");
    h.goto("/2026/44");
    assert!(!h.calendar_shown());

    // The missing-plan page toggles too, and fits when open.
    h.goto_page("/2026/45");
    h.toggle_once();
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_toggles_from_the_day_heading_at_375px() {
    assert_calendar_toggle(375);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn calendar_toggles_from_the_day_heading_at_1280px() {
    assert_calendar_toggle(1280);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn invalid_pages_always_show_the_calendar_below_the_message_without_a_toggle() {
    let h = Opts::new().launch();
    for path in ["/2026/54", "/2026-02-30"] {
        h.goto_page(path);
        assert_eq!(h.status(), 404, "{path}");
        assert_eq!(h.count(".day-row"), 0, "{path}");
        assert_eq!(h.count(".day-toggle"), 0, "{path}");
        assert_eq!(h.count("#calendar.calendar.is-open"), 1, "{path}");
        assert!(h.calendar_shown(), "{path}");
        // The message and its "This week" link come first, then the calendar.
        assert_eq!(
            h.js("[...document.querySelector('.app').children].map(e => e.className)"),
            serde_json::json!(["header", "empty", "nav", "calendar is-open", "footer"]),
            "{path}"
        );
        assert!(h.rect(".nav")[3] <= h.rect("#calendar")[2], "{path}");
        assert_eq!(h.texts("a.nav-link"), ["This week"], "{path}");
    }
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
    // The only header link is the Today link.
    assert_eq!(h.texts("a.all-weeks"), ["Today"]);
    assert_eq!(h.count("a.all-weeks.today-link"), 1);
    assert_eq!(h.cal_heading(), "December 2026");
    // The day row of the displayed date (the week's Monday) with working steps and the calendar closed.
    assert_eq!(h.count(".day-row"), 1);
    assert_eq!(h.heading(), "Monday, December 28");
    assert_eq!(
        h.js(
            "[...document.querySelectorAll('.day-row a.day-step')].map(a => a.getAttribute('href'))"
        ),
        serde_json::json!(["/2026-12-27", "/2026-12-29"])
    );
    assert!(!h.calendar_shown());
    h.click("a.day-step[aria-label='Next day']");
    h.wait_page("/2026-12-29");
    assert_eq!(h.status(), 404);
    assert_eq!(h.heading(), "Tuesday, December 29");

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
    // Today is the displayed date.
    assert_eq!(h.heading(), "Wednesday, December 2");
    assert_eq!(h.count("a.day-step[href]"), 2);
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
        // The calendar is there, shown, on today's month (December 2, 2026).
        assert!(h.calendar_shown(), "{path}");
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
    h.click(".day-toggle");
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
fn monday_shows_two_shopping_cards_after_the_meals() {
    let h = Harness::start("", 1280);
    assert_eq!(h.texts(".shop-label"), [L2_LABEL, L1_LABEL]);
    assert_eq!(h.count(".shop"), 2);
    assert_eq!(
        h.js(
            "[...document.querySelectorAll('.shop')].every(s => \
             (s.compareDocumentPosition(document.querySelector('.meals')) & Node.DOCUMENT_POSITION_PRECEDING) !== 0)"
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
    // The card comes after the empty-day message.
    assert_eq!(h.text(".empty"), "No meals planned for this day.");
    assert_eq!(
        h.js(
            "(document.querySelector('.shop').compareDocumentPosition(document.querySelector('.empty')) \
             & Node.DOCUMENT_POSITION_PRECEDING) !== 0"
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
fn shopping_items_are_plain_rows() {
    let h = Harness::start("", 1280);
    h.expand_all_shops();
    assert_eq!(h.count("input[type=checkbox], [role=checkbox]"), 0);
    assert_eq!(
        h.js("/\\d+\\/\\d+ items/.test(document.body.textContent)"),
        false
    );
    assert_eq!(
        h.js("[...document.querySelectorAll('.shop li')].every(li => li.parentElement.tagName === 'UL' \
              && getComputedStyle(li).listStyleType === 'none')"),
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
    // Tab reaches the first shopping toggle after the meals.
    h.tab_to(".shop-toggle");
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
            return r.left >= app.left - 0.5 && r.right <= app.right + 0.5 && r.width <= 560; }), \
            appWidth: app.width, \
            noScroll: document.documentElement.scrollWidth <= window.innerWidth, \
            bg: getComputedStyle(document.querySelector('.shop')).backgroundColor, \
            open: document.querySelector('.shop').classList.contains('is-open'), \
            border: getComputedStyle(document.querySelector('.shop')).borderTopColor }; })()",
    );
    assert_eq!(report["fits"], true);
    assert!(report["appWidth"].as_f64().unwrap() <= 560.0);
    assert_eq!(report["noScroll"], true);
    assert_eq!(report["bg"], LIGHT.rgb("surface"));
    // A closed card has a line border; an open one is ringed in ink.
    let border = if report["open"] == true {
        "ink"
    } else {
        "line"
    };
    assert_eq!(report["border"], LIGHT.rgb(border));
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
fn plan_page_order_is_day_row_calendar_summary_meals_then_tasks() {
    let h = Harness::start("", 1280);
    let order = "[...document.querySelector('.app').children].map(e => e.className.split(' ')[0])";
    let tasks =
        "[...document.querySelector('.tasks').children].map(e => e.className.split(' ')[0])";
    h.goto("/2026-10-26");
    assert_eq!(
        h.js(order),
        serde_json::json!([
            "header", "day-row", "calendar", "summary", "meals", "tasks", "footer"
        ])
    );
    assert_eq!(h.count(".meals > .meal"), 4);
    assert_eq!(
        h.js(tasks),
        serde_json::json!(["shop", "shop", "prep-intro", "prep"])
    );
    // The reminder of Monday has no title and is not shown.
    h.goto("/2026-10-28");
    assert_eq!(
        h.js(order),
        serde_json::json!(["header", "day-row", "calendar", "empty", "tasks", "footer"])
    );
    assert_eq!(
        h.js(tasks),
        serde_json::json!(["shop", "prep-intro", "prep", "reminder"])
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
    assert_eq!(h.texts(".prep-title"), ["Monday Prep Session (~1.5 hours)"]);
    // Covers: present for Monday.
    assert_eq!(h.texts(".prep-covers"), ["Covers Monday through Wednesday"]);
    assert_eq!(h.count(".prep-body"), 0);
    // Wednesday shows its own session, which has no covers element at all.
    h.goto("/2026-10-28");
    assert_eq!(h.texts(".prep-title"), ["Wednesday Evening Prep (~45 min)"]);
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

    // A square surface card (ringed in ink while open), a solid yellow band and on-yellow text.
    assert_eq!(
        h.styles(
            ".prep",
            &["backgroundColor", "borderTopColor", "borderRadius"]
        ),
        [LIGHT.rgb("surface"), LIGHT.rgb("ink"), "0px".to_string()]
    );
    assert_eq!(
        h.styles(".prep-toggle", &["backgroundColor"]),
        [LIGHT.rgb("yellow")]
    );
    let title = h.styles(
        ".prep-title",
        &["fontSize", "fontWeight", "color", "fontFamily"],
    );
    assert_eq!(title[..3], ["17px", "600", &LIGHT.rgb("on-yellow")]);
    assert!(title[3].contains("Jost"));
    assert_eq!(
        h.styles(".prep-covers", &["fontSize", "color"]),
        ["14px".to_string(), LIGHT.rgb("on-yellow")]
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
            "11px".to_string(),
            "600".to_string(),
            LIGHT.rgb("ink"),
            LIGHT.rgb("yellow-soft"),
            "1px 6px".to_string(),
            "2px".to_string()
        ]
    );
    assert!(badge[6].contains("JetBrains Mono"));
    assert_eq!(
        h.styles(".prep-task", &["backgroundColor", "borderRadius"]),
        [LIGHT.rgb("surface-sunken"), "0px".to_string()]
    );
    assert_eq!(
        h.styles(".prep-task-name", &["fontSize", "fontWeight", "color"]),
        ["16px".to_string(), "600".to_string(), LIGHT.rgb("ink")]
    );
    assert_eq!(
        h.styles(
            ".prep-task-instructions",
            &["fontSize", "color", "lineHeight"]
        ),
        [
            "16px".to_string(),
            LIGHT.rgb("ink-muted"),
            "24px".to_string()
        ]
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
    h.open_calendar();
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
    // A sunken, square block with a dashed strong border, no gradient.
    let r = h.styles(
        ".reminder",
        &[
            "backgroundImage",
            "backgroundColor",
            "borderTopColor",
            "borderTopStyle",
            "borderRadius",
            "padding",
            "display",
        ],
    );
    assert_eq!(r[0], "none");
    assert_eq!(r[1], LIGHT.rgb("surface-sunken"));
    assert_eq!(r[2], LIGHT.rgb("line-strong"));
    assert_eq!(r[3], "dashed");
    assert_eq!(r[4], "0px");
    assert_eq!(r[5], "12px 16px");
    assert_eq!(r[6], "flex");
    assert_eq!(h.styles(".reminder-icon", &["fontSize"]), ["20px"]);
    assert_eq!(
        h.styles(".reminder-title", &["fontSize", "fontWeight", "color"]),
        ["16px".to_string(), "600".to_string(), LIGHT.rgb("ink")]
    );
    assert_eq!(
        h.styles(".reminder-body", &["fontSize", "color"]),
        ["14px".to_string(), LIGHT.rgb("ink-muted")]
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
    assert_eq!(h.text(".prep-title"), "Bare session");
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
    assert_eq!(h.texts(".prep-title"), ["Session A", "Session C"]);
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
            assert_eq!(h.texts(".prep-title"), ["Tuesday prep"]);
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
    assert_eq!(h.text(".prep-title"), "Crafted session");
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
    assert_eq!(h.text(".prep-title"), x);
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
    h.js("document.querySelector(\"a[aria-label='Previous day']\").focus()");
    // The calendar is closed, so the heading's button sits between the two steps.
    h.press("Tab");
    assert_eq!(
        h.js("document.activeElement === document.querySelector('.day-toggle')"),
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
    let row = h.js("(() => { const app = document.querySelector('.app').getBoundingClientRect(); \
              const row = [...document.querySelectorAll('.day-row > *')].map(e => e.getBoundingClientRect()); \
              const h = document.querySelector('.day-heading'); \
              return { inside: row.every(r => r.left >= app.left - 0.5 && r.right <= app.right + 0.5), \
                height: h.getBoundingClientRect().height, font: parseFloat(getComputedStyle(h).fontSize), \
                lines: document.querySelector('.day-toggle').getClientRects().length }; })()");
    assert_eq!(row["inside"], true, "{row}");
    assert_eq!(row["lines"], 1, "{row}");
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

    // The calendar on the plan page: closed by default, then open from the day heading.
    let sql = bare_weeks(&[(2026, 43), (2026, 46)]);
    let h = Opts::new().width(w).sql(&sql).launch();
    h.capture(w, "calendar-closed");
    h.open_calendar();
    assert_eq!(h.cal_heading(), "October 2026");
    h.capture(w, "calendar-open");
    h.goto_page("/2026/45");
    assert_eq!(h.status(), 404);
    h.open_calendar();
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
    assert_eq!(h.texts(".prep-title"), ["Sunday prep"]);
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
    // The missing-plan page keeps the day row, so the steps go on.
    assert_eq!(h.heading(), "Monday, November 2");
    h.step_day("Next day", "/2026-11-03");
    assert_eq!(h.heading(), "Tuesday, November 3");
    h.step_day("Previous day", "/2026-11-02");
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

    h.open_calendar();
    let mark = |cell: &str, kind: &str| {
        h.js(&format!(
            "(() => {{ const m = document.querySelector(\"#calendar a[href='{cell}'] .cal-mark-{kind}\"); \
              const s = getComputedStyle(m); const r = m.getBoundingClientRect(); \
              return [s.backgroundColor, s.borderRadius, s.clipPath, r.width, r.height]; }})()"
        ))
    };
    let triangle = "polygon(50% 0px, 100% 100%, 0px 100%)";
    // Shopping is a small square and prep a small triangle, both ink; on the selected date, on-blue.
    assert_eq!(
        mark("/2026-10-28", "shopping"),
        serde_json::json!([LIGHT.rgb("ink"), "0px", "none", 6, 6])
    );
    assert_eq!(
        mark("/2026-10-28", "prep"),
        serde_json::json!([LIGHT.rgb("ink"), "0px", triangle, 7, 6])
    );
    assert_eq!(
        mark("/2026-10-26", "shopping"),
        serde_json::json!([LIGHT.rgb("on-blue"), "0px", "none", 6, 6])
    );
    assert_eq!(
        mark("/2026-10-26", "prep"),
        serde_json::json!([LIGHT.rgb("on-blue"), "0px", triangle, 7, 6])
    );
    // The planned-week mark is an ink-muted circle.
    h.cal_click("Next month");
    assert_eq!(
        h.styles(
            ".cal-planned .cal-dot",
            &["backgroundColor", "borderRadius"]
        ),
        [LIGHT.rgb("ink-muted"), "50%".to_string()]
    );
    h.cal_click("Previous month");
    // The today ring is an inset ink ring on the cell, and the selected cell is the blue fill.
    let ring = h.styles("#calendar .cal-today:not(.cal-selected)", &["boxShadow"]);
    assert!(ring[0].contains(&LIGHT.rgb("ink")), "{ring:?}");
    assert!(ring[0].contains("inset"), "{ring:?}");
    assert!(ring[0].contains("1.5px"), "{ring:?}");
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
    assert_eq!(h.texts(".prep-title"), ["Monday Prep Session (~1.5 hours)"]);
}

// ---- Feature 14: redesign ---------------------------------------------------------------------

/// Every colour token of the design system: name, light hex, dark hex (as written in `web/styles.css`).
const TOKENS: [(&str, &str, &str); 20] = [
    ("paper", "#f1e9d8", "#0f1220"),
    ("surface", "#fbf7ee", "#181c2c"),
    ("surface-sunken", "#e9e0cc", "#0a0c16"),
    ("line", "#dcd1ba", "#2b3044"),
    ("line-strong", "#7a705c", "#737a96"),
    ("ink", "#17171c", "#f2ede1"),
    ("ink-muted", "#55514a", "#b0b2bf"),
    ("ink-faint", "#857c69", "#7f8499"),
    ("blue", "#1d3f9e", "#86a0ff"),
    ("blue-soft", "#dde3f4", "#1c2550"),
    ("on-blue", "#fbf7ee", "#0a0c16"),
    ("red", "#b8321b", "#ff7a5c"),
    ("on-red", "#fbf7ee", "#0a0c16"),
    ("yellow", "#f2c12e", "#f5c84a"),
    ("on-yellow", "#17171c", "#0a0c16"),
    ("yellow-soft", "#f8e8b0", "#3b310f"),
    ("macro-protein", "#a8430e", "#ff9b62"),
    ("macro-fat", "#755700", "#e6c05c"),
    ("macro-carbs", "#35652a", "#9fd27f"),
    ("macro-fiber", "#683d91", "#c8a3f2"),
];

/// A theme of the design system, by its `prefers-color-scheme` name.
struct Theme {
    scheme: &'static str,
}

const LIGHT: Theme = Theme { scheme: "light" };
const DARK: Theme = Theme { scheme: "dark" };

impl Theme {
    /// The token's hex value in this theme.
    fn hex(&self, name: &str) -> &'static str {
        let (_, light, dark) = TOKENS
            .iter()
            .find(|(n, _, _)| *n == name)
            .unwrap_or_else(|| panic!("no token --{name}"));
        if self.scheme == "dark" { dark } else { light }
    }

    /// The token as a computed `rgb()` string.
    fn rgb(&self, name: &str) -> String {
        let hex = self.hex(name).trim_start_matches('#');
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        format!("rgb({}, {}, {})", channel(0), channel(2), channel(4))
    }
}

/// A fixture day (Monday) with shopping, prep, meals with badges and a tip, and a reminder. The shopping
/// labels lose their emoji: those are household data, and this day checks the interface's own text.
fn redesign_day_sql() -> String {
    "UPDATE shopping_lists SET label = trim(replace(replace(label, '🛒', ''), '🧴', ''));\n\
     UPDATE days SET reminder_title = 'Shop day', reminder_body = 'Pick up the top-up list.', reminder_icon = '🧾' \
     WHERE id = '00000002-0000-4000-8000-000000000002';"
        .to_string()
}

/// Launch on the redesign day (Monday, October 26; today is Wednesday the 28th).
fn redesign_day(width: u32) -> Harness {
    Opts::new()
        .sql(&redesign_day_sql())
        .width(width)
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/2026-10-26")
        .launch()
}

impl Harness {
    /// Fetch `url` (same origin) as text in the page.
    fn fetch_text(&self, url: &str) -> String {
        self.tab
            .evaluate(&format!("fetch('{url}').then(r => r.text())"), true)
            .unwrap()
            .value
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Press Tab until the focused element matches `selector` (from the top of the page).
    fn tab_to(&self, selector: &str) {
        self.js("document.activeElement && document.activeElement.blur()");
        for _ in 0..150 {
            self.press("Tab");
            if self.js(&format!("document.activeElement.matches('{selector}')")) == true {
                return;
            }
        }
        panic!("Tab never reached {selector}");
    }

    /// Open the calendar on a narrow page and expand the first meal, every shopping list and every prep card.
    fn expand_redesign_day(&self) {
        self.click(".meal-toggle");
        self.wait_for("document.querySelectorAll('.meal-details').length === 1");
        self.expand_all(".shop-toggle");
        self.expand_all(".prep-toggle");
    }
}

/// Remove the first brace-delimited block that starts at `marker` (its own braces included).
fn strip_block(css: &str, marker: &str) -> String {
    let start = css.find(marker).unwrap_or_else(|| panic!("no {marker}"));
    let open = start + css[start..].find('{').unwrap();
    let mut depth = 0;
    for (i, c) in css[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return format!("{}{}", &css[..start], &css[open + i + 1..]);
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after {marker}");
}

/// The custom properties declared in the first block that starts at `marker`.
fn declared_properties(css: &str, marker: &str) -> Vec<String> {
    let start = css.find(marker).unwrap_or_else(|| panic!("no {marker}"));
    let open = start + css[start..].find('{').unwrap();
    let close = open + css[open..].find('}').unwrap();
    css[open + 1..close]
        .split(';')
        .filter_map(|d| d.trim().strip_prefix("--"))
        .map(|d| d.split(':').next().unwrap().trim().to_string())
        .collect()
}

impl Harness {
    /// Choose a theme through the footer toggle (as a user would) and wait for it to apply.
    fn choose_theme(&self, value: &str) {
        self.js(&format!(
            "document.querySelector('input[name=theme][value={value}]').click()"
        ));
        self.wait_for(&format!(
            "document.querySelector('input[name=theme]:checked').value === '{value}'"
        ));
    }

    /// Choose Light or Dark and wait until `data-theme` follows.
    fn use_theme(&self, t: &Theme) {
        self.choose_theme(t.scheme);
        self.wait_for(&format!(
            "document.documentElement.getAttribute('data-theme') === '{}'",
            t.scheme
        ));
    }

    fn theme_attr(&self) -> serde_json::Value {
        self.js("document.documentElement.getAttribute('data-theme')")
    }

    fn stored_theme(&self) -> serde_json::Value {
        self.js("localStorage.getItem('sous-theme')")
    }

    /// A custom property of the root element, as computed.
    fn token(&self, name: &str) -> String {
        self.js(&format!(
            "getComputedStyle(document.documentElement).getPropertyValue('--{name}').trim()"
        ))
        .as_str()
        .unwrap()
        .to_string()
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn tokens_apply_in_light_and_dark() {
    let h = redesign_day(1280);
    h.click(".meal-toggle");
    h.wait_for("document.querySelectorAll('.meal-details').length === 1");
    h.open_calendar();
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        let s = t.scheme;
        assert_eq!(
            h.js("getComputedStyle(document.documentElement).colorScheme"),
            s
        );
        // Every colour token has its design-system value on the root, in this theme.
        for (name, _, _) in TOKENS {
            assert_eq!(h.token(name), t.hex(name), "{s} --{name}");
        }
        assert_eq!(h.token("focus"), t.hex("ink"), "{s} --focus");
        // The shared, non-colour tokens are identical in both themes.
        for (name, value) in [
            ("space-1", "4px"),
            ("space-2", "8px"),
            ("space-3", "12px"),
            ("space-4", "16px"),
            ("space-5", "24px"),
            ("space-6", "32px"),
            ("radius-square", "0"),
            ("radius-soft", "2px"),
            ("radius-circle", "50%"),
            ("column-width", "560px"),
        ] {
            assert_eq!(h.token(name), value, "{s} --{name}");
        }
        assert!(h.token("font-sans").starts_with("\"Jost\""), "{s}");
        assert!(
            h.token("font-mono").starts_with("\"JetBrains Mono\""),
            "{s}"
        );
        // Components read the tokens.
        assert_eq!(
            h.js("getComputedStyle(document.body).backgroundColor"),
            t.rgb("paper"),
            "{s} body"
        );
        assert_eq!(
            h.styles(".meal-name", &["color"]),
            [t.rgb("ink")],
            "{s} text"
        );
        assert_eq!(
            h.styles(".meal", &["borderTopColor", "borderTopWidth"]),
            [t.rgb("ink"), "1px".to_string()],
            "{s} open card"
        );
        assert_eq!(
            h.styles(".shop", &["backgroundColor"]),
            [t.rgb("surface")],
            "{s} card"
        );
        // The selected date is the blue disc with on-blue text; today is ringed in ink.
        assert_eq!(
            h.styles("#calendar .cal-selected", &["backgroundColor", "color"]),
            [t.rgb("blue"), t.rgb("on-blue")],
            "{s} selected date"
        );
        let ring = h.styles("#calendar .cal-today:not(.cal-selected)", &["boxShadow"]);
        assert!(ring[0].contains(&t.rgb("ink")), "{s} today ring {ring:?}");
        // No gradient on any card.
        assert_eq!(h.styles(".meal", &["backgroundImage"]), ["none"]);
    }
}

#[test]
fn stylesheet_holds_every_colour_in_the_token_blocks() {
    let css = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/styles.css"),
    )
    .unwrap();
    let light = declared_properties(&css, ":root {");
    let dark = declared_properties(&css, ":root[data-theme=\"dark\"] {");
    // Every colour token is on :root, redefined by the dark block, and agrees with the table here.
    for (name, light_hex, dark_hex) in TOKENS {
        assert!(light.iter().any(|p| p == name), "--{name} missing on :root");
        assert!(dark.iter().any(|p| p == name), "--{name} missing in dark");
        assert!(css.contains(&format!("--{name}: {light_hex};")), "{name}");
        assert!(css.contains(&format!("--{name}: {dark_hex};")), "{name}");
    }
    // The dark block holds colours only; the sizes and fonts are shared.
    for p in &dark {
        assert!(
            TOKENS.iter().any(|(n, _, _)| n == p) || p == "focus" || p == "shadow",
            "--{p} in the dark block is not a colour token"
        );
    }
    for token in [
        "space-1",
        "space-6",
        "radius-square",
        "radius-soft",
        "radius-circle",
        "column-width",
        "font-sans",
        "font-mono",
    ] {
        assert!(light.iter().any(|p| p == token), "--{token} is not a token");
    }
    // The theme follows `data-theme`, never a media query, and there is no sidebar width.
    assert!(!css.contains("prefers-color-scheme"));
    assert!(!css.contains("sidebar"));
    assert!(css.contains("color-scheme: light;") && css.contains("color-scheme: dark;"));

    // Outside the two token blocks there is no raw colour.
    let rest = strip_block(
        &strip_block(&css, ":root[data-theme=\"dark\"] {"),
        ":root {",
    );
    let bytes = rest.as_bytes();
    for (i, _) in rest.match_indices('#') {
        let run = bytes[i + 1..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        let word = &rest[i + 1..i + 1 + run];
        assert!(
            !(matches!(run, 3 | 4 | 6 | 8) && word.bytes().all(|b| b.is_ascii_hexdigit())),
            "hex colour #{word} outside the token blocks"
        );
    }
    for func in [
        "rgb(", "rgba(", "hsl(", "hsla(", "hwb(", "lab(", "lch(", "oklab(", "oklch(", "color(",
        "gradient",
    ] {
        assert!(!rest.contains(func), "{func} outside the token blocks");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn theme_toggle_defaults_to_system_and_remembers_the_choice() {
    let h = Opts::new().launch();
    // A labelled group of three radios in the footer, System checked, nothing stored.
    assert_eq!(h.count("footer.footer .theme[role=radiogroup]"), 1);
    assert_eq!(h.text(".theme-label"), "Theme");
    assert_eq!(h.texts(".theme-option"), ["Light", "Dark", "System"]);
    assert_eq!(
        h.js("[...document.querySelectorAll('input[type=radio][name=theme]')].map(i => i.value)"),
        serde_json::json!(["light", "dark", "system"])
    );
    assert_eq!(
        h.js("[...document.querySelectorAll('input[name=theme]')].filter(i => i.checked).map(i => i.value)"),
        serde_json::json!(["system"])
    );
    assert_eq!(h.stored_theme(), serde_json::Value::Null);
    // System resolves through the emulated device preference (light here).
    assert_eq!(h.theme_attr(), "light");

    // Dark sets data-theme and is stored; it survives a reload even though the device says light.
    h.choose_theme("dark");
    h.wait_for("document.documentElement.getAttribute('data-theme') === 'dark'");
    assert_eq!(h.stored_theme(), "dark");
    h.goto("/2026/44");
    assert_eq!(h.theme_attr(), "dark");
    assert_eq!(h.stored_theme(), "dark");
    assert_eq!(
        h.js("document.querySelector('input[name=theme]:checked').value"),
        "dark"
    );
    assert_eq!(
        h.js("getComputedStyle(document.body).backgroundColor"),
        DARK.rgb("paper")
    );

    // Light is stored and wins over a dark device.
    h.set_color_scheme("dark");
    h.choose_theme("light");
    h.wait_for("document.documentElement.getAttribute('data-theme') === 'light'");
    assert_eq!(h.stored_theme(), "light");
    h.goto("/2026/44");
    assert_eq!(h.theme_attr(), "light");

    // System removes the key and follows the device again (dark here).
    h.choose_theme("system");
    h.wait_for("document.documentElement.getAttribute('data-theme') === 'dark'");
    assert_eq!(h.stored_theme(), serde_json::Value::Null);
    h.goto("/2026/44");
    assert_eq!(h.theme_attr(), "dark");
    assert_eq!(
        h.js("document.querySelector('input[name=theme]:checked').value"),
        "system"
    );

    // The footer is last on pages of every kind, and carries the toggle.
    for path in ["/2026/44", "/2026/45", "/2026-02-30", "/2026"] {
        h.goto_page(path);
        assert_eq!(
            h.js("document.querySelector('.app').lastElementChild.className"),
            "footer",
            "{path}"
        );
        assert_eq!(h.count(".footer input[name=theme]"), 3, "{path}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn system_theme_follows_the_device_without_a_reload() {
    let h = Opts::new().launch();
    h.js("window.__marker = 1");
    for scheme in ["dark", "light", "dark"] {
        h.set_color_scheme(scheme);
        h.wait_for(&format!(
            "document.documentElement.getAttribute('data-theme') === '{scheme}'"
        ));
        assert_eq!(
            h.js("getComputedStyle(document.body).backgroundColor"),
            Theme { scheme }.rgb("paper")
        );
        // The same document: nothing reloaded.
        assert_eq!(h.js("window.__marker"), 1);
    }
    // A stored choice does not follow the device.
    h.choose_theme("light");
    h.set_color_scheme("dark");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(h.theme_attr(), "light");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn theme_is_set_before_the_stylesheet_and_the_app_script() {
    let h = Opts::new().launch();
    let html = h.fetch_text("/2026/44");
    let theme = html
        .find("setAttribute(\"data-theme\"")
        .expect("theme script");
    let sheet = html.find("rel=\"stylesheet\"").expect("stylesheet");
    let app = html.find("/app.").expect("app script");
    assert!(
        theme < sheet,
        "the theme script comes before the stylesheet"
    );
    assert!(sheet < app);
    assert!(
        html[..theme].rfind("<script").is_some() && !html[..theme].contains("defer"),
        "the theme script is inline and not deferred"
    );
    // Before the app runs, the stored choice is already applied: the first thing a reload shows.
    h.choose_theme("dark");
    h.navigate("/2026/44");
    assert_eq!(
        h.js("document.documentElement.getAttribute('data-theme')"),
        "dark"
    );
    // An unreadable value falls back to the device preference.
    h.js("localStorage.setItem('sous-theme', 'sepia')");
    h.goto("/2026/44");
    assert_eq!(h.theme_attr(), "light");
    assert_eq!(
        h.js("document.querySelector('input[name=theme]:checked').value"),
        "system"
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn interface_has_no_emoji_and_carries_its_labels() {
    let h = redesign_day(1280);
    h.expand_redesign_day();
    h.expand_all(".meal-toggle");
    // Only the reminder's own icon (household data) is an emoji.
    assert_eq!(h.text(".reminder-icon"), "🧾");
    assert_eq!(
        h.js("(() => { const icon = document.querySelector('.reminder-icon').innerText; \
              return /\\p{Extended_Pictographic}/u.test(document.body.innerText.split(icon).join('')); })()"),
        false
    );
    // No pictographic character in the interface's own text on the other pages either.
    for path in ["/2026", "/2026/45", "/2026-02-30"] {
        h.goto_page(path);
        assert_eq!(
            h.js("/\\p{Extended_Pictographic}/u.test(document.body.innerText)"),
            false,
            "{path}"
        );
    }
    h.goto("/2026-10-26");
    h.expand_redesign_day();
    h.expand_all(".meal-toggle");
    // Badges and labels are uppercased by CSS; their text stays in sentence case.
    let inner = |sel: &str| -> Vec<String> {
        serde_json::from_value(h.js(&format!(
            "[...document.querySelectorAll('{sel}')].map(e => e.innerText)"
        )))
        .unwrap()
    };
    assert_eq!(inner(".badge-two"), ["FOR TWO", "FOR TWO"]);
    assert_eq!(h.texts(".badge-two"), ["For two", "For two"]);
    assert_eq!(
        inner(".badge-assembly"),
        ["ASSEMBLE 2 MIN", "ASSEMBLE 10 MIN", "ASSEMBLE 1 MIN"]
    );
    assert_eq!(
        h.texts(".badge-assembly"),
        ["Assemble 2 min", "Assemble 10 min", "Assemble 1 min"]
    );
    assert_eq!(inner(".badge-cook"), ["COOK 25 MIN"]);
    assert_eq!(h.texts(".badge-cook"), ["Cook 25 min"]);
    assert_eq!(inner(".tip-label"), ["TIP", "TIP", "TIP"]);
    assert_eq!(h.texts(".tip-label"), ["Tip", "Tip", "Tip"]);
    assert_eq!(h.count(".tip-label .shape.shape-circle"), 3);
    assert_eq!(h.texts(".tip > span:last-child").len(), 3);
    assert_eq!(inner(".shop .band-label"), ["SHOPPING", "SHOPPING"]);
    assert_eq!(h.texts(".shop .band-label"), ["Shopping", "Shopping"]);
    assert_eq!(h.count(".shop .band-label .shape-square"), 2);
    assert_eq!(inner(".prep .band-label"), ["PREP"]);
    assert_eq!(h.texts(".prep .band-label"), ["Prep"]);
    assert_eq!(h.count(".prep .band-label .shape-triangle"), 1);
    assert_eq!(
        inner(".meal-type"),
        ["BREAKFAST", "LUNCH", "DINNER", "SNACK"]
    );
    // The kcal figure has its unit in its own element; no icon element is left.
    assert_eq!(h.texts(".meal-kcal-unit"), ["kcal", "kcal", "kcal", "kcal"]);
    assert_eq!(h.count(".meal-icon"), 0);
    // The expand indicator is a plain plus or minus.
    assert_eq!(h.texts(".meal-indicator"), ["−", "−", "−", "−"]);
    // The band label sits above the list label and the prep title.
    assert_eq!(
        h.js("(() => { const l = document.querySelector('.shop .band-label').getBoundingClientRect(); \
              const t = document.querySelector('.shop-label').getBoundingClientRect(); return l.bottom <= t.top + 0.5; })()"),
        true
    );
    assert_eq!(
        h.js("(() => { const l = document.querySelector('.prep .band-label').getBoundingClientRect(); \
              const t = document.querySelector('.prep-title').getBoundingClientRect(); return l.bottom <= t.top + 0.5; })()"),
        true
    );
    // The year page's week links carry a decorative arrow.
    h.goto_page("/2026");
    assert_eq!(h.count(".week-link .week-link-arrow[aria-hidden=true]"), 1);
    assert_eq!(h.text(".week-link-arrow"), "›");
}

/// JS defining the WCAG contrast ratios of the design system's pairs on the current page.
const CONTRAST_JS: &str = r#"(() => {
  const parse = c => c.match(/[\d.]+/g).map(Number);
  const lin = v => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
  const lum = c => 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
  const ratio = (a, b) => { const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); };
  const bgOf = el => {
    for (let e = el; e; e = e.parentElement) {
      const p = parse(getComputedStyle(e).backgroundColor);
      if (p.length < 4 || p[3] > 0) return p;
    }
    return [255, 255, 255];
  };
  const q = sel => document.querySelector(sel);
  const text = sel => ratio(parse(getComputedStyle(q(sel)).color), bgOf(q(sel)));
  const bg = (sel, ground) => ratio(parse(getComputedStyle(q(sel)).backgroundColor), ground);
  const ring = sel => ratio(parse(getComputedStyle(q(sel)).boxShadow), bgOf(q(sel).parentElement));
  const cal = bgOf(q('#calendar'));
  const blue = bgOf(q('#calendar .cal-selected'));
  return {
    body: text('.meal-name'),
    bodyOnPaper: text('.day-heading'),
    mealType: text('.meal-type'),
    mealKcalUnit: text('.meal-kcal-unit'),
    subtitle: text('.subtitle'),
    tip: text('.tip-label'),
    shopTitle: text('.shop-label'),
    shopLabel: text('.shop .band-label'),
    shopNote: text('.shop-note'),
    prepTitle: text('.prep-title'),
    prepLabel: text('.prep .band-label'),
    prepCovers: text('.prep-covers'),
    badgeTwo: text('.badge-two'),
    summaryLabel: text('.summary-label'),
    reminderBody: text('.reminder-body'),
    footer: text('.theme-label'),
    selectedDate: text('#calendar .cal-selected'),
    weekNumber: Math.min(...[...document.querySelectorAll('#calendar .cal-row .cal-wk')]
      .map(e => ratio(parse(getComputedStyle(e).color), bgOf(e)))),
    todayRing: ring('#calendar .cal-today:not(.cal-selected)'),
    stepBorder: ratio(parse(getComputedStyle(q('a.day-step[href]')).borderTopColor), bgOf(q('.day-row'))),
    shopMark: bg('#calendar a[href="/2026-10-28"] .cal-mark-shopping', cal),
    prepMark: bg('#calendar a[href="/2026-10-28"] .cal-mark-prep', cal),
    shopMarkSelected: bg('#calendar .cal-selected .cal-mark-shopping', blue),
    prepMarkSelected: bg('#calendar .cal-selected .cal-mark-prep', blue),
  };
})()"#;

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn text_and_marks_meet_contrast_in_both_themes() {
    let h = redesign_day(1280);
    h.open_calendar();
    h.expand_redesign_day();
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        let scheme = t.scheme;
        let r = h.js(CONTRAST_JS);
        let ratio = |name: &str| {
            r[name]
                .as_f64()
                .unwrap_or_else(|| panic!("{name} missing: {r}"))
        };
        for name in [
            "body",
            "bodyOnPaper",
            "mealType",
            "mealKcalUnit",
            "subtitle",
            "tip",
            "shopTitle",
            "shopLabel",
            "shopNote",
            "prepTitle",
            "prepLabel",
            "prepCovers",
            "badgeTwo",
            "summaryLabel",
            "reminderBody",
            "footer",
            "selectedDate",
            "weekNumber",
        ] {
            assert!(ratio(name) >= 4.5, "{scheme}: {name} {}", ratio(name));
        }
        for name in [
            "todayRing",
            "stepBorder",
            "shopMark",
            "prepMark",
            "shopMarkSelected",
            "prepMarkSelected",
        ] {
            assert!(ratio(name) >= 3.0, "{scheme}: {name} {}", ratio(name));
        }
    }
}

/// The focused element's outline as [style, width, colour]; its offset must be 2px.
fn focus_outline(h: &Harness, what: &str) -> Vec<String> {
    let mut o: Vec<String> = serde_json::from_value(h.js(
        "(() => { const s = getComputedStyle(document.activeElement); \
          return [s.outlineStyle, s.outlineWidth, s.outlineColor, s.outlineOffset]; })()",
    ))
    .unwrap_or_else(|_| panic!("{what}"));
    assert_eq!(o[3], "2px", "{what} outline offset");
    o.truncate(3);
    o
}

/// No ancestor of the focused element clips (its overflow is `visible`), so an outside outline shows whole.
fn assert_no_clipping_ancestor(h: &Harness) {
    let clipped = h.js("(() => { const out = []; \
        for (let e = document.activeElement.parentElement; e; e = e.parentElement) { \
          const s = getComputedStyle(e); \
          if (s.overflowX !== 'visible' || s.overflowY !== 'visible') out.push(e.className || e.tagName); } \
        return out; })()");
    assert_eq!(clipped, serde_json::json!([]), "clipping ancestors");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn summary_figures_stay_inside_their_columns_at_every_width() {
    for width in [320, 375, 421, 480, 768, 900, 1280] {
        let h = redesign_day(width);
        let r = h.js("(() => { const items = [...document.querySelectorAll('.summary-item')]; \
            const boxes = items.map(i => i.getBoundingClientRect()); \
            const bad = []; \
            items.forEach((item, k) => { \
              const v = item.querySelector('.summary-value'); \
              if (v.scrollWidth > v.clientWidth) bad.push(`scroll ${k}`); \
              const range = document.createRange(); range.selectNodeContents(v); \
              const c = range.getBoundingClientRect(); const b = boxes[k]; \
              if (c.left < b.left - 0.5 || c.right > b.right + 0.5 || c.top < b.top - 2 || c.bottom > b.bottom + 0.5) bad.push(`outside ${k}`); \
              if (k > 0 && boxes[k - 1].right > b.left + 0.5) bad.push(`overlap ${k}`); }); \
            return bad; })()");
        assert_eq!(r, serde_json::json!([]), "width {width}");
        assert_eq!(h.count(".summary-value"), 5);
    }
}

/// The open calendar's layout problems on the current page: overflow of the column or viewport, cells outside
/// the calendar, and day cells that are not circles.
const CALENDAR_FIT_JS: &str = r#"(() => {
  const bad = [];
  const cal = document.querySelector('#calendar').getBoundingClientRect();
  const app = document.querySelector('.app');
  const a = app.getBoundingClientRect(), s = getComputedStyle(app);
  const left = a.left + parseFloat(s.borderLeftWidth) + parseFloat(s.paddingLeft);
  const right = a.right - parseFloat(s.borderRightWidth) - parseFloat(s.paddingRight);
  if (cal.left < left - 0.5 || cal.right > right + 0.5) bad.push(`calendar ${cal.left}..${cal.right} not in ${left}..${right}`);
  if (document.documentElement.scrollWidth > window.innerWidth) bad.push(`scrollWidth ${document.documentElement.scrollWidth} > ${window.innerWidth}`);
  const cells = [...document.querySelectorAll('#calendar .cal-row a, #calendar .cal-row span.cal-cell')];
  if (cells.length < 28) bad.push(`only ${cells.length} cells`);
  cells.forEach(c => {
    const b = c.getBoundingClientRect();
    if (b.left < cal.left - 0.5 || b.right > cal.right + 0.5) bad.push(`cell ${c.textContent} outside calendar`);
    if (Math.abs(b.width - b.height) > 0.5) bad.push(`cell ${c.textContent} ${b.width}x${b.height}`);
  });
  return bad;
})()"#;

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn open_calendar_fits_the_column_at_phone_widths() {
    for width in [320, 375] {
        let h = redesign_day(width);
        h.open_calendar();
        assert_eq!(
            h.js(CALENDAR_FIT_JS),
            serde_json::json!([]),
            "day page {width}"
        );
        // The marks stay inside their cells.
        assert_eq!(
            h.js("[...document.querySelectorAll('#calendar .cal-marks')].filter(m => { \
                  const c = m.parentElement.getBoundingClientRect(), b = m.getBoundingClientRect(); \
                  return b.left < c.left - 0.5 || b.right > c.right + 0.5 || b.bottom > c.bottom + 0.5; }).length"),
            0,
            "marks {width}"
        );
    }
    let h = Opts::new().width(320).path("/2026-02-30").launch();
    assert_eq!(h.status(), 404);
    assert!(h.calendar_shown());
    assert_eq!(
        h.js(CALENDAR_FIT_JS),
        serde_json::json!([]),
        "invalid date 320"
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn selected_today_keeps_an_on_blue_ring() {
    let h = Opts::new().path("/").launch();
    h.open_calendar();
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        let cell = "#calendar .cal-selected.cal-today";
        assert_eq!(h.count(cell), 1, "{}", t.scheme);
        let shadow = |h: &Harness| h.styles(cell, &["boxShadow"])[0].clone();
        let resting = shadow(&h);
        assert_ne!(resting, "none", "{} resting", t.scheme);
        assert!(
            resting.contains(&t.rgb("on-blue")),
            "{} {resting}",
            t.scheme
        );
        assert_eq!(h.styles(cell, &["backgroundColor"]), [t.rgb("blue")]);
        h.tab.find_element(cell).unwrap().move_mouse_over().unwrap();
        assert_eq!(shadow(&h), resting, "{} hovered", t.scheme);
        let ratio = h.js(&format!(
            "(() => {{ const p = c => c.match(/[\\d.]+/g).map(Number); \
              const lin = v => {{ v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }}; \
              const lum = c => 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]); \
              const [hi, lo] = [lum(p('{}')), lum(p('{}'))].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); }})()",
            t.rgb("on-blue"),
            t.rgb("blue")
        ));
        assert!(ratio.as_f64().unwrap() >= 3.0, "{} {ratio}", t.scheme);
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn keyboard_focus_shows_a_two_pixel_focus_outline() {
    let h = redesign_day(1280);
    h.open_calendar();
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        let ink = t.rgb("ink");
        // A link, a button and a radio, each with a 2px solid ink outline offset by 2px.
        for selector in [
            "#calendar a.cal-cell",
            "a.day-step[href]",
            ".day-toggle",
            ".meal-toggle",
            ".shop-toggle",
            ".prep-toggle",
            "#calendar button",
        ] {
            h.tab_to(selector);
            let what = format!("{} {selector}", t.scheme);
            assert_eq!(
                focus_outline(&h, &what)[..],
                ["solid", "2px", &ink],
                "{what}"
            );
            assert_no_clipping_ancestor(&h);
        }
        // The radio: its own outline and the label text beside it, which is what shows.
        h.tab_to("input[name=theme]");
        let what = format!("{} radio", t.scheme);
        assert_eq!(
            focus_outline(&h, &what)[..],
            ["solid", "2px", &ink],
            "{what}"
        );
        assert_eq!(
            h.js(
                "(() => { const s = getComputedStyle(document.activeElement.nextElementSibling); \
                  return [s.outlineStyle, s.outlineWidth, s.outlineColor, s.outlineOffset]; })()"
            ),
            serde_json::json!(["solid", "2px", ink, "2px"]),
            "{what} label"
        );
        assert_no_clipping_ancestor(&h);
    }
    // A card toggle expands with Enter and collapses again.
    h.tab_to(".prep-toggle");
    h.press("Enter");
    h.wait_for("document.querySelectorAll('.prep-body').length === 1");
    h.press("Enter");
    h.wait_for("document.querySelectorAll('.prep-body').length === 0");

    // Header links on the year page.
    let h = Opts::new().path("/2026").launch();
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        for selector in ["a.all-weeks", "a.week-link"] {
            h.tab_to(selector);
            let what = format!("{} {selector}", t.scheme);
            assert_eq!(
                focus_outline(&h, &what)[..],
                ["solid", "2px", &t.rgb("ink")],
                "{what}"
            );
            assert_no_clipping_ancestor(&h);
        }
    }
    // The day toggle at a narrow width, and the meal toggle by pointer.
    let h = redesign_day(375);
    h.tab_to(".day-toggle");
    assert_eq!(
        h.js("(() => { const s = getComputedStyle(document.activeElement); return [s.outlineStyle, s.outlineWidth, s.outlineColor]; })()"),
        serde_json::json!(["solid", "2px", LIGHT.rgb("ink")])
    );
    h.tab_to(".meal-toggle");
    h.press("Enter");
    h.wait_for("document.querySelectorAll('.meal-details').length === 1");
    h.click(".meal-toggle");
    h.wait_for("document.querySelectorAll('.meal-details').length === 0");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_014() {
    let sql = bare_weeks(&[(2026, 43), (2026, 46)]);
    // Capture the page as loaded (light), then again after the toggle switches to dark.
    let both = |h: &Harness, page: &str, width: u32| {
        for t in [LIGHT, DARK] {
            h.use_theme(&t);
            h.capture_file(&format!("014-{page}-{width}-{}.png", t.scheme));
        }
        h.use_theme(&LIGHT);
    };

    // The day with shopping, prep, an expanded meal and a reminder: calendar closed, then open.
    for width in [375, 1280] {
        let h = redesign_day(width);
        h.expand_redesign_day();
        assert!(!h.calendar_shown());
        both(&h, "day", width);
        h.open_calendar();
        both(&h, "day-calendar", width);
    }

    // The year page, the missing-plan page and an invalid date at phone width.
    let h = Opts::new().width(375).sql(&sql).path("/2026").launch();
    assert_eq!(h.count(".week-link"), 3);
    both(&h, "year", 375);
    let h = Opts::new().width(375).sql(&sql).path("/2026/45").launch();
    assert_eq!(h.status(), 404);
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    both(&h, "missing-plan", 375);
    h.open_calendar();
    both(&h, "missing-plan-calendar", 375);
    let h = Opts::new().width(375).path("/2026-02-30").launch();
    assert_eq!(h.status(), 404);
    both(&h, "invalid-date", 375);
}

// ---- Feature 16: today link ----

/// The Today link's `href` on the current page, or `None` when the page has no `.today-link`.
fn today_href(h: &Harness) -> Option<String> {
    let v = h.js("(() => { const a = document.querySelector('.today-link'); return a && a.getAttribute('href'); })()");
    v.as_str().map(str::to_string)
}

/// Whether the header has a `.header-meta` row.
fn has_header_meta(h: &Harness) -> bool {
    h.count(".header .header-meta") == 1
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn today_link_shows_on_other_days_weeks_and_missing_pages() {
    // The harness's today is Wednesday, December 2 (W49, no plan).
    let h = Opts::new().path("/2026-10-28").launch();
    for path in ["/2026-10-28", "/2026/44", "/2026/53", "/2026-11-11"] {
        h.goto_page(path);
        assert_eq!(h.count("a.today-link"), 1, "{path}");
        assert_eq!(
            h.js("(() => { const a = document.querySelector('a.today-link'); \
                  return [a.className, a.textContent, a.children.length, a.hasAttribute('aria-label'), \
                          a.getAttribute('href'), a === document.querySelector('.header-meta').lastElementChild]; })()"),
            serde_json::json!(["all-weeks today-link", "Today", 0, false, "/2026-12-02", true]),
            "{path}"
        );
    }
    // Following it opens today's missing-plan page with the calendar closed.
    h.click("a.today-link");
    h.wait_page("/2026-12-02");
    assert_eq!(h.heading(), "Wednesday, December 2");
    assert_eq!(h.texts(".empty"), ["No meal plan for this week yet."]);
    assert!(!h.calendar_shown());
    assert_eq!(h.count(".today-link"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn today_link_leads_to_todays_date_in_a_planned_week() {
    let h = Opts::new()
        .sql(&bare_weeks(&[(2026, 43)]))
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/2026-10-26")
        .launch();
    for path in ["/2026-10-26", "/2026/43"] {
        h.goto_page(path);
        assert_eq!(today_href(&h).as_deref(), Some("/2026-10-28"), "{path}");
        h.click("a.today-link");
        h.wait_page("/2026-10-28");
        assert_eq!(h.status(), 200, "{path}");
        assert_eq!(h.heading(), "Wednesday, October 28");
        assert!(!h.calendar_shown());
        assert_eq!(h.count(".today-link"), 0);
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn today_link_uses_the_server_today_not_the_browser_clock() {
    // 2026-11-01T22:30Z is Sunday, November 1 in Berlin.
    let h = Opts::new()
        .clock(clock_at(2026, 11, 1, 22, 30, 0))
        .path("/2026-10-28")
        .launch();
    for tz in ["Pacific/Kiritimati", "Pacific/Pago_Pago"] {
        h.set_browser_timezone(tz);
        h.goto("/2026-10-28");
        assert_eq!(today_href(&h).as_deref(), Some("/2026-11-01"), "{tz}");
        for path in ["/", "/2026-11-01"] {
            h.goto(path);
            assert_eq!(today_href(&h), None, "{tz} {path}");
        }
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn no_today_link_when_the_displayed_date_is_today() {
    // Missing-plan pages for today: no link and no header row at all (no subtitle).
    let h = Opts::new().path("/").launch();
    for path in ["/", "/2026/49", "/2026-12-02"] {
        h.goto_page(path);
        assert_eq!(today_href(&h), None, "{path}");
        assert!(!has_header_meta(&h), "{path}");
    }
    // A planned today: the row holds only the subtitle.
    let h = Opts::new()
        .clock(clock_at(2026, 10, 28, 11, 0, 0))
        .path("/")
        .launch();
    for path in ["/", "/2026/44", "/2026-10-28"] {
        h.goto_page(path);
        assert_eq!(today_href(&h), None, "{path}");
        assert_eq!(
            h.js("[...document.querySelector('.header-meta').children].map(e => e.tagName + '.' + e.className)"),
            serde_json::json!(["P.subtitle"]),
            "{path}"
        );
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn other_pages_have_no_today_link() {
    let h = Opts::new().path("/2026").launch();
    assert_eq!(h.status(), 200);
    assert_eq!(h.count(".today-link"), 0);
    assert_eq!(h.texts("a.all-weeks"), ["This week"]);
    for (path, status) in [("/2026/54", 404), ("/2026-02-30", 404), ("/2031", 404)] {
        h.goto_page(path);
        assert_eq!(h.status(), status, "{path}");
        assert_eq!(h.count(".today-link"), 0, "{path}");
    }
    let h = Opts::new().db(Db::NoTables).path("/").launch();
    assert_eq!(h.status(), 503);
    assert_eq!(h.count(".today-link"), 0);
}

/// Distance in px between the link's right edge and the header's.
fn link_right_gap(h: &Harness) -> f64 {
    (h.rect(".today-link")[1] - h.rect(".header")[1]).abs()
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn today_link_sits_at_the_right_end_of_the_row() {
    for width in [1280, 375] {
        let h = Opts::new().width(width).path("/2026/44").launch();
        // With a subtitle.
        assert!(h.count(".header-meta > .subtitle") == 1);
        assert!(link_right_gap(&h) < 1.0, "{width} subtitle");
        // Without one: the row holds only the link, far from the left edge.
        h.goto_page("/2026/53");
        assert_eq!(h.count(".header-meta > *"), 1);
        assert!(link_right_gap(&h) < 1.0, "{width} no subtitle");
        assert!(
            h.rect(".today-link")[0] > h.rect(".header")[0] + 100.0,
            "{width} left"
        );
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn a_long_subtitle_wraps_beside_the_today_link_at_320px() {
    let long = "A long week of batch cooking with several make-ahead dishes, freezer staples and snacks for school and work, planned around a busy schedule with guests on the weekend and leftovers on Monday evening.";
    let sql =
        format!("UPDATE weeks SET subtitle = '{long}' WHERE iso_year = 2026 AND iso_week = 44;");
    let h = Opts::new().width(320).sql(&sql).path("/2026/44").launch();
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= window.innerWidth"),
        true
    );
    let sub = h.rect(".subtitle");
    let link = h.rect(".today-link");
    assert!(sub[3] - sub[2] > 40.0, "subtitle wraps: {sub:?}");
    assert!(sub[1] <= link[0], "subtitle ends before the link");
    assert!(
        (link[2] - sub[2]).abs() < 20.0,
        "link on the subtitle's first line"
    );
    assert!(link_right_gap(&h) < 1.0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn today_link_matches_header_links_in_both_themes() {
    let year = Opts::new().path("/2026").launch();
    let h = Opts::new().path("/2026/44").launch();
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        year.use_theme(&t);
        // Move the pointer off the link (the previous round left it there).
        h.tab
            .find_element(".title")
            .unwrap()
            .move_mouse_over()
            .unwrap();
        let props = ["color", "fontSize", "textDecorationColor"];
        assert_eq!(
            h.styles(".today-link", &props),
            [t.rgb("ink-muted"), "14px".into(), t.rgb("line-strong")],
            "{}",
            t.scheme
        );
        assert_eq!(
            h.styles(".today-link", &["color", "fontSize"]),
            year.styles("a.all-weeks", &["color", "fontSize"]),
            "{} like the year page",
            t.scheme
        );
        // Contrast of the text on its background.
        let ratio = h.js(
            "(() => { const parse = c => c.match(/[\\d.]+/g).map(Number); \
              const lin = v => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }; \
              const lum = c => 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]); \
              const el = document.querySelector('.today-link'); \
              let bg = [255, 255, 255]; \
              for (let e = el; e; e = e.parentElement) { const p = parse(getComputedStyle(e).backgroundColor); \
                if (p.length < 4 || p[3] > 0) { bg = p; break; } } \
              const [hi, lo] = [lum(parse(getComputedStyle(el).color)), lum(bg)].sort((x, y) => y - x); \
              return (hi + 0.05) / (lo + 0.05); })()",
        );
        assert!(ratio.as_f64().unwrap() >= 4.5, "{} {ratio}", t.scheme);
        // Hover turns it ink.
        h.tab
            .find_element(".today-link")
            .unwrap()
            .move_mouse_over()
            .unwrap();
        assert_eq!(
            h.styles(".today-link", &["color"]),
            [t.rgb("ink")],
            "{} hover",
            t.scheme
        );
        // Keyboard focus shows the shared outline.
        h.tab_to(".today-link");
        let what = format!("{} today link", t.scheme);
        assert_eq!(
            focus_outline(&h, &what)[..],
            ["solid", "2px", &t.rgb("ink")],
            "{what}"
        );
        assert_no_clipping_ancestor(&h);
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_016() {
    let both = |h: &Harness, page: &str, width: u32| {
        for t in [LIGHT, DARK] {
            h.use_theme(&t);
            h.capture_file(&format!("016-{page}-{width}-{}.png", t.scheme));
        }
        h.use_theme(&LIGHT);
    };
    for width in [375, 1280] {
        // A day other than today.
        let h = redesign_day(width);
        assert_eq!(today_href(&h).as_deref(), Some("/2026-10-28"));
        both(&h, "other-day", width);
        // Today.
        let h = Opts::new()
            .width(width)
            .clock(clock_at(2026, 10, 28, 11, 0, 0))
            .path("/2026-10-28")
            .launch();
        assert_eq!(today_href(&h), None);
        both(&h, "today", width);
    }
    // A missing-plan page for another week: the link without a subtitle.
    let h = Opts::new().width(375).path("/2026/53").launch();
    assert_eq!(h.status(), 404);
    assert_eq!(today_href(&h).as_deref(), Some("/2026-12-02"));
    both(&h, "missing-plan", 375);
}

// ---- Features 17 and 18: household profile, read-first ----

const SECTION_KEYS: [&str; 5] = ["household", "food", "kitchen", "planning", "other"];
const SECTION_TITLES: [&str; 5] = [
    "People",
    "Food preferences",
    "Kitchen",
    "Planning habits",
    "Other context",
];
const SECTION_HINTS: [&str; 5] = [
    "Who eats, portions and dietary restrictions.",
    "Likes, dislikes and cuisines.",
    "Equipment, storage and limitations.",
    "Cooking time, shopping, budget, leftovers and batch prep.",
    "Anything else worth knowing.",
];
const PROFILE_XSS: &str = "</script><img src=x onerror=alert(1)>\nsecond line";

fn section_key(key: &str) -> sous::model::SectionKey {
    sous::model::SectionKey::parse(key).unwrap()
}

/// An empty database with the writer on, opened on the profile page.
fn profile_page(width: u32) -> Harness {
    Opts::new()
        .db(Db::Empty)
        .mcp()
        .width(width)
        .path("/profile")
        .launch()
}

impl Harness {
    /// The stored text of a section, read through a separate connection.
    fn profile_text(&self, key: &str) -> String {
        self.rt.block_on(async {
            let pool = db::open_read_pool(&self.db_path).await.unwrap();
            let profile = sous::profile::load_profile(&pool).await.unwrap();
            pool.close().await;
            profile
                .sections
                .into_iter()
                .find(|s| s.key.as_str() == key)
                .unwrap()
                .text
        })
    }

    /// Another writer replaces a section's text (with the right revision) through `profile::update_section`.
    fn profile_write(&self, key: &str, text: &str) {
        self.rt.block_on(async {
            let read = db::open_read_pool(&self.db_path).await.unwrap();
            let write = db::open_write_pool(&self.db_path).await.unwrap();
            let profile = sous::profile::load_profile(&read).await.unwrap();
            let current = profile
                .sections
                .into_iter()
                .find(|s| s.key.as_str() == key)
                .unwrap();
            sous::profile::update_section(&write, section_key(key), text, &current.revision)
                .await
                .unwrap_or_else(|_| panic!("update_section {key}"));
            read.close().await;
            write.close().await;
        });
    }

    /// Set a section's textarea as a user's edit would (React sees an input event).
    fn fill(&self, key: &str, text: &str) {
        self.js(&format!(
            "(() => {{ const t = document.getElementById('profile-{key}'); \
             const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set; \
             set.call(t, {}); t.dispatchEvent(new Event('input', {{ bubbles: true }})); }})()",
            serde_json::to_string(text).unwrap()
        ));
        self.wait_for(&format!(
            "document.getElementById('profile-{key}').value === {}",
            serde_json::to_string(text).unwrap()
        ));
    }

    fn draft(&self, key: &str) -> String {
        self.js(&format!("document.getElementById('profile-{key}').value"))
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The selector of a section.
    fn section(key: &str) -> String {
        format!("section.field[data-key=\"{key}\"]")
    }

    fn edit_selector(key: &str) -> String {
        format!("{} .field-row > button.button", Self::section(key))
    }

    fn save_selector(key: &str) -> String {
        format!(
            "{} .field-actions > button:nth-child(1)",
            Self::section(key)
        )
    }

    fn cancel_selector(key: &str) -> String {
        format!(
            "{} .field-actions > button:nth-child(2)",
            Self::section(key)
        )
    }

    /// Click Edit on a section and wait for its field.
    fn open(&self, key: &str) {
        self.click(&Self::edit_selector(key));
        self.wait_for(&format!(
            "document.getElementById('profile-{key}') !== null"
        ));
    }

    fn is_open(&self, key: &str) -> bool {
        self.js(&format!(
            "document.getElementById('profile-{key}') !== null"
        ))
        .as_bool()
        .unwrap()
    }

    /// The saved text a section shows in read mode, or `None` while it shows its hint or its field.
    fn shown_text(&self, key: &str) -> Option<String> {
        self.js(&format!(
            "(() => {{ const n = document.querySelector('{} .field-text'); return n ? n.textContent : null; }})()",
            Self::section(key)
        ))
        .as_str()
        .map(str::to_string)
    }

    fn selector_disabled(&self, selector: &str) -> bool {
        self.js(&format!("document.querySelector('{selector}').disabled"))
            .as_bool()
            .unwrap()
    }

    fn save_disabled(&self, key: &str) -> bool {
        self.selector_disabled(&Self::save_selector(key))
    }

    fn cancel_disabled(&self, key: &str) -> bool {
        self.selector_disabled(&Self::cancel_selector(key))
    }

    /// The text of a section's live region.
    fn status_text(&self, key: &str) -> String {
        self.js(&format!(
            "document.querySelector('{} [role=status]').textContent",
            Self::section(key)
        ))
        .as_str()
        .unwrap()
        .to_string()
    }

    /// The texts of every button in a section.
    fn section_buttons(&self, key: &str) -> Vec<String> {
        self.texts(&format!("{} button", Self::section(key)))
    }

    /// The selector of the conflict notice's "Keep mine" or "Use saved" button.
    fn notice_button(key: &str, label: &str) -> String {
        format!(
            "{} .notice-actions button:nth-child({})",
            Self::section(key),
            if label == "Keep mine" { 1 } else { 2 }
        )
    }

    /// The conflict notice's saved text, or `None` while no notice is open.
    fn notice_text(&self, key: &str) -> Option<String> {
        self.js(&format!(
            "(() => {{ const n = document.querySelector('{} .notice-text'); \
             return n ? n.textContent : null; }})()",
            Self::section(key)
        ))
        .as_str()
        .map(str::to_string)
    }

    fn wait_notice(&self, key: &str, text: &str) {
        self.wait_for(&format!(
            "(() => {{ const n = document.querySelector('{} .notice-text'); \
             return n !== null && n.textContent === {}; }})()",
            Self::section(key),
            serde_json::to_string(text).unwrap()
        ));
    }

    fn wait_status(&self, key: &str, text: &str) {
        self.wait_for(&format!(
            "document.querySelector('{} [role=status]').textContent === {}",
            Self::section(key),
            serde_json::to_string(text).unwrap()
        ));
    }

    /// Wait until a section's field is closed and its Edit button has focus.
    fn wait_closed_on_edit(&self, key: &str) {
        self.wait_for(&format!(
            "document.getElementById('profile-{key}') === null \
             && document.activeElement === document.querySelector('{}')",
            Self::edit_selector(key)
        ));
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn profile_page_has_five_labelled_sections_in_order() {
    let h = profile_page(1280);
    assert_eq!(h.status(), 200);
    assert_eq!(h.js("document.title"), "Household — Meal Plan");
    assert_eq!(h.text(".title"), "Household");
    assert_eq!(h.count(".header-meta"), 0);
    assert_eq!(h.texts(".header > a.back-link"), ["Back to today"]);
    assert_eq!(h.count("a.all-weeks"), 0);
    assert_eq!(h.texts("h2.field-title"), SECTION_TITLES);
    assert_eq!(h.count("textarea"), 0);
    assert_eq!(h.count("[role=status]"), 5);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn profile_sections_load_in_read_mode() {
    let h = profile_page(1280);
    assert_eq!(h.count("textarea"), 0);
    assert_eq!(h.texts("h2.field-title"), SECTION_TITLES);
    assert_eq!(h.texts(".field-row > button.button"), ["Edit"; 5]);
    assert_eq!(
        h.js("[...document.querySelectorAll('.field-row > button.button')].map(b => b.getAttribute('aria-label'))"),
        serde_json::json!([
            "Edit People",
            "Edit Food preferences",
            "Edit Kitchen",
            "Edit Planning habits",
            "Edit Other context"
        ])
    );
    assert_eq!(h.texts(".field-hint"), SECTION_HINTS);
    assert_eq!(h.count(".field-text"), 0);
    assert_eq!(h.count(".empty"), 0);
    assert_eq!(
        h.js("document.body.innerText.includes('No household details yet.')"),
        false
    );
    // Saved text replaces the hint, with its line breaks; the other sections keep theirs.
    h.profile_write("household", "Two adults\nOne toddler\n\nNo peanuts");
    h.profile_write("planning", "Shop on Saturday.\nBatch cook on Sunday.");
    h.goto_page("/profile");
    assert_eq!(
        h.shown_text("household").as_deref(),
        Some("Two adults\nOne toddler\n\nNo peanuts")
    );
    assert_eq!(
        h.shown_text("planning").as_deref(),
        Some("Shop on Saturday.\nBatch cook on Sunday.")
    );
    assert_eq!(
        h.js("document.querySelector('section[data-key=\"household\"] .field-text').innerText"),
        "Two adults\nOne toddler\n\nNo peanuts"
    );
    assert_eq!(h.count(".field-text"), 2);
    assert_eq!(h.count("textarea"), 0);
    for key in ["household", "planning"] {
        assert_eq!(
            h.count(&format!("{} .field-hint", Harness::section(key))),
            0,
            "{key}"
        );
    }
    assert_eq!(
        h.texts(".field-hint"),
        [SECTION_HINTS[1], SECTION_HINTS[2], SECTION_HINTS[4]]
    );
    assert_eq!(h.count(".empty"), 0);
    assert_eq!(
        h.js("document.body.innerText.includes('No household details yet.')"),
        false
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn profile_fields_are_labelled_described_and_announce_status() {
    let h = profile_page(1280);
    for (i, key) in SECTION_KEYS.iter().enumerate() {
        let one_status_last = format!(
            "(() => {{ const s = document.querySelector('{}'); \
             return s.querySelectorAll('[role=status]').length === 1 \
             && s.lastElementChild.getAttribute('role') === 'status'; }})()",
            Harness::section(key)
        );
        assert_eq!(h.js(&one_status_last), true, "{key}: read mode");
        h.open(key);
        // The title is the textarea's label (its accessible name), the hint its description.
        assert_eq!(
            h.js(&format!(
                "(() => {{ const t = document.getElementById('profile-{key}'); \
                 return [t.labels.length, t.labels[0].textContent, \
                 document.getElementById(t.getAttribute('aria-describedby')).textContent, \
                 t.closest('.field').querySelectorAll('[role=status]').length]; }})()"
            )),
            serde_json::json!([1, SECTION_TITLES[i], SECTION_HINTS[i], 1]),
            "{key}"
        );
        assert_eq!(h.js(&one_status_last), true, "{key}: edit mode");
    }
    assert_eq!(
        h.js("document.querySelector('label.field-title').textContent"),
        "People"
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn edit_then_cancel_keeps_the_saved_text_and_focuses_edit() {
    let h = profile_page(1280);
    h.profile_write("kitchen", "Oven");
    h.goto_page("/profile");
    h.open("kitchen");
    h.wait_for("document.activeElement === document.getElementById('profile-kitchen')");
    assert_eq!(h.draft("kitchen"), "Oven");
    assert!(h.save_disabled("kitchen"));
    h.fill("kitchen", "Oven and hob");
    assert!(!h.save_disabled("kitchen"));
    assert!(!h.cancel_disabled("kitchen"));
    h.click(&Harness::cancel_selector("kitchen"));
    h.wait_closed_on_edit("kitchen");
    assert_eq!(h.count("textarea"), 0);
    assert_eq!(h.shown_text("kitchen").as_deref(), Some("Oven"));
    assert_eq!(h.profile_text("kitchen"), "Oven");
    assert_eq!(h.status_text("kitchen"), "");
    // The draft was dropped.
    h.open("kitchen");
    assert_eq!(h.draft("kitchen"), "Oven");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn edit_then_save_closes_the_field_with_saved_and_focuses_edit() {
    let h = profile_page(1280);
    // A real click and real typing.
    h.open("food");
    h.wait_for("document.activeElement === document.getElementById('profile-food')");
    assert!(h.save_disabled("food"));
    h.tab.type_str("Loves soup").unwrap();
    h.wait_for("document.getElementById('profile-food').value === 'Loves soup'");
    assert!(!h.save_disabled("food"));
    h.click(&Harness::save_selector("food"));
    h.wait_status("food", "Saved.");
    h.wait_closed_on_edit("food");
    assert_eq!(h.shown_text("food").as_deref(), Some("Loves soup"));
    assert_eq!(h.profile_text("food"), "Loves soup");
    // A multi-line edit in another section, kept with its line breaks.
    h.open("kitchen");
    h.fill("kitchen", "Oven\nSlow cooker\n\nSmall fridge");
    h.click(&Harness::save_selector("kitchen"));
    h.wait_status("kitchen", "Saved.");
    h.wait_closed_on_edit("kitchen");
    assert_eq!(
        h.shown_text("kitchen").as_deref(),
        Some("Oven\nSlow cooker\n\nSmall fridge")
    );
    assert_eq!(h.status_text("food"), "Saved.");
    // The next Edit clears "Saved." in that section only.
    h.open("food");
    assert_eq!(h.status_text("food"), "");
    assert_eq!(h.status_text("kitchen"), "Saved.");
    // Reload: the text persists, and every section is back in read mode.
    h.goto_page("/profile");
    assert_eq!(h.count("textarea"), 0);
    assert_eq!(h.shown_text("food").as_deref(), Some("Loves soup"));
    assert_eq!(
        h.shown_text("kitchen").as_deref(),
        Some("Oven\nSlow cooker\n\nSmall fridge")
    );
    for key in ["household", "planning", "other"] {
        assert_eq!(h.shown_text(key), None, "{key}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn save_is_enabled_only_while_the_draft_differs_from_the_saved_text() {
    let h = profile_page(1280);
    h.open("other");
    assert!(h.save_disabled("other"));
    h.fill("other", "x");
    assert!(!h.save_disabled("other"));
    h.fill("other", "");
    assert!(h.save_disabled("other"));
    h.fill("other", "x");
    h.click(&Harness::save_selector("other"));
    h.wait_status("other", "Saved.");
    h.wait_closed_on_edit("other");
    h.open("other");
    assert!(h.save_disabled("other"));
    h.fill("other", "xy");
    assert!(!h.save_disabled("other"));
    h.fill("other", "x");
    assert!(h.save_disabled("other"));
    // A save in flight disables Save and Cancel: hold the request and look at the buttons.
    h.js("(() => { window.__release = null; const real = window.fetch; \
          window.fetch = (...args) => new Promise((resolve) => { window.__release = () => resolve(real(...args)); }); \
          return true; })()");
    h.fill("other", "xyz");
    h.click(&Harness::save_selector("other"));
    h.wait_for("window.__release !== null");
    assert!(h.save_disabled("other"));
    assert!(h.cancel_disabled("other"));
    h.js("window.__release()");
    h.wait_status("other", "Saved.");
    h.wait_closed_on_edit("other");
    assert_eq!(h.profile_text("other"), "xyz");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn keyboard_opens_edits_and_saves_every_section() {
    let h = profile_page(1280);
    for (i, key) in SECTION_KEYS.iter().enumerate() {
        let enter_or_space = if i % 2 == 0 { "Enter" } else { " " };
        h.tab_to(&Harness::edit_selector(key));
        h.press(enter_or_space);
        h.wait_for(&format!(
            "document.activeElement === document.getElementById('profile-{key}')"
        ));
        h.tab.type_str("x").unwrap();
        h.wait_for(&format!(
            "document.getElementById('profile-{key}').value === 'x'"
        ));
        h.press("Tab");
        assert_eq!(
            h.js(&format!(
                "document.activeElement === document.querySelector('{}')",
                Harness::save_selector(key)
            )),
            true,
            "{key}: Save follows its field"
        );
        h.press(enter_or_space);
        h.wait_status(key, "Saved.");
        h.wait_closed_on_edit(key);
        assert_eq!(h.profile_text(key), "x", "{key}");
        assert_eq!(h.shown_text(key).as_deref(), Some("x"), "{key}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn several_sections_can_be_open_and_save_on_their_own() {
    let h = profile_page(1280);
    h.open("food");
    h.open("kitchen");
    h.fill("food", "Soups");
    h.fill("kitchen", "Oven");
    h.click(&Harness::save_selector("food"));
    h.wait_status("food", "Saved.");
    h.wait_closed_on_edit("food");
    assert!(h.is_open("kitchen"));
    assert_eq!(h.draft("kitchen"), "Oven");
    assert_eq!(h.status_text("kitchen"), "");
    assert_eq!(h.profile_text("food"), "Soups");
    assert_eq!(h.profile_text("kitchen"), "");
    h.click(&Harness::save_selector("kitchen"));
    h.wait_status("kitchen", "Saved.");
    h.wait_closed_on_edit("kitchen");
    assert_eq!(h.profile_text("kitchen"), "Oven");
    assert_eq!(h.status_text("food"), "Saved.");
    assert_eq!(h.count("textarea"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn there_is_no_empty_state_whatever_is_saved() {
    let h = profile_page(1280);
    for text in ["", "Cook on Sundays"] {
        h.profile_write("planning", text);
        h.goto_page("/profile");
        assert_eq!(h.count(".empty"), 0);
        assert_eq!(
            h.js("document.body.innerText.includes('No household details yet.')"),
            false
        );
    }
    // Saving the only text away leaves the section showing its hint, and still no empty state.
    h.open("planning");
    h.fill("planning", "");
    h.click(&Harness::save_selector("planning"));
    h.wait_status("planning", "Saved.");
    h.wait_closed_on_edit("planning");
    assert_eq!(h.shown_text("planning"), None);
    assert_eq!(
        h.texts(&format!("{} .field-hint", Harness::section("planning"))),
        [SECTION_HINTS[3]]
    );
    assert_eq!(h.count(".empty"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn conflict_notice_shows_the_other_writers_text_and_use_saved_replaces_the_draft() {
    let h = profile_page(1280);
    h.open("household");
    h.fill("household", "mine");
    h.profile_write("household", "theirs\nsecond line");
    h.click(&Harness::save_selector("household"));
    h.wait_notice("household", "theirs\nsecond line");
    assert!(
        h.status_text("household")
            .starts_with("This section was changed elsewhere.")
    );
    assert_eq!(h.draft("household"), "mine", "the draft stays");
    assert_eq!(h.profile_text("household"), "theirs\nsecond line");
    // Save and Cancel are hidden: only Keep mine and Use saved are left.
    assert!(h.is_open("household"));
    assert_eq!(
        h.count(&format!("{} .field-actions", Harness::section("household"))),
        0
    );
    assert_eq!(h.section_buttons("household"), ["Keep mine", "Use saved"]);
    // Typing in the field leaves the notice open.
    h.fill("household", "mine, longer");
    assert_eq!(
        h.notice_text("household").as_deref(),
        Some("theirs\nsecond line")
    );
    assert_eq!(h.section_buttons("household"), ["Keep mine", "Use saved"]);
    // Line breaks are preserved in the display.
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('.notice-text')).whiteSpace"),
        "pre-wrap"
    );
    assert_eq!(
        h.js("document.querySelector('.notice-text').getBoundingClientRect().height >= 2 * 24"),
        true
    );
    h.click(&Harness::notice_button("household", "Use saved"));
    h.wait_closed_on_edit("household");
    assert_eq!(h.count(".notice"), 0);
    assert_eq!(
        h.shown_text("household").as_deref(),
        Some("theirs\nsecond line")
    );
    assert_eq!(h.status_text("household"), "");
    // The revision is the other writer's: the next edit saves without a conflict.
    h.open("household");
    assert_eq!(h.draft("household"), "theirs\nsecond line");
    assert!(h.save_disabled("household"));
    h.fill("household", "later");
    h.click(&Harness::save_selector("household"));
    h.wait_status("household", "Saved.");
    assert_eq!(h.profile_text("household"), "later");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn conflict_notice_says_so_when_the_saved_version_is_empty() {
    let h = profile_page(1280);
    h.profile_write("kitchen", "Oven");
    h.goto_page("/profile");
    h.open("kitchen");
    assert_eq!(h.draft("kitchen"), "Oven");
    // Another writer clears the section; then this page edits and saves.
    h.profile_write("kitchen", "");
    h.fill("kitchen", "Oven and hob");
    h.click(&Harness::save_selector("kitchen"));
    h.wait_notice("kitchen", "The saved version is empty.");
    assert_eq!(
        h.js("getComputedStyle(document.querySelector('section[data-key=\"kitchen\"] .notice-text')).color"),
        LIGHT.rgb("ink-muted")
    );
    assert_eq!(h.draft("kitchen"), "Oven and hob");
    h.click(&Harness::notice_button("kitchen", "Use saved"));
    h.wait_closed_on_edit("kitchen");
    assert_eq!(h.count(".notice"), 0);
    // Read mode shows the hint for the empty saved version.
    assert_eq!(h.shown_text("kitchen"), None);
    assert_eq!(
        h.texts(&format!("{} .field-hint", Harness::section("kitchen"))),
        [SECTION_HINTS[2]]
    );
    assert_eq!(h.status_text("kitchen"), "");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn keep_mine_stays_open_on_a_second_conflict_and_closes_on_success() {
    let h = profile_page(1280);
    h.open("food");
    h.fill("food", "mine");
    h.profile_write("food", "second writer");
    h.click(&Harness::save_selector("food"));
    h.wait_notice("food", "second writer");
    // A third writer changes the section before "Keep mine" is pressed.
    h.profile_write("food", "third writer");
    h.click(&Harness::notice_button("food", "Keep mine"));
    h.wait_notice("food", "third writer");
    assert_eq!(h.draft("food"), "mine");
    assert_eq!(h.profile_text("food"), "third writer");
    assert!(h.is_open("food"), "the field stays open");
    assert!(h.notice_text("food").is_some(), "the notice stays open");
    // A further "Keep mine" saves against the newer revision and closes the notice and the field.
    h.click(&Harness::notice_button("food", "Keep mine"));
    h.wait_status("food", "Saved.");
    h.wait_closed_on_edit("food");
    assert_eq!(h.notice_text("food"), None);
    assert_eq!(h.count(".notice"), 0);
    assert_eq!(h.shown_text("food").as_deref(), Some("mine"));
    assert_eq!(h.profile_text("food"), "mine");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn a_failed_save_shows_the_error_and_keeps_the_draft() {
    let h = profile_page(1280);
    // Every profile update now aborts: the endpoint answers 503.
    h.exec(
        "CREATE TRIGGER no_profile_updates BEFORE UPDATE ON profile_sections \
         BEGIN SELECT RAISE(ABORT, 'x'); END;",
    );
    h.open("kitchen");
    h.fill("kitchen", "Gas hob");
    h.click(&Harness::save_selector("kitchen"));
    h.wait_status("kitchen", "Couldn't save. Try again.");
    assert!(h.is_open("kitchen"));
    assert_eq!(h.draft("kitchen"), "Gas hob");
    assert!(!h.save_disabled("kitchen"), "Save can be pressed again");
    assert!(!h.cancel_disabled("kitchen"));
    assert_eq!(h.profile_text("kitchen"), "");
    // Once the failure is gone the same draft saves and the field closes.
    h.exec("DROP TRIGGER no_profile_updates");
    h.click(&Harness::save_selector("kitchen"));
    h.wait_status("kitchen", "Saved.");
    h.wait_closed_on_edit("kitchen");
    assert_eq!(h.profile_text("kitchen"), "Gas hob");
    assert_eq!(h.shown_text("kitchen").as_deref(), Some("Gas hob"));
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn cancel_after_a_failed_save_closes_the_field_with_the_old_text_and_no_status() {
    let h = profile_page(1280);
    h.profile_write("kitchen", "Oven");
    h.goto_page("/profile");
    h.exec(
        "CREATE TRIGGER no_profile_updates BEFORE UPDATE ON profile_sections \
         BEGIN SELECT RAISE(ABORT, 'x'); END;",
    );
    h.open("kitchen");
    h.fill("kitchen", "Gas hob");
    h.click(&Harness::save_selector("kitchen"));
    h.wait_status("kitchen", "Couldn't save. Try again.");
    h.click(&Harness::cancel_selector("kitchen"));
    h.wait_closed_on_edit("kitchen");
    assert_eq!(h.shown_text("kitchen").as_deref(), Some("Oven"));
    assert_eq!(h.status_text("kitchen"), "");
    assert_eq!(h.profile_text("kitchen"), "Oven");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn a_network_error_shows_the_error_and_keeps_the_draft() {
    let h = profile_page(1280);
    h.js("(window.fetch = () => Promise.reject(new TypeError('offline')), true)");
    h.open("other");
    h.fill("other", "Draft");
    h.click(&Harness::save_selector("other"));
    h.wait_status("other", "Couldn't save. Try again.");
    assert!(h.is_open("other"));
    assert_eq!(h.draft("other"), "Draft");
    assert!(!h.save_disabled("other"));
    assert!(!h.cancel_disabled("other"));
    // Typing clears the error.
    h.fill("other", "Draft 2");
    assert_eq!(h.status_text("other"), "");
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn profile_text_renders_as_literal_text_with_line_breaks() {
    let h = profile_page(1280);
    h.profile_write("other", PROFILE_XSS);
    h.goto_page("/profile");
    // Read mode: literal text, two lines.
    assert_eq!(h.shown_text("other").as_deref(), Some(PROFILE_XSS));
    assert_eq!(h.count("img"), 0);
    assert_eq!(h.js("window.pwned === undefined"), true);
    assert_eq!(
        h.js("document.querySelector('.field-text').children.length"),
        0
    );
    assert_eq!(
        h.js("document.querySelector('.field-text').innerText"),
        PROFILE_XSS
    );
    // The field after Edit holds it just as literally.
    h.open("other");
    assert_eq!(h.draft("other"), PROFILE_XSS);
    assert_eq!(h.count("img"), 0);
    // The conflict notice shows another writer's text just as literally.
    h.open("household");
    h.fill("household", "mine");
    h.profile_write("household", PROFILE_XSS);
    h.click(&Harness::save_selector("household"));
    h.wait_notice("household", PROFILE_XSS);
    assert_eq!(h.count("img"), 0);
    assert_eq!(
        h.js("document.querySelector('.notice-text').innerText"),
        PROFILE_XSS
    );
    assert_eq!(
        h.js("document.querySelector('.notice-text').children.length"),
        0
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn footer_link_is_on_every_page_but_the_profile() {
    for (mode, paths) in [
        (
            Db::Fixture,
            vec![
                ("/2026/44", "/profile?from=/2026-10-26"),
                ("/2026-10-28", "/profile?from=/2026-10-28"),
                ("/2026/53", "/profile?from=/2026-12-28"),
                ("/", "/profile?from=/2026-12-02"),
                ("/2026", "/profile?from=/2026"),
                ("/2026/54", "/profile"),
                ("/2026-02-30", "/profile"),
            ],
        ),
        (Db::ClosedPool, vec![("/2026/44", "/profile")]),
    ] {
        let h = Opts::new().db(mode).mcp().path(paths[0].0).launch();
        for (path, href) in paths {
            h.goto_page(path);
            assert_eq!(h.count("a.footer-link"), 1, "{path}");
            assert_eq!(h.text("a.footer-link"), "Household", "{path}");
            assert_eq!(
                h.js("document.querySelector('a.footer-link').getAttribute('href')"),
                href,
                "{path}"
            );
            // Inside the footer, above the theme toggle, centred in the column.
            assert_eq!(
                h.js("(() => { const f = document.querySelector('.footer'); \
                      return f.firstElementChild === f.querySelector('a.footer-link') \
                      && f.children[1].classList.contains('theme'); })()"),
                true,
                "{path}"
            );
            let [l, r, _, _] = h.rect("a.footer-link");
            let [al, ar, _, _] = h.rect(".app");
            assert!(
                ((l + r) / 2.0 - (al + ar) / 2.0).abs() <= 1.0,
                "{path}: centred"
            );
            assert_eq!(
                h.js("(() => { const s = getComputedStyle(document.querySelector('a.footer-link')); \
                      return [s.fontSize, s.color, s.textDecorationLine, s.textDecorationColor]; })()"),
                serde_json::json!(["14px", LIGHT.rgb("ink-muted"), "underline", LIGHT.rgb("line-strong")]),
                "{path}"
            );
        }
    }
    let h = Opts::new().mcp().path("/profile").launch();
    assert_eq!(h.count("a.footer-link"), 0);
    assert_eq!(h.count(".footer .theme"), 1);
    // The link leads to the profile page.
    h.goto_page("/2026/44");
    h.click("a.footer-link");
    h.wait_page("/profile");
    assert_eq!(h.text(".title"), "Household");
    assert_eq!(h.count("a.footer-link"), 0);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn profile_page_is_one_column_without_overflow() {
    let long_word = "a".repeat(120);
    for width in [320, 375, 1280] {
        let h = profile_page(width);
        h.profile_write("food", &long_word);
        h.profile_write("household", "A long line with words\nand another line");
        h.goto_page("/profile");
        let viewport = h
            .js("document.documentElement.clientWidth")
            .as_f64()
            .unwrap();
        let [left, right, _, _] = h.rect(".app");
        assert!(right - left <= 560.0, "{width}: column {}", right - left);
        assert_eq!(right - left, viewport.min(560.0), "{width}");
        // Nothing overflows or leaves the column in this state.
        let fits = |state: &str| {
            assert_eq!(
                h.js(
                    "document.documentElement.scrollWidth <= document.documentElement.clientWidth"
                ),
                true,
                "{width}: {state}: overflow"
            );
            for sel in [
                ".field",
                ".field-row",
                ".field-title",
                ".field-text",
                ".field-hint",
                ".field-status",
                "textarea",
                ".notice",
                ".button",
            ] {
                assert_eq!(
                    h.js(&format!(
                        "[...document.querySelectorAll('{sel}')].every(e => {{ const r = e.getBoundingClientRect(); \
                         return r.left >= {left} - 0.5 && r.right <= {right} + 0.5; }})"
                    )),
                    true,
                    "{width}: {state}: {sel} inside the column"
                );
            }
        };
        // Read mode with a long unbroken word saved.
        fits("read");
        assert_eq!(h.shown_text("food").as_deref(), Some(long_word.as_str()));
        for key in SECTION_KEYS {
            // Edit sits at the row's right end, centred on the title.
            let section = Harness::section(key);
            let [_, edit_right, edit_top, edit_bottom] = h.rect(&Harness::edit_selector(key));
            let [_, row_right, _, _] = h.rect(&format!("{section} .field-row"));
            let [_, _, title_top, title_bottom] = h.rect(&format!("{section} h2.field-title"));
            assert!(
                (edit_right - row_right).abs() <= 1.0,
                "{width}: {key}: right edge"
            );
            assert!(
                ((edit_top + edit_bottom) / 2.0 - (title_top + title_bottom) / 2.0).abs() <= 1.0,
                "{width}: {key}: vertical centre"
            );
        }
        // With the long-word section open.
        h.open("food");
        fits("open");
        assert_eq!(
            h.js("getComputedStyle(document.querySelector('textarea')).resize"),
            "vertical"
        );
        assert_eq!(
            h.js("document.querySelector('textarea').getBoundingClientRect().height >= 4 * 24"),
            true
        );
        // During a conflict whose notice text is a long word.
        h.fill("food", "mine");
        h.profile_write("food", &"b".repeat(120));
        h.click(&Harness::save_selector("food"));
        h.wait_notice("food", &"b".repeat(120));
        fits("conflict");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn profile_controls_use_the_design_system_tokens_in_both_themes() {
    let h = profile_page(1280);
    // household: saved text, read mode. food: saved, "Saved." showing. other: empty, shows its hint.
    h.profile_write("household", "Two adults\nOne toddler");
    h.goto_page("/profile");
    h.open("food");
    h.fill("food", "Soups");
    h.click(&Harness::save_selector("food"));
    h.wait_status("food", "Saved.");
    h.wait_closed_on_edit("food");
    // kitchen: open with a changed draft (Save enabled); planning: open and unchanged (Save disabled).
    h.open("kitchen");
    h.fill("kitchen", "x");
    h.open("planning");
    let near = |a: f64, b: f64, what: &str| assert!((a - b).abs() < 0.5, "{what}: {a} vs {b}");
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        h.js("(document.activeElement && document.activeElement.blur(), true)");
        let style = |sel: &str, props: &str| {
            h.js(&format!(
                "(() => {{ const s = getComputedStyle(document.querySelector('{sel}')); return [{props}]; }})()"
            ))
        };
        let sec = |key: &str, part: &str| format!("{} {part}", Harness::section(key));
        // Read mode: the title, the text and the hint.
        assert_eq!(
            style(
                &sec("household", "h2.field-title"),
                "s.fontSize, s.fontWeight, s.color, s.lineHeight"
            ),
            serde_json::json!(["17px", "600", t.rgb("ink"), "23px"]),
            "{}",
            t.scheme
        );
        assert_eq!(
            style(
                &sec("household", ".field-text"),
                "s.fontSize, s.fontWeight, s.color, s.lineHeight, s.whiteSpace, s.overflowWrap"
            ),
            serde_json::json!(["16px", "400", t.rgb("ink"), "24px", "pre-wrap", "anywhere"]),
            "{}",
            t.scheme
        );
        assert_eq!(
            style(
                &sec("other", ".field-hint"),
                "s.fontSize, s.color, s.lineHeight"
            ),
            serde_json::json!(["14px", t.rgb("ink-muted"), "20px"]),
            "{}",
            t.scheme
        );
        assert_eq!(
            style(
                &Harness::edit_selector("household"),
                "s.backgroundColor, s.borderTopColor, s.borderTopStyle, s.color, s.fontWeight, s.fontSize"
            ),
            serde_json::json!([
                t.rgb("surface"),
                t.rgb("ink"),
                "solid",
                t.rgb("ink"),
                "600",
                "14px"
            ]),
            "{}",
            t.scheme
        );
        assert_eq!(
            style(
                &sec("food", "[role=status]"),
                "s.fontSize, s.color, s.lineHeight"
            ),
            serde_json::json!(["14px", t.rgb("ink-muted"), "20px"]),
            "{}",
            t.scheme
        );
        // Edit mode: the label.
        assert_eq!(
            style(
                &sec("kitchen", "label.field-title"),
                "s.fontSize, s.fontWeight, s.color"
            ),
            serde_json::json!(["17px", "600", t.rgb("ink")]),
            "{}",
            t.scheme
        );
        // Spacing in read mode.
        let [_, _, _, row_bottom] = h.rect(&sec("household", ".field-row"));
        let [_, _, text_top, text_bottom] = h.rect(&sec("household", ".field-text"));
        near(text_top - row_bottom, 8.0, "text below the row");
        let [_, _, _, row_bottom] = h.rect(&sec("other", ".field-row"));
        let [_, _, hint_top, hint_bottom] = h.rect(&sec("other", ".field-hint"));
        near(hint_top - row_bottom, 8.0, "hint below the row");
        // An empty status takes no space.
        let [_, _, status_top, status_bottom] = h.rect(&sec("household", "[role=status]"));
        near(status_bottom - status_top, 0.0, "empty status height");
        let [_, _, _, section_bottom] = h.rect(&Harness::section("household"));
        near(section_bottom, text_bottom, "section ends with its text");
        let [_, _, _, section_bottom] = h.rect(&Harness::section("other"));
        near(section_bottom, hint_bottom, "section ends with its hint");
        // "Saved." sits 12px under the text.
        let [_, _, _, food_text_bottom] = h.rect(&sec("food", ".field-text"));
        let [_, _, saved_top, _] = h.rect(&sec("food", "[role=status]"));
        near(saved_top - food_text_bottom, 12.0, "status below the text");
        // Edit mode: the actions row, 12px under the field, Save before Cancel, 12px apart.
        let [_, _, _, field_bottom] = h.rect("#profile-kitchen");
        let [_, _, actions_top, _] = h.rect(&sec("kitchen", ".field-actions"));
        near(actions_top - field_bottom, 12.0, "actions below the field");
        let [_, save_right, _, _] = h.rect(&Harness::save_selector("kitchen"));
        let [cancel_left, _, _, _] = h.rect(&Harness::cancel_selector("kitchen"));
        near(cancel_left - save_right, 12.0, "Save and Cancel apart");
        assert_eq!(
            h.texts(&sec("kitchen", ".field-actions button")),
            ["Save", "Cancel"]
        );
        // Sections are 24px apart.
        for pair in SECTION_KEYS.windows(2) {
            let [_, _, _, above] = h.rect(&Harness::section(pair[0]));
            let [_, _, below, _] = h.rect(&Harness::section(pair[1]));
            near(below - above, 24.0, "sections apart");
        }
        // Text field: surface fill, 1px line border, body type in ink.
        assert_eq!(
            style(
                "#profile-kitchen",
                "s.backgroundColor, s.borderTopColor, s.borderTopWidth, s.color, s.fontSize, s.borderRadius"
            ),
            serde_json::json!([
                t.rgb("surface"),
                t.rgb("line"),
                "1px",
                t.rgb("ink"),
                "16px",
                "0px"
            ]),
            "{}",
            t.scheme
        );
        // Button, enabled: a 1.5px ink ring on surface.
        assert_eq!(
            style(
                &Harness::save_selector("kitchen"),
                "s.backgroundColor, s.borderTopColor, s.borderTopStyle, s.color, s.fontWeight, s.fontSize"
            ),
            serde_json::json!([
                t.rgb("surface"),
                t.rgb("ink"),
                "solid",
                t.rgb("ink"),
                "600",
                "14px"
            ]),
            "{}",
            t.scheme
        );
        // Button, disabled: line-strong ring, ink-faint text, no fill.
        assert_eq!(
            style(
                &Harness::save_selector("planning"),
                "s.backgroundColor, s.borderTopColor, s.color"
            ),
            serde_json::json!(["rgba(0, 0, 0, 0)", t.rgb("line-strong"), t.rgb("ink-faint")]),
            "{}",
            t.scheme
        );
        // Focus: the border turns ink and the standard outline shows.
        h.tab_to("#profile-kitchen");
        assert_eq!(
            style(
                "#profile-kitchen",
                "s.borderTopColor, s.outlineStyle, s.outlineWidth, s.outlineColor"
            ),
            serde_json::json!([t.rgb("ink"), "solid", "2px", t.rgb("ink")]),
            "{}",
            t.scheme
        );
    }
    h.use_theme(&LIGHT);
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn profile_page_has_no_emoji() {
    let h = profile_page(1280);
    h.open("household");
    h.fill("household", "x");
    h.click(&Harness::save_selector("household"));
    h.wait_status("household", "Saved.");
    h.open("food");
    assert_eq!(
        h.js("/\\p{Extended_Pictographic}/u.test(document.body.innerText)"),
        false
    );
}

/// Full-page PNGs of the read-first profile page and a day page, at 375 and 1280 px, light and dark.
#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_018() {
    let both = |h: &Harness, name: &str, width: u32| {
        for t in [LIGHT, DARK] {
            h.use_theme(&t);
            h.capture_file(&format!("{name}-{width}-{}.png", t.scheme));
        }
        h.use_theme(&LIGHT);
    };
    for width in [375, 1280] {
        // Read mode, all five empty.
        let h = profile_page(width);
        assert_eq!(h.count(".empty"), 0);
        assert_eq!(h.count("textarea"), 0);
        both(&h, "profile-read-empty", width);

        // Read mode with multi-line text in three sections, two empty, one showing "Saved.".
        h.profile_write(
            "household",
            "Two adults and a toddler.\nPortions: 2.5 adults.\nNo peanuts at home.",
        );
        h.profile_write(
            "food",
            "We like soups and stews.\n\nThe toddler avoids anything spicy.",
        );
        h.profile_write(
            "planning",
            "Shop on Saturday.\nBatch cook on Sunday evening.\nLeftovers for lunch.",
        );
        h.goto_page("/profile");
        h.open("food");
        h.fill(
            "food",
            "We like soups, stews and bread.\n\nThe toddler avoids anything spicy.",
        );
        h.click(&Harness::save_selector("food"));
        h.wait_status("food", "Saved.");
        h.wait_closed_on_edit("food");
        both(&h, "profile-read", width);

        // Edit mode: two sections open, one with a changed draft.
        h.open("household");
        h.open("planning");
        h.fill(
            "planning",
            "Shop on Saturday.\nBatch cook on Sunday evening.\nLeftovers for lunch and dinner.",
        );
        both(&h, "profile-edit", width);

        // A conflict with multi-line saved text, and another section showing the error.
        h.goto_page("/profile");
        let theirs = "We like soups and stews.\nThe toddler now eats mild curry.";
        h.open("food");
        h.fill("food", "We like soups, stews and bread.\nNo mushrooms.");
        h.profile_write("food", theirs);
        h.click(&Harness::save_selector("food"));
        h.wait_notice("food", theirs);
        h.exec(
            "CREATE TRIGGER no_profile_updates BEFORE UPDATE ON profile_sections \
             BEGIN SELECT RAISE(ABORT, 'x'); END;",
        );
        h.open("kitchen");
        h.fill("kitchen", "Oven, hob, slow cooker.\nSmall freezer.");
        h.click(&Harness::save_selector("kitchen"));
        h.wait_status("kitchen", "Couldn't save. Try again.");
        both(&h, "profile-conflict", width);

        // A day page with the footer link.
        let h = Opts::new()
            .width(width)
            .clock(clock_at(2026, 10, 28, 11, 0, 0))
            .path("/2026-10-28")
            .launch();
        assert_eq!(h.count("a.footer-link"), 1);
        both(&h, "day-footer", width);
    }
}

// ---- Feature 19: back link on the household page ------------------------------------------

/// The household page with a 2026-11-17 clock (today is Tuesday 2026-11-17).
fn back_page(path: &'static str, width: u32) -> Harness {
    Opts::new()
        .db(Db::Empty)
        .mcp()
        .width(width)
        .clock(clock_at(2026, 11, 17, 12, 0, 0))
        .path(path)
        .launch()
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn household_back_link_names_where_it_goes() {
    for (path, label, href) in [
        (
            "/profile?from=/2026-11-11",
            "Back to Wednesday, November 11",
            "/2026-11-11",
        ),
        ("/profile?from=/2026-11-17", "Back to today", "/2026-11-17"),
        ("/profile", "Back to today", "/2026-11-17"),
        (
            "/profile?from=//example.com",
            "Back to today",
            "/2026-11-17",
        ),
        ("/profile?from=/2026", "Back to 2026", "/2026"),
    ] {
        let h = back_page(path, 1280);
        assert_eq!(h.status(), 200, "{path}");
        assert_eq!(h.count("a.back-link"), 1, "{path}");
        assert_eq!(h.text("a.back-link"), label, "{path}");
        assert_eq!(
            h.js("document.querySelector('a.back-link').getAttribute('href')"),
            href,
            "{path}"
        );
        assert_eq!(
            h.js("(() => { const l = document.querySelector('a.back-link'); \
                  return l === document.querySelector('.header').firstElementChild \
                  && l.nextElementSibling === document.querySelector('h1'); })()"),
            true,
            "{path}"
        );
        assert_eq!(h.count(".header-meta"), 0, "{path}");
        assert_eq!(h.count("a.all-weeks"), 0, "{path}");
        assert_eq!(
            h.js("[...document.querySelectorAll('*')].filter(e => e.textContent.trim() === 'This week').length"),
            0,
            "{path}"
        );
        let text = h.text("a.back-link");
        assert!(!text.contains(['←', '‹', '→']), "{path}: {text}");
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn household_back_link_follows_the_design_system() {
    let h = back_page("/profile?from=/2026-11-11", 1280);
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        assert_eq!(
            h.js(
                "(() => { const s = getComputedStyle(document.querySelector('a.back-link')); \
                  return [s.fontSize, s.color, s.textDecorationLine, s.textDecorationColor]; })()"
            ),
            serde_json::json!([
                "14px",
                t.rgb("ink-muted"),
                "underline",
                t.rgb("line-strong")
            ]),
            "{}",
            t.scheme
        );
        let [ll, lr, _, lb] = h.rect("a.back-link");
        let [tl, _, tt, tb] = h.rect(".title");
        let [hl, hr, _, _] = h.rect(".header");
        let [_, _, ft, _] = h.rect(".fields");
        assert!(
            (tt - lb - 8.0).abs() <= 0.5,
            "{}: gap {}",
            t.scheme,
            tt - lb
        );
        assert!((ll - tl).abs() <= 0.5, "{}: left", t.scheme);
        assert!(lr - ll < hr - hl, "{}: sized to its text", t.scheme);
        assert!((ft - tb - 24.0).abs() <= 0.5, "{}: {}", t.scheme, ft - tb);
    }
    // Hover turns it ink (after the resting checks, as the pointer stays on the link).
    for t in [LIGHT, DARK] {
        h.use_theme(&t);
        h.tab
            .find_element("a.back-link")
            .unwrap()
            .move_mouse_over()
            .unwrap();
        assert_eq!(
            h.styles("a.back-link", &["color"]),
            [t.rgb("ink")],
            "{} hover",
            t.scheme
        );
    }
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn household_back_link_fits_at_320px() {
    let h = back_page("/profile?from=/2026-09-30", 320);
    assert_eq!(h.text("a.back-link"), "Back to Wednesday, September 30");
    let [_, _, top, bottom] = h.rect("a.back-link");
    assert_eq!(bottom - top, 20.0);
    assert_eq!(
        h.js("document.documentElement.scrollWidth <= document.documentElement.clientWidth"),
        true
    );
}

#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn footer_link_and_back_link_round_trip() {
    for (start, from) in [("/2026-10-28", "/2026-10-28"), ("/2026", "/2026")] {
        let h = Opts::new().mcp().path(start).launch();
        h.click("a.footer-link");
        h.wait_page("/profile");
        assert_eq!(
            h.js("location.pathname + location.search"),
            format!("/profile?from={from}")
        );
        h.click("a.back-link");
        h.wait_page(from);
        if start == "/2026" {
            assert_eq!(h.text(".title"), "Meal Plan: 2026");
        } else {
            assert_eq!(h.text(".day-heading"), "Wednesday, October 28");
        }
    }
}

/// Full-page PNGs of the household page with its back link, at 375 and 1280 px, light and dark.
#[test]
#[ignore = "requires Google Chrome; run: cargo test --test browser -- --ignored"]
fn visual_capture_019() {
    for width in [375, 1280] {
        let h = back_page("/profile?from=/2026-11-11", width);
        for t in [LIGHT, DARK] {
            h.use_theme(&t);
            h.capture_file(&format!("profile-back-{width}-{}.png", t.scheme));
        }
        h.use_theme(&LIGHT);
    }
}
