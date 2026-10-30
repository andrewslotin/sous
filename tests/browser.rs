//! Browser tests against the compiled, embedded web assets (headless Google Chrome).
//!
//! Run with: `cargo test --test browser -- --ignored`

use headless_chrome::protocol::cdp::Emulation::SetDeviceMetricsOverride;
use headless_chrome::{Browser, LaunchOptions, Tab};
use sous::db;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, SqliteConnection};
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

struct Harness {
    _rt: tokio::runtime::Runtime,
    _dir: TempDir,
    _browser: Browser,
    tab: Arc<Tab>,
    base: String,
}

impl Harness {
    /// Fresh DB (migrated, fixture + `extra_sql`), in-process server, Chrome at `width` px.
    fn start(extra_sql: &str, width: u32) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let base = rt.block_on(async {
            db::migrate(&path).await.unwrap();
            let mut c = SqliteConnection::connect_with(
                &SqliteConnectOptions::new()
                    .filename(&path)
                    .journal_mode(SqliteJournalMode::Wal)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
            sqlx::raw_sql(FIXTURE).execute(&mut c).await.unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(extra_sql.to_string()))
                .execute(&mut c)
                .await
                .unwrap();
            let pool = db::open_read_pool(&path).await.unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, sous::app(pool)).await.unwrap() });
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
            _rt: rt,
            _dir: dir,
            _browser: browser,
            tab,
            base,
        };
        h.goto("/2026/44");
        h
    }

    fn goto(&self, path: &str) {
        self.tab
            .navigate_to(&format!("{}{path}", self.base))
            .unwrap()
            .wait_until_navigated()
            .unwrap();
        self.wait_for("document.querySelector('.day-button') !== null");
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
