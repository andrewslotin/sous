//! MCP protocol tests (feature 6): the `/mcp` endpoint through the router, plus one real client over TCP.
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::TimeZone;
use http_body_util::BodyExt;
use serde_json::{Map, Value, json};
use sous::calendar::Clock;
use sous::{McpConfig, db};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, Row, SqliteConnection, SqlitePool};
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;

const W44: &str = include_str!("../fixtures/2026-w44.sql");
const W44_WEEKLY: &str = include_str!("../fixtures/2026-w44-weekly.sql");
const HISTORY: &str = include_str!("../fixtures/2026-history.sql");
const STRATEGY_0003: &str = include_str!("fixtures/0003-strategy.sql");
const TOOLS_DOC: &str = include_str!("../docs/mcp-tools.json");
const SAVE_EXAMPLE: &str = include_str!("../docs/examples/save-week.json");

const CASSEROLE_DINNER: &str = "000000c3-0000-4000-8000-000000000001";
const CASSEROLE_LEFTOVERS: &str = "000000c3-0000-4000-8000-000000000002";
const TOOL_NAMES: [&str; 8] = [
    "get_context",
    "list_weeks",
    "get_week",
    "search_meals",
    "get_meal",
    "get_shopping_lists",
    "save_week",
    "update_profile_section",
];

// ---- Harness ----

struct Env {
    _dir: TempDir,
    path: PathBuf,
    read: SqlitePool,
    write: SqlitePool,
}

impl Env {
    async fn empty() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        db::migrate(&path).await.unwrap();
        let read = db::open_read_pool(&path).await.unwrap();
        let write = db::open_write_pool(&path).await.unwrap();
        Env {
            _dir: dir,
            path,
            read,
            write,
        }
    }

    /// W44 fixtures plus the history fixture.
    async fn with_fixtures() -> Self {
        let env = Self::empty().await;
        let mut c = env.writer().await;
        for sql in [W44, W44_WEEKLY, HISTORY] {
            sqlx::raw_sql(sql).execute(&mut c).await.unwrap();
        }
        env
    }

    /// A database migrated through 0003 only, holding the strategy rows and every other fixture,
    /// then upgraded by `db::migrate`.
    async fn upgraded_from_version_3() -> Self {
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
        {
            let mut c = SqliteConnection::connect_with(
                &SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
            old.run(&mut c).await.unwrap();
            for sql in [W44, W44_WEEKLY, STRATEGY_0003, HISTORY] {
                sqlx::raw_sql(sql).execute(&mut c).await.unwrap();
            }
        }
        db::migrate(&path).await.unwrap();
        let read = db::open_read_pool(&path).await.unwrap();
        let write = db::open_write_pool(&path).await.unwrap();
        Env {
            _dir: dir,
            path,
            read,
            write,
        }
    }

    /// A separate connection, like an external writer.
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

    fn app(&self, clock: Clock) -> Router {
        self.app_with(clock, vec![], vec![])
    }

    fn app_with(&self, clock: Clock, hosts: Vec<String>, origins: Vec<String>) -> Router {
        sous::app_with_mcp(
            self.read.clone(),
            clock,
            McpConfig {
                write_pool: self.write.clone(),
                allowed_hosts: hosts,
                allowed_origins: origins,
            },
        )
    }

    /// Every row of every table, as text, for before/after comparisons.
    async fn dump(&self) -> Vec<String> {
        let mut c = self.writer().await;
        let tables: Vec<String> = sqlx::query(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .fetch_all(&mut c)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>(0))
        .collect();
        let mut out = Vec::new();
        for table in tables {
            let columns: Vec<String> =
                sqlx::query("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                    .bind(&table)
                    .fetch_all(&mut c)
                    .await
                    .unwrap()
                    .iter()
                    .map(|row| row.get::<String, _>(0))
                    .collect();
            let expr = columns
                .iter()
                .map(|c| format!("quote(\"{c}\")"))
                .collect::<Vec<_>>()
                .join(" || ',' || ");
            let sql = format!("SELECT {expr} FROM \"{table}\" ORDER BY 1");
            let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut c)
                .await
                .unwrap();
            out.push(format!("== {table}: {} rows", rows.len()));
            out.extend(rows.iter().map(|row| row.get::<String, _>(0)));
        }
        out
    }
}

fn clock_at(tz: &str, y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> Clock {
    Clock::fixed(
        tz.parse().unwrap(),
        chrono::Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap(),
    )
}

/// The journey clock: Monday 2026-10-26 in Berlin, current week 2026-W44.
fn journey_clock() -> Clock {
    clock_at("Europe/Berlin", 2026, 10, 26, 9, 0, 0)
}

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "sous-test", "version": "1.0.0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.text))
    }
}

/// POST to `/mcp` with the base headers; `extra` can override them.
async fn post_raw(app: &Router, body: String, extra: &[(&str, &str)]) -> Reply {
    let mut builder = Request::post("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    send(app, builder.body(Body::from(body)).unwrap()).await
}

async fn send(app: &Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    assert!(
        headers.get("access-control-allow-origin").is_none(),
        "no CORS header may be sent"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// One 2026-07-28 stateless request; returns the HTTP reply.
async fn modern_raw(app: &Router, id: u64, method: &str, mut params: Value) -> Reply {
    let name = params["name"].as_str().map(str::to_owned);
    params["_meta"] = meta();
    let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let mut extra = vec![
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", method),
    ];
    if let Some(name) = name.as_deref().filter(|_| method == "tools/call") {
        extra.push(("mcp-name", name));
    }
    post_raw(app, body.to_string(), &extra).await
}

/// A 2026-07-28 request that must succeed; returns the JSON-RPC `result`.
async fn modern(app: &Router, method: &str, params: Value) -> Value {
    let reply = modern_raw(app, 1, method, params).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let body = reply.json();
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}

/// A tool call; returns the whole `CallToolResult`.
async fn call(app: &Router, tool: &str, args: Value) -> Value {
    modern(app, "tools/call", json!({"name": tool, "arguments": args})).await
}

/// A tool call that must succeed; returns `structuredContent` and checks the text block matches.
async fn ok(app: &Router, tool: &str, args: Value) -> Value {
    let result = call(app, tool, args).await;
    assert_eq!(result["isError"], false, "{tool}: {result}");
    let structured = result["structuredContent"].clone();
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    let text: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, structured, "text block must hold the same JSON");
    structured
}

/// A tool call that must fail as a tool error; returns the error code.
async fn err(app: &Router, tool: &str, args: Value) -> String {
    let result = call(app, tool, args).await;
    assert_eq!(result["isError"], true, "{tool}: {result}");
    let error = &result["structuredContent"]["error"];
    let code = error["code"].as_str().unwrap().to_owned();
    let message = error["message"].as_str().unwrap();
    assert_eq!(
        result["content"][0]["text"],
        format!("{code}: {message}").as_str()
    );
    code
}

const ALREADY_SAVED: &str =
    "A plan is already saved for this week. Fetch it with get_week and resubmit with its revision.";
const NOT_SAVED: &str = "No plan is saved for this week. Omit expectedRevision to create it.";
const STALE: &str = "The week changed since it was read. Fetch it with get_week and resubmit.";

/// Calls `save_week` and asserts a `revision_conflict` error with exactly this message.
async fn assert_conflict(app: &Router, args: Value, message: &str) {
    let result = call(app, "save_week", args).await;
    assert_eq!(result["isError"], true, "{result}");
    let error = &result["structuredContent"]["error"];
    assert_eq!(error["code"], "revision_conflict", "{result}");
    assert_eq!(error["message"], message, "{result}");
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let reply = send(
        app,
        Request::get(uri)
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let value = serde_json::from_str(&reply.text).unwrap_or(Value::Null);
    (reply.status, value)
}

/// The JSON payload embedded in an HTML page.
fn page_json(html: &str) -> Value {
    let marker = "<script id=\"sous-page\" type=\"application/json\">";
    let start = html.find(marker).expect("sous-page script") + marker.len();
    let end = start + html[start..].find("</script>").unwrap();
    serde_json::from_str(&html[start..end]).unwrap()
}

fn id(n: u32) -> String {
    format!("e5000000-0000-4000-8000-{n:012x}")
}

fn step(n: u32, text: &str) -> Value {
    json!({"id": id(n), "text": text})
}

fn meal(n: u32, kind: &str, name: &str, steps: Vec<Value>) -> Value {
    json!({
        "id": id(n), "type": kind, "name": name, "description": format!("{name}, per person"),
        "kcal": 420, "proteinG": 31.5, "fatG": 12, "carbsG": 40, "fiberG": 9, "forTwo": true,
        "assemblyTime": "10 min", "cookTime": null, "tip": "Season at the end.", "steps": steps,
    })
}

/// A complete 2026-W45 document (Monday 2026-11-02) using most field kinds.
fn week45() -> Value {
    json!({
        "week": {"id": id(1), "subtitle": "Soup season", "prepIntro": "Cook the lentils on Sunday."},
        "days": [
            {
                "id": id(2), "weekday": 1,
                "meals": [
                    meal(10, "Breakfast", "Overnight Oats", vec![step(11, "Stir oats and milk.")]),
                    meal(12, "Dinner", "Red Lentil Soup", vec![
                        step(13, "Sweat the onion."),
                        step(14, "Add lentils and water."),
                        step(15, "Simmer 20 minutes."),
                    ]),
                ],
                "shoppingListIds": [id(30), id(31)],
                "reminder": {"title": "Soak the oats", "body": null, "icon": "🌙"},
            },
            {
                "id": id(3), "weekday": 2, "meals": [],
                "shoppingListIds": [id(31)], "reminder": null,
            },
        ],
        "shoppingLists": [
            {"id": id(30), "label": "🛒 Monday Shop", "note": "Check the pantry first.", "categories": [
                {"id": id(40), "name": "Produce", "items": [
                    {"id": id(50), "text": "2 onions"},
                    {"id": id(51), "text": "1 lemon"},
                ]},
            ]},
            {"id": id(31), "label": "🧴 Household", "note": null, "categories": []},
        ],
        "prepSessions": [
            {"id": id(60), "title": "Sunday prep", "covers": "Mon–Tue", "weekday": 1, "tasks": [
                {"id": id(61), "name": "Cook lentils", "time": "20 min", "instructions": "Boil and drain."},
            ]},
        ],
    })
}

fn save_args(iso_week: u32, expected: Value, document: Value) -> Value {
    json!({"isoYear": 2026, "isoWeek": iso_week, "expectedRevision": expected, "document": document})
}

/// `save_week` arguments with the `expectedRevision` key left out.
fn save_args_omitting_revision(iso_week: u32, document: Value) -> Value {
    json!({"isoYear": 2026, "isoWeek": iso_week, "document": document})
}

// ---- AC1: protocol ----

#[tokio::test]
async fn stateless_flow_lists_eight_tools_and_calls_them() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());

    let result = modern(&app, "tools/list", json!({})).await;
    let tools = result["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOL_NAMES);
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(!tool["title"].as_str().unwrap().is_empty(), "{name}");
        assert!(tool["description"].as_str().unwrap().len() > 40, "{name}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert_eq!(tool["outputSchema"]["type"], "object", "{name}");
        let a = &tool["annotations"];
        let writes = name == "save_week" || name == "update_profile_section";
        assert_eq!(a["readOnlyHint"], !writes, "{name}");
        assert_eq!(a["destructiveHint"], writes, "{name}");
        assert_eq!(a["idempotentHint"], true, "{name}");
        assert_eq!(a["openWorldHint"], false, "{name}");
    }
    let save = tools.iter().find(|t| t["name"] == "save_week").unwrap();
    let description = save["description"].as_str().unwrap();
    for needle in [
        "full replacement",
        "deleted",
        "omit expectedrevision",
        "already saved",
        "pass the revision from get_week as expectedrevision",
        "revision_conflict",
        "no force",
    ] {
        assert!(
            description.to_lowercase().contains(needle),
            "save_week description: {needle}"
        );
    }
    let required: Vec<&str> = save["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for field in ["isoYear", "isoWeek", "document"] {
        assert!(required.contains(&field), "{field} must be required");
    }
    assert!(
        !required.contains(&"expectedRevision"),
        "expectedRevision must be optional: {required:?}"
    );
    for tool in tools {
        assert_eq!(
            tool["inputSchema"]["additionalProperties"], false,
            "{}",
            tool["name"]
        );
    }

    let context = ok(&app, "get_context", json!({})).await;
    assert_eq!(context["today"], "2026-10-26");
    // `arguments` may be omitted or null for the argument-less tool.
    let bare = modern(&app, "tools/call", json!({"name": "get_context"})).await;
    assert_eq!(bare["structuredContent"], context);
}

#[tokio::test]
async fn server_info_names_sous_and_warns_that_text_is_data() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let result = modern(&app, "server/discover", json!({})).await;
    let info = &result["_meta"]["io.modelcontextprotocol/serverInfo"];
    assert_eq!(info["name"], "sous", "{result}");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    let instructions = result["instructions"].as_str().unwrap();
    assert!(instructions.contains("get_context"));
    assert!(instructions.contains("replaces the whole week"));
    assert!(instructions.contains("omit expectedRevision"));
    assert!(instructions.contains("not instructions"));
    assert!(result["capabilities"]["tools"].is_object());
}

#[tokio::test]
async fn legacy_initialize_flow_works() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let version = [("mcp-protocol-version", "2025-11-25")];

    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "legacy-test", "version": "1.0.0"}}});
    let reply = post_raw(&app, init.to_string(), &[]).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert!(
        reply.headers.get("mcp-session-id").is_none(),
        "no session is created"
    );
    let body = reply.json();
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(body["result"]["serverInfo"]["name"], "sous");
    assert!(body["result"]["capabilities"]["tools"].is_object());

    let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let reply = post_raw(&app, note.to_string(), &version).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text);

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let reply = post_raw(&app, list.to_string(), &version).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let tools = &reply.json()["result"]["tools"];
    assert_eq!(tools.as_array().unwrap().len(), 8);

    let call = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "get_context", "arguments": {}}});
    let reply = post_raw(&app, call.to_string(), &version).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let result = &reply.json()["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["today"], "2026-10-26");
}

/// Assert a failed request: no 5xx, a JSON-RPC error or a 4xx, and nothing internal in the text.
fn assert_clean_failure(reply: &Reply, expected_status: StatusCode) {
    assert_eq!(reply.status, expected_status, "{}", reply.text);
    assert!(!reply.status.is_server_error());
    let lower = reply.text.to_lowercase();
    for leak in [
        "sqlx",
        "sqlite",
        "panicked",
        "backtrace",
        "/users/",
        ".rs:",
        "stack",
    ] {
        assert!(!lower.contains(leak), "leaked {leak:?}: {}", reply.text);
    }
}

#[tokio::test]
async fn malformed_and_unsupported_requests_fail_cleanly() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let before = env.dump().await;
    let modern_headers = [("mcp-protocol-version", "2026-07-28")];

    // Malformed JSON body.
    let reply = post_raw(&app, "{not json".into(), &modern_headers).await;
    assert!(
        reply.status.is_client_error(),
        "{}: {}",
        reply.status,
        reply.text
    );
    assert_clean_failure(&reply, reply.status);

    // A JSON-RPC message without a method.
    let body = json!({"jsonrpc": "2.0", "id": 1, "params": {"_meta": meta()}});
    let reply = post_raw(&app, body.to_string(), &modern_headers).await;
    assert!(
        reply.status.is_client_error(),
        "{}: {}",
        reply.status,
        reply.text
    );
    assert_clean_failure(&reply, reply.status);

    // An unknown method is a JSON-RPC method-not-found error.
    let reply = modern_raw(&app, 1, "does/not/exist", json!({})).await;
    assert_clean_failure(&reply, StatusCode::NOT_FOUND);
    assert_eq!(reply.json()["error"]["code"], -32601);

    // An unknown tool is a protocol error (invalid params), not a tool error.
    let reply = modern_raw(
        &app,
        1,
        "tools/call",
        json!({"name": "drop_tables", "arguments": {}}),
    )
    .await;
    assert_clean_failure(&reply, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["error"]["code"], -32602, "{body}");
    assert!(body.get("result").is_none());

    // An unsupported protocol revision header.
    let list =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta()}});
    let reply = post_raw(
        &app,
        list.to_string(),
        &[("mcp-protocol-version", "1999-01-01")],
    )
    .await;
    assert_clean_failure(&reply, StatusCode::BAD_REQUEST);

    // An unsupported revision in the request metadata.
    let mut bad_meta = meta();
    bad_meta["io.modelcontextprotocol/protocolVersion"] = json!("1999-01-01");
    let list =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": bad_meta}});
    let reply = post_raw(&app, list.to_string(), &modern_headers).await;
    assert!(
        reply.status.is_client_error(),
        "{}: {}",
        reply.status,
        reply.text
    );
    assert_clean_failure(&reply, reply.status);

    // Wrong content type and missing Accept.
    let reply = post_raw(&app, "{}".into(), &[("content-type", "text/plain")]).await;
    assert_clean_failure(&reply, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let reply = post_raw(&app, "{}".into(), &[("accept", "application/json")]).await;
    assert!(reply.status.is_client_error(), "{}", reply.status);
    assert_clean_failure(&reply, reply.status);

    // GET and DELETE have nothing to stream or terminate in stateless mode.
    for request in [
        Request::get("/mcp")
            .header("host", "localhost")
            .header("accept", "text/event-stream"),
        Request::delete("/mcp").header("host", "localhost"),
    ] {
        let reply = send(&app, request.body(Body::empty()).unwrap()).await;
        assert!(
            reply.status.is_client_error(),
            "{}: {}",
            reply.status,
            reply.text
        );
        assert_clean_failure(&reply, reply.status);
    }

    assert_eq!(
        env.dump().await,
        before,
        "failed requests must not change data"
    );
}

// ---- AC2: journeys ----

#[tokio::test]
async fn journey_plan_recall_and_shop() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());

    // 1. Where are we?
    let context = ok(&app, "get_context", json!({})).await;
    assert_eq!(context["today"], "2026-10-26");
    assert_eq!(context["timezone"], "Europe/Berlin");
    assert_eq!(context["currentWeek"]["isoYear"], 2026);
    assert_eq!(context["currentWeek"]["isoWeek"], 44);
    assert_eq!(context["currentWeek"]["startDate"], "2026-10-26");
    assert_eq!(context["currentWeek"]["endDate"], "2026-11-01");
    assert_eq!(context["currentWeek"]["path"], "/2026/44");

    // 2. Plan next week.
    assert_eq!(
        err(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 45})).await,
        "week_not_found"
    );
    let saved = ok(&app, "save_week", save_args(45, Value::Null, week45())).await;
    let revision = saved["revision"].as_str().unwrap().to_owned();
    let fetched = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 45})).await;
    assert_eq!(fetched, saved);
    assert_eq!(fetched["document"]["week"]["startDate"], "2026-11-02");
    assert_eq!(
        fetched["document"]["days"][0]["meals"][1]["name"],
        "Red Lentil Soup"
    );
    assert!(!revision.is_empty());

    // 3. The casserole from three weeks ago.
    let found = ok(
        &app,
        "search_meals",
        json!({"query": "casserole", "fromDate": "2026-10-05", "toDate": "2026-10-11"}),
    )
    .await;
    let ids: Vec<&str> = found["meals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [CASSEROLE_LEFTOVERS, CASSEROLE_DINNER]);
    assert_eq!(found["nextCursor"], Value::Null);
    let dinner = ok(&app, "get_meal", json!({"mealId": CASSEROLE_DINNER})).await;
    let steps: Vec<&str> = dinner["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        steps,
        [
            "Heat the oven to 190°C.",
            "Brown the chicken.",
            "Layer chicken, broccoli and sauce in the dish.",
            "Bake 35 min until golden.",
        ]
    );
    assert_eq!(dinner["date"], "2026-10-07");
    assert_eq!(dinner["weekPath"], "/2026/41");

    // 4. Today's shopping.
    let today = ok(&app, "get_shopping_lists", json!({})).await;
    assert_eq!(today["date"], "2026-10-26");
    assert_eq!(today["timezone"], "Europe/Berlin");
    assert_eq!(today["planExists"], true);
    let labels: Vec<&str> = today["lists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["🧴 Household", "🛒 Monday Shop → covers Mon–Wed"]);
}

// ---- AC3: timezones ----

#[tokio::test]
async fn defaults_follow_the_service_timezone() {
    let env = Env::with_fixtures().await;
    let mut c = env.writer().await;
    for (n, (year, week)) in [(2026, 43), (2026, 45), (2026, 53), (2027, 1)]
        .into_iter()
        .enumerate()
    {
        let sql = format!(
            "INSERT INTO weeks (id, iso_year, iso_week) VALUES ('{}', {year}, {week})",
            id(900 + n as u32)
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut c)
            .await
            .unwrap();
    }

    // (clock, today, isoYear, isoWeek)
    let cases = [
        (
            clock_at("Europe/Berlin", 2026, 11, 1, 22, 59, 59),
            "2026-11-01",
            2026,
            44,
        ),
        (
            clock_at("Europe/Berlin", 2026, 11, 1, 23, 0, 0),
            "2026-11-02",
            2026,
            45,
        ),
        (
            clock_at("Europe/Berlin", 2026, 10, 25, 22, 59, 59),
            "2026-10-25",
            2026,
            43,
        ),
        (
            clock_at("Europe/Berlin", 2026, 10, 25, 23, 0, 0),
            "2026-10-26",
            2026,
            44,
        ),
        (
            clock_at("Europe/Berlin", 2026, 12, 31, 12, 0, 0),
            "2026-12-31",
            2026,
            53,
        ),
        (
            clock_at("Europe/Berlin", 2027, 1, 3, 22, 59, 59),
            "2027-01-03",
            2026,
            53,
        ),
        (
            clock_at("Europe/Berlin", 2027, 1, 3, 23, 0, 0),
            "2027-01-04",
            2027,
            1,
        ),
        (
            clock_at("America/New_York", 2026, 11, 2, 3, 0, 0),
            "2026-11-01",
            2026,
            44,
        ),
        (
            clock_at("America/New_York", 2026, 11, 2, 5, 0, 0),
            "2026-11-02",
            2026,
            45,
        ),
    ];
    for (clock, today, year, week) in cases {
        let timezone = clock.timezone().name().to_owned();
        let app = env.app(clock);
        let label = format!("{timezone} {today}");

        let context = ok(&app, "get_context", json!({})).await;
        assert_eq!(context["today"], today, "{label}");
        assert_eq!(context["timezone"], timezone.as_str(), "{label}");
        assert_eq!(context["currentWeek"]["isoYear"], year, "{label}");
        assert_eq!(context["currentWeek"]["isoWeek"], week, "{label}");

        let default_week = ok(&app, "get_week", json!({})).await;
        assert_eq!(default_week["document"]["week"]["isoYear"], year, "{label}");
        assert_eq!(default_week["document"]["week"]["isoWeek"], week, "{label}");

        let shopping = ok(&app, "get_shopping_lists", json!({})).await;
        assert_eq!(shopping["date"], today, "{label}");
        assert_eq!(shopping["timezone"], timezone.as_str(), "{label}");
        assert_eq!(shopping["isoYear"], year, "{label}");
        assert_eq!(shopping["isoWeek"], week, "{label}");
    }
}

#[tokio::test]
async fn explicit_dates_ignore_the_clock_timezone() {
    let env = Env::with_fixtures().await;
    let berlin = env.app(clock_at("Europe/Berlin", 2026, 10, 26, 9, 0, 0));
    let kiritimati = env.app(clock_at("Pacific/Kiritimati", 2031, 3, 4, 23, 30, 0));
    let mut a = ok(&berlin, "get_shopping_lists", json!({"date": "2026-10-26"})).await;
    let mut b = ok(
        &kiritimati,
        "get_shopping_lists",
        json!({"date": "2026-10-26"}),
    )
    .await;
    assert_eq!(a["timezone"], "Europe/Berlin");
    assert_eq!(b["timezone"], "Pacific/Kiritimati");
    a["timezone"] = Value::Null;
    b["timezone"] = Value::Null;
    assert_eq!(a, b);
    assert_eq!(a["lists"].as_array().unwrap().len(), 2);

    let week_a = ok(&berlin, "get_week", json!({"isoYear": 2026, "isoWeek": 44})).await;
    let week_b = ok(
        &kiritimati,
        "get_week",
        json!({"isoYear": 2026, "isoWeek": 44}),
    )
    .await;
    assert_eq!(week_a, week_b);
}

// ---- AC4 / AC5 through MCP ----

#[tokio::test]
async fn search_paginates_through_the_cursor() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = 0;
    loop {
        let mut args = json!({"query": "CASSEROLE", "limit": 1});
        if !cursor.is_null() {
            args["cursor"] = cursor.clone();
        }
        let page = ok(&app, "search_meals", args).await;
        pages += 1;
        for meal in page["meals"].as_array().unwrap() {
            seen.push(meal["id"].as_str().unwrap().to_owned());
        }
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
        assert!(cursor.is_string());
        assert!(pages < 10);
    }
    assert_eq!(
        seen,
        [
            CASSEROLE_LEFTOVERS,
            CASSEROLE_DINNER,
            "000000c3-0000-4000-8000-000000000004"
        ]
    );
    assert_eq!(pages, 3);

    // The same cursor against other filters, and garbage, are invalid input.
    let first = ok(
        &app,
        "search_meals",
        json!({"query": "casserole", "limit": 1}),
    )
    .await;
    let cursor = first["nextCursor"].clone();
    assert_eq!(
        err(
            &app,
            "search_meals",
            json!({"query": "rye", "limit": 1, "cursor": cursor})
        )
        .await,
        "invalid_input"
    );
    assert_eq!(
        err(
            &app,
            "search_meals",
            json!({"query": "casserole", "cursor": "not-a-cursor"})
        )
        .await,
        "invalid_input"
    );
    assert_eq!(
        err(&app, "list_weeks", json!({"cursor": "zz"})).await,
        "invalid_input"
    );
}

#[tokio::test]
async fn shopping_cases_through_mcp() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());

    let tuesday = ok(&app, "get_shopping_lists", json!({"date": "2026-10-27"})).await;
    assert_eq!(tuesday["planExists"], true);
    assert_eq!(tuesday["lists"], json!([]));

    let wednesday = ok(&app, "get_shopping_lists", json!({"date": "2026-10-28"})).await;
    let labels: Vec<&str> = wednesday["lists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["🛒 Monday Shop → covers Mon–Wed"]);

    // Thursday has no day record; the Thursday Top-Up list is assigned to no day.
    let thursday = ok(&app, "get_shopping_lists", json!({"date": "2026-10-29"})).await;
    assert_eq!(thursday["planExists"], true);
    assert_eq!(thursday["lists"], json!([]));

    let next_week = ok(&app, "get_shopping_lists", json!({"date": "2026-11-02"})).await;
    assert_eq!(next_week["planExists"], false);
    assert_eq!(next_week["lists"], json!([]));
    assert_eq!(next_week["weekPath"], "/2026/45");

    // Monday: exact categories and items, in stored order.
    let monday = ok(&app, "get_shopping_lists", json!({"date": "2026-10-26"})).await;
    let shop = &monday["lists"][1];
    let categories: Vec<(&str, Vec<&str>)> = shop["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap(),
                c["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["text"].as_str().unwrap())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(categories[0].0, "Produce");
    assert_eq!(categories[0].1[0], "1 cucumber");
    assert_eq!(categories[1].0, "Dairy / Chilled");
    assert_eq!(categories[1].1, ["2L oat milk", "Skyr (400g)"]);
    assert_eq!(categories[2], ("Bakery", vec!["1 loaf dark rye bread"]));
    assert_eq!(
        shop["note"],
        "You have: salmon, oats, walnuts, almonds, chia, miso, spices."
    );
}

#[tokio::test]
async fn argument_errors_are_tool_errors() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let cases = [
        ("get_week", json!({"isoYear": 2026})),
        ("get_week", json!({"isoWeek": 44})),
        ("get_week", json!({"isoYear": 2025, "isoWeek": 53})),
        ("get_week", json!({"isoYear": "2026", "isoWeek": 44})),
        (
            "get_week",
            json!({"isoYear": 2026, "isoWeek": 44, "extra": 1}),
        ),
        ("get_context", json!({"unexpected": true})),
        ("list_weeks", json!({"fromDate": "2026-1-5"})),
        (
            "list_weeks",
            json!({"fromDate": "2026-10-10", "toDate": "2026-10-01"}),
        ),
        ("list_weeks", json!({"limit": 0})),
        ("list_weeks", json!({"limit": 101})),
        ("search_meals", json!({})),
        ("search_meals", json!({"query": ""})),
        (
            "search_meals",
            json!({"query": "x", "toDate": "2026-02-30"}),
        ),
        ("get_meal", json!({})),
        (
            "get_meal",
            json!({"mealId": CASSEROLE_DINNER.to_uppercase()}),
        ),
        ("get_shopping_lists", json!({"date": "tomorrow"})),
        ("get_shopping_lists", json!({"date": "9999-12-31"})),
    ];
    for (tool, args) in cases {
        let result = call(&app, tool, args.clone()).await;
        assert_eq!(result["isError"], true, "{tool} {args}: {result}");
        assert_eq!(
            result["structuredContent"]["error"]["code"], "invalid_input",
            "{tool} {args}"
        );
        assert!(
            !result["structuredContent"]["error"]["message"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        err(
            &app,
            "get_meal",
            json!({"mealId": "000000c3-0000-4000-8000-0000000000ff"})
        )
        .await,
        "meal_not_found"
    );
    let weeks = ok(
        &app,
        "list_weeks",
        json!({"fromDate": "2026-10-01", "toDate": "2026-10-31"}),
    )
    .await;
    let paths: Vec<&str> = weeks["weeks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["/2026/44", "/2026/41"]);
    assert_eq!(weeks["nextCursor"], Value::Null);
}

// ---- AC6 / AC7 / AC8 ----

#[tokio::test]
async fn get_week_equals_the_read_api_and_get_meal_has_every_field() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let (status, api) = get_json(&app, "/api/2026/44").await;
    assert_eq!(status, StatusCode::OK);
    let mcp = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 44})).await;
    assert_eq!(mcp["document"], api);

    let meal = ok(&app, "get_meal", json!({"mealId": CASSEROLE_DINNER})).await;
    let (_, w41) = get_json(&app, "/api/2026/41").await;
    let from_week = &w41["days"][2]["meals"][0];
    assert_eq!(from_week["id"], CASSEROLE_DINNER);
    let mut expected = from_week.clone();
    let fields = expected.as_object_mut().unwrap();
    for (key, value) in [
        ("date", json!("2026-10-07")),
        ("weekday", json!(3)),
        ("isoYear", json!(2026)),
        ("isoWeek", json!(41)),
        ("weekPath", json!("/2026/41")),
    ] {
        fields.insert(key.into(), value);
    }
    assert_eq!(meal, expected);
    assert_eq!(
        meal,
        json!({
            "id": CASSEROLE_DINNER, "type": "Dinner", "name": "Chicken & Broccoli Casserole",
            "description": "Per person: 150g chicken, broccoli, light cheese sauce",
            "kcal": 610.0, "proteinG": 45.0, "fatG": 21.5, "carbsG": 48.0, "fiberG": 7.0,
            "forTwo": true, "assemblyTime": "15 min", "cookTime": "35 min",
            "tip": "Rest 5 min before serving.",
            "steps": meal["steps"],
            "date": "2026-10-07", "weekday": 3, "isoYear": 2026, "isoWeek": 41, "weekPath": "/2026/41",
        })
    );
    assert_eq!(meal["steps"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn saved_week_is_visible_without_rebuilding_the_router() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let (status, _) = get_json(&app, "/api/2026/45").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    ok(&app, "save_week", save_args(45, Value::Null, week45())).await;

    let (status, api) = get_json(&app, "/api/2026/45").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        api["days"][0]["meals"][1]["steps"][2]["text"],
        "Simmer 20 minutes."
    );
    assert!(api.get("strategyCards").is_none());
    assert_eq!(api["schemaVersion"], 3);
    let page = send(
        &app,
        Request::get("/2026/45")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    let payload = page_json(&page.text);
    assert_eq!(payload["kind"], "week");
    assert_eq!(payload["document"], api);
}

#[tokio::test]
async fn invalid_and_conflicting_saves_change_nothing() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let before = env.dump().await;

    let mut unknown_list = week45();
    unknown_list["days"][0]["shoppingListIds"] = json!([id(99)]);
    let result = call(&app, "save_week", save_args(45, Value::Null, unknown_list)).await;
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "invalid_input"
    );
    assert_eq!(env.dump().await, before);

    // A missing nullable key is rejected; an explicit null is required.
    let mut missing_tip = week45();
    missing_tip["days"][0]["meals"][0]
        .as_object_mut()
        .unwrap()
        .remove("tip");
    let result = call(&app, "save_week", save_args(45, Value::Null, missing_tip)).await;
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "invalid_input"
    );
    assert!(
        result["structuredContent"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tip")
    );
    // And unknown fields or enum values.
    let mut brunch = week45();
    brunch["days"][0]["meals"][0]["type"] = json!("Brunch");
    assert_eq!(
        err(&app, "save_week", save_args(45, Value::Null, brunch)).await,
        "invalid_input"
    );
    let mut extra = week45();
    extra["week"]["colour"] = json!("red");
    assert_eq!(
        err(&app, "save_week", save_args(45, Value::Null, extra)).await,
        "invalid_input"
    );
    // An unrepresentable week (2025 has no week 53).
    let impossible =
        json!({"isoYear": 2025, "isoWeek": 53, "expectedRevision": null, "document": week45()});
    assert_eq!(err(&app, "save_week", impossible).await, "invalid_input");
    assert_eq!(env.dump().await, before);

    // Conflicts: a stale revision, a revision for a missing week, and creating an existing week.
    let saved = ok(&app, "save_week", save_args(45, Value::Null, week45())).await;
    let revision = saved["revision"].as_str().unwrap().to_owned();
    let after_save = env.dump().await;
    let mut edited = week45();
    edited["days"][0]["meals"][0]["name"] = json!("Porridge");
    assert_conflict(&app, save_args(45, json!("stale"), edited.clone()), STALE).await;
    assert_conflict(
        &app,
        save_args(45, Value::Null, edited.clone()),
        ALREADY_SAVED,
    )
    .await;
    assert_conflict(
        &app,
        save_args_omitting_revision(45, edited.clone()),
        ALREADY_SAVED,
    )
    .await;
    assert_conflict(
        &app,
        save_args(46, json!(revision), edited.clone()),
        NOT_SAVED,
    )
    .await;
    assert_eq!(
        env.dump().await,
        after_save,
        "conflicts must not change data"
    );

    // The right revision works once; replaying the same request is then a conflict.
    let revised = ok(
        &app,
        "save_week",
        save_args(45, json!(revision), edited.clone()),
    )
    .await;
    assert_ne!(revised["revision"], saved["revision"]);
    assert_eq!(
        revised["document"]["days"][0]["meals"][0]["name"],
        "Porridge"
    );
    assert_conflict(&app, save_args(45, json!(revision), edited), STALE).await;

    // The error text names the remedy.
    let result = call(&app, "save_week", save_args(45, json!("stale"), week45())).await;
    assert!(
        result["structuredContent"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("get_week")
    );
}

// ---- Feature 12: strategy cards are gone ----

#[tokio::test]
async fn save_week_rejects_strategy_cards_and_get_week_has_none() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());

    let saved = ok(&app, "save_week", save_args(45, Value::Null, week45())).await;
    let revision = saved["revision"].clone();
    assert!(saved["document"].get("strategyCards").is_none());
    assert_eq!(saved["document"]["schemaVersion"], 3);
    let fetched = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 44})).await;
    assert!(fetched["document"].get("strategyCards").is_none());
    assert_eq!(fetched["document"]["schemaVersion"], 3);
    let (_, year) = get_json(&app, "/api/2026").await;
    assert_eq!(year["schemaVersion"], 1);

    let before = env.dump().await;
    let card = json!({
        "id": id(70), "title": "Batch cooking", "icon": "🍲", "theme": "green",
        "items": [{"id": id(71), "text": "Double the soup."}],
    });
    for cards in [json!([]), json!([card])] {
        let mut with_cards = week45();
        with_cards["days"][0]["meals"][0]["name"] = json!("Porridge");
        with_cards["strategyCards"] = cards;
        let result = call(
            &app,
            "save_week",
            save_args(45, revision.clone(), with_cards),
        )
        .await;
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(
            result["structuredContent"]["error"]["code"],
            "invalid_input"
        );
        assert!(
            result["structuredContent"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("strategyCards")
        );
        assert_eq!(env.dump().await, before, "an invalid save changes nothing");
        let current = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 45})).await;
        assert_eq!(current, saved, "revision and document are unchanged");
    }

    // The same document without strategyCards saves.
    let mut edited = week45();
    edited["days"][0]["meals"][0]["name"] = json!("Porridge");
    let revised = ok(&app, "save_week", save_args(45, revision, edited)).await;
    assert_eq!(
        revised["document"]["days"][0]["meals"][0]["name"],
        "Porridge"
    );
    assert_ne!(revised["revision"], saved["revision"]);
}

// Revisions a pre-0004 server returned for weeks of the fixtures, as a client read them before the upgrade.
/// Revision a pre-0004 server returned for 2026-W44 with `2026-w44.sql` and `2026-w44-weekly.sql` (strategy cards included).
const PRE_UPGRADE_REVISION_W44: &str =
    "7fa4dcb36ebf41ae6a452dda00149193d059dcd37836ea574a3cc76570ce81c0";
/// Revision a pre-0004 server returned for 2026-W41 from `2026-history.sql` (a week without strategy cards).
const PRE_UPGRADE_REVISION_W41: &str =
    "eaf04f5a234fa72d7b34a04ed47d184c9913d510329112bd8104f6407b3b0ccd";

/// Turns a `get_week` document into a `save_week` document by dropping the derived fields.
fn to_input(document: &Value) -> Value {
    let mut doc = document.clone();
    let root = doc.as_object_mut().unwrap();
    root.remove("schemaVersion");
    let week = root["week"].as_object_mut().unwrap();
    for key in ["isoYear", "isoWeek", "startDate", "endDate"] {
        week.remove(key);
    }
    let days = root["days"].as_array_mut().unwrap();
    days.retain(|d| !d["id"].is_null());
    for day in days {
        day.as_object_mut().unwrap().remove("date");
    }
    doc
}

#[tokio::test]
async fn revisions_from_before_the_upgrade_conflict() {
    let env = Env::upgraded_from_version_3().await;
    let app = env.app(journey_clock());

    for (week, old_revision) in [
        (44, PRE_UPGRADE_REVISION_W44),
        (41, PRE_UPGRADE_REVISION_W41),
    ] {
        let args = json!({"isoYear": 2026, "isoWeek": week});
        let current = ok(&app, "get_week", args).await;
        assert_ne!(current["revision"], old_revision, "W{week}");
        let mut edited = to_input(&current["document"]);
        edited["week"]["subtitle"] = json!("Edited after the upgrade");

        let before = env.dump().await;
        assert_conflict(
            &app,
            save_args(week, json!(old_revision), edited.clone()),
            STALE,
        )
        .await;
        assert_eq!(
            env.dump().await,
            before,
            "W{week}: a conflict changes nothing"
        );
        let unchanged = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": week})).await;
        assert_eq!(unchanged, current, "W{week}");

        // The revision that get_week returns after the upgrade replaces the week.
        let saved = ok(
            &app,
            "save_week",
            save_args(week, current["revision"].clone(), edited),
        )
        .await;
        assert_eq!(
            saved["document"]["week"]["subtitle"],
            "Edited after the upgrade"
        );
        assert_ne!(saved["revision"], current["revision"]);
        let reread = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": week})).await;
        assert_eq!(reread, saved, "W{week}");
    }
}

// ---- AC9 ----

#[tokio::test]
async fn read_tools_never_write_and_never_touch_the_write_pool() {
    let env = Env::with_fixtures().await;
    // A write pool that is closed: any use of it fails with a database error.
    let dead_write = db::open_write_pool(&env.path).await.unwrap();
    dead_write.close().await;
    let app = sous::app_with_mcp(
        env.read.clone(),
        journey_clock(),
        McpConfig {
            write_pool: dead_write,
            allowed_hosts: vec![],
            allowed_origins: vec![],
        },
    );
    let before = env.dump().await;
    let calls = [
        ("get_context", json!({})),
        ("list_weeks", json!({})),
        ("get_week", json!({})),
        ("get_week", json!({"isoYear": 2026, "isoWeek": 41})),
        ("search_meals", json!({"query": "casserole"})),
        (
            "search_meals",
            json!({"fromDate": "2026-09-01", "toDate": "2026-10-31", "limit": 2}),
        ),
        ("get_meal", json!({"mealId": CASSEROLE_DINNER})),
        ("get_shopping_lists", json!({})),
        ("get_shopping_lists", json!({"date": "2026-11-02"})),
    ];
    for (tool, args) in calls {
        ok(&app, tool, args).await;
    }
    assert_eq!(env.dump().await, before);

    // The read pool itself rejects writes; the only user of the write pool is save_week.
    assert!(
        sqlx::query("DELETE FROM weeks")
            .execute(&env.read)
            .await
            .is_err()
    );
    assert_eq!(
        err(&app, "save_week", save_args(45, Value::Null, week45())).await,
        "service_unavailable"
    );
    assert_eq!(env.dump().await, before);
}

#[tokio::test]
async fn pages_and_api_still_reject_mutations() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let before = env.dump().await;
    for uri in ["/", "/2026/44", "/api/2026/44"] {
        for method in ["POST", "PUT", "DELETE"] {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "localhost")
                .body(Body::from("{}"))
                .unwrap();
            let reply = send(&app, request).await;
            assert_eq!(
                reply.status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {uri}"
            );
        }
    }
    assert_eq!(env.dump().await, before);
    // The HTML fallback still answers unknown paths.
    let reply = send(
        &app,
        Request::get("/nope")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn database_failures_return_a_safe_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.db");
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    SqliteConnection::connect_with(&opts).await.unwrap();
    let pool = db::open_read_pool(&path).await.unwrap();
    let app = sous::app_with_mcp(
        pool.clone(),
        journey_clock(),
        McpConfig {
            write_pool: pool,
            allowed_hosts: vec![],
            allowed_origins: vec![],
        },
    );
    for (tool, args) in [
        ("get_week", json!({"isoYear": 2026, "isoWeek": 44})),
        ("list_weeks", json!({})),
        ("search_meals", json!({"query": "x"})),
        ("get_meal", json!({"mealId": CASSEROLE_DINNER})),
        ("get_shopping_lists", json!({})),
    ] {
        let result = call(&app, tool, args).await;
        assert_eq!(result["isError"], true, "{tool}");
        let error = &result["structuredContent"]["error"];
        assert_eq!(error["code"], "service_unavailable", "{tool}");
        let text = result.to_string().to_lowercase();
        for leak in ["sql", "table", "no such", "empty.db", "/var/", "/tmp"] {
            assert!(!text.contains(leak), "{tool} leaked {leak:?}: {text}");
        }
    }
}

#[tokio::test]
async fn host_and_origin_validation() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let list =
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta()}})
            .to_string();
    let modern = [
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", "tools/list"),
    ];
    let with = |extra: &[(&'static str, &'static str)]| {
        let mut all: Vec<(&'static str, &'static str)> = modern.to_vec();
        all.extend_from_slice(extra);
        all
    };

    let reply = post_raw(&app, list.clone(), &modern).await;
    assert_eq!(reply.status, StatusCode::OK);
    // Loopback hosts with a port are fine.
    let reply = post_raw(&app, list.clone(), &with(&[("host", "127.0.0.1:3000")])).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);

    // Another host is rejected (a duplicate header would be ambiguous, so build the request directly).
    let request = Request::post("/mcp")
        .header("host", "evil.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/list")
        .body(Body::from(list.clone()))
        .unwrap();
    assert_eq!(send(&app, request).await.status, StatusCode::FORBIDDEN);

    // Any Origin is rejected unless configured.
    let request = |origin: &str| {
        Request::post("/mcp")
            .header("host", "localhost")
            .header("origin", origin)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/list")
            .body(Body::from(list.clone()))
            .unwrap()
    };
    assert_eq!(
        send(&app, request("http://evil.example")).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, request("http://localhost:3000")).await.status,
        StatusCode::FORBIDDEN
    );

    let configured = env.app_with(
        journey_clock(),
        vec!["sous.example".into()],
        vec!["https://app.example:443".into()],
    );
    let ok_request = |host: &str, origin: &str| {
        Request::post("/mcp")
            .header("host", host)
            .header("origin", origin)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/list")
            .body(Body::from(list.clone()))
            .unwrap()
    };
    let reply = send(
        &configured,
        ok_request("sous.example", "https://app.example"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert_eq!(
        send(
            &configured,
            ok_request("sous.example", "https://other.example")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    // Configuring a host replaces the loopback defaults.
    assert_eq!(
        send(&configured, ok_request("localhost", "https://app.example"))
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    // Other routes are unchanged by the MCP settings.
    let page = send(
        &app,
        Request::get("/2026/44")
            .header("host", "evil.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
}

// ---- AC10: a real client over TCP ----

#[tokio::test]
async fn mcp_client_smoke_test() {
    use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use rmcp::{ClientLifecycleMode, ClientServiceExt, ServiceExt};

    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let handler = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("sous-smoke-client", "0.1.0"),
    );
    let transport = || {
        StreamableHttpClientTransport::from_config(StreamableHttpClientTransportConfig::with_uri(
            url.clone(),
        ))
    };
    let client = handler
        .clone()
        .serve_with_lifecycle(
            transport(),
            ClientLifecycleMode::Auto {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                legacy_version: None,
            },
        )
        .await
        .expect("client connects");
    let peer = client.peer_info().expect("peer info");
    println!(
        "client: sous-smoke-client 0.1.0 (rmcp 3.8); negotiated protocol version: {}; server: {} {}",
        peer.protocol_version,
        peer.server_info.as_ref().map_or("?", |s| s.name.as_str()),
        peer.server_info
            .as_ref()
            .map_or("?", |s| s.version.as_str()),
    );

    let tools = client.list_all_tools().await.expect("tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, TOOL_NAMES);

    async fn tool(
        client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
        name: &'static str,
        args: Value,
    ) -> Value {
        let Value::Object(arguments) = args else {
            panic!("arguments must be an object")
        };
        let result = client
            .call_tool(CallToolRequestParams::new(name).with_arguments(arguments))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_ne!(result.is_error, Some(true), "{name}: {result:?}");
        result.structured_content.expect("structured content")
    }

    let context = tool(&client, "get_context", json!({})).await;
    assert_eq!(context["today"], "2026-10-26");
    let saved = tool(
        &client,
        "save_week",
        save_args_omitting_revision(45, week45()),
    )
    .await;
    assert_eq!(saved["document"]["week"]["isoWeek"], 45);
    let found = tool(
        &client,
        "search_meals",
        json!({"query": "casserole", "fromDate": "2026-10-05", "toDate": "2026-10-11"}),
    )
    .await;
    assert_eq!(found["meals"][1]["id"], CASSEROLE_DINNER);
    let meal = tool(&client, "get_meal", json!({"mealId": CASSEROLE_DINNER})).await;
    assert_eq!(meal["steps"][3]["text"], "Bake 35 min until golden.");
    let shopping = tool(&client, "get_shopping_lists", json!({})).await;
    assert_eq!(shopping["lists"].as_array().unwrap().len(), 2);
    client.cancel().await.ok();

    // The legacy handshake works with the same client.
    let legacy = handler
        .serve(transport())
        .await
        .expect("legacy client connects");
    println!(
        "legacy handshake negotiated protocol version: {}",
        legacy.peer_info().unwrap().protocol_version
    );
    let tools = legacy.list_all_tools().await.expect("tools");
    assert_eq!(tools.len(), 8);
    legacy.cancel().await.ok();
}

// ---- Prep session weekday (feature 13) ----

#[tokio::test]
async fn save_week_prep_weekday_is_validated_stored_and_served() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());

    // Every accepted value round-trips through get_week, the API and the page document.
    let mut revision = Value::Null;
    for weekday in [json!(1), json!(4), json!(7), Value::Null] {
        let mut document = week45();
        document["prepSessions"][0]["weekday"] = weekday.clone();
        let saved = ok(&app, "save_week", save_args(45, revision.clone(), document)).await;
        revision = saved["revision"].clone();
        assert_eq!(saved["document"]["schemaVersion"], 3);
        assert_eq!(saved["document"]["prepSessions"][0]["weekday"], weekday);
        let fetched = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 45})).await;
        assert_eq!(fetched["document"]["prepSessions"][0]["weekday"], weekday);
        assert_eq!(fetched["revision"], revision);
    }

    // Anything else is invalid_input and changes nothing.
    let before = env.dump().await;
    for bad in [json!(0), json!(8), json!(-1), json!(1.5), json!("3")] {
        let mut document = week45();
        document["prepSessions"][0]["weekday"] = bad.clone();
        assert_eq!(
            err(&app, "save_week", save_args(45, revision.clone(), document)).await,
            "invalid_input",
            "{bad}"
        );
        assert_eq!(env.dump().await, before, "{bad}");
    }
    let mut missing = week45();
    missing["prepSessions"][0]
        .as_object_mut()
        .unwrap()
        .remove("weekday");
    assert_eq!(
        err(&app, "save_week", save_args(45, revision.clone(), missing)).await,
        "invalid_input"
    );
    assert_eq!(env.dump().await, before);
    let fetched = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 45})).await;
    assert_eq!(fetched["revision"], revision);

    // A weekday-only change alters the revision; an identical resave keeps it.
    let mut same = week45();
    same["prepSessions"][0]["weekday"] = Value::Null;
    let resaved = ok(&app, "save_week", save_args(45, revision.clone(), same)).await;
    assert_eq!(resaved["revision"], revision);
    let mut moved = week45();
    moved["prepSessions"][0]["weekday"] = json!(2);
    let changed = ok(&app, "save_week", save_args(45, revision.clone(), moved)).await;
    assert_ne!(changed["revision"], revision);
}

#[tokio::test]
async fn prep_weekday_appears_in_the_tool_schemas() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let tools = modern(&app, "tools/list", json!({})).await["tools"].clone();
    let tool = |name: &str| {
        tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()
            .clone()
    };
    let input = tool("save_week")["inputSchema"]["properties"]["document"]["properties"]
        ["prepSessions"]["items"]
        .clone();
    let weekday = &input["properties"]["weekday"];
    assert_eq!(weekday["type"], json!(["integer", "null"]));
    assert_eq!(
        (&weekday["minimum"], &weekday["maximum"]),
        (&json!(1), &json!(7))
    );
    assert!(
        input["required"]
            .as_array()
            .unwrap()
            .contains(&json!("weekday"))
    );

    let output = &tool("get_week")["outputSchema"]["properties"]["document"]["properties"]["prepSessions"]
        ["items"];
    let weekday = &output["properties"]["weekday"];
    assert!(weekday.to_string().contains("integer"), "{weekday}");
    assert!(weekday.to_string().contains("null"), "{weekday}");
}

// ---- Documentation stays in step with the server ----

#[tokio::test]
async fn mcp_tools_doc_matches_server() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let result = modern(&app, "tools/list", json!({})).await;
    if std::env::var_os("SOUS_REGEN_TOOLS_DOC").is_some() {
        let pretty = serde_json::to_string_pretty(&result["tools"]).unwrap() + "\n";
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/docs/mcp-tools.json"),
            pretty,
        )
        .unwrap();
        return;
    }
    let documented: Value = serde_json::from_str(TOOLS_DOC).expect("docs/mcp-tools.json is JSON");
    assert_eq!(
        documented, result["tools"],
        "docs/mcp-tools.json is out of date; regenerate it from tools/list"
    );
    assert_eq!(
        serde_json::to_string_pretty(&documented).unwrap() + "\n",
        TOOLS_DOC
    );
}

#[tokio::test]
async fn save_week_schema_admits_null_expected_revision_for_create() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let result = modern(&app, "tools/list", json!({})).await;
    let tool = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "save_week")
        .unwrap();
    let schema = &tool["inputSchema"];
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(!required.contains(&"expectedRevision"), "{required:?}");
    assert_eq!(
        schema["properties"]["expectedRevision"]["type"],
        json!(["string", "null"])
    );

    let validator = jsonschema::validator_for(schema).expect("inputSchema compiles");
    let example: Value = serde_json::from_str(SAVE_EXAMPLE).unwrap();
    let args = |revision: Option<Value>| {
        let mut args = json!({"isoYear": 2026, "isoWeek": 45, "document": example});
        if let Some(revision) = revision {
            args["expectedRevision"] = revision;
        }
        args
    };
    assert!(
        validator.is_valid(&args(Some(Value::Null))),
        "create example with null expectedRevision must validate"
    );
    assert!(validator.is_valid(&args(Some(json!("0123abcd")))));
    assert!(
        validator.is_valid(&args(None)),
        "create example without expectedRevision must validate"
    );
    assert!(!validator.is_valid(&args(Some(json!(7)))));
    let mut as_string = args(None);
    as_string["document"] = json!(serde_json::to_string(&example).unwrap());
    assert!(
        !validator.is_valid(&as_string),
        "a document sent as a JSON string must not validate"
    );
}

#[tokio::test]
async fn save_week_document_schema_is_inline() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let result = modern(&app, "tools/list", json!({})).await;
    let tool = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "save_week")
        .unwrap();
    let document = &tool["inputSchema"]["properties"]["document"];
    assert_eq!(document["type"], "object");
    assert_eq!(document["additionalProperties"], false);
    let mut required: Vec<&str> = document["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["days", "prepSessions", "shoppingLists", "week"]);
    assert_eq!(
        document["description"],
        "The complete week. Everything not listed here is deleted from the stored week."
    );
    let session = &document["properties"]["prepSessions"]["items"];
    let weekday = &session["properties"]["weekday"];
    assert_eq!(weekday["type"], json!(["integer", "null"]));
    assert_eq!(weekday["minimum"], 1);
    assert_eq!(weekday["maximum"], 7);
    assert!(
        session["required"]
            .as_array()
            .unwrap()
            .contains(&json!("weekday"))
    );
}

#[tokio::test]
async fn tool_schemas_are_self_contained() {
    fn walk(value: &Value, path: &str, tool: &str) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    assert!(key != "$ref" && key != "$defs", "{tool}: `{key}` at {path}");
                    walk(child, &format!("{path}/{key}"), tool);
                }
            }
            Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    walk(child, &format!("{path}/{i}"), tool);
                }
            }
            _ => {}
        }
    }
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let result = modern(&app, "tools/list", json!({})).await;
    for tool in result["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        walk(&tool["inputSchema"], "/inputSchema", name);
        walk(&tool["outputSchema"], "/outputSchema", name);
    }
}

/// Real results of all eight tools are valid against the published (inline) `outputSchema` and
/// against rmcp's old `$defs` schema, and a result with one wrongly typed value is rejected by both.
#[tokio::test]
async fn tool_results_match_old_and_inline_output_schemas() {
    use rmcp::handler::server::tool::schema_for_output as old;
    use sous::model::{
        Context, MealPage, MealRecord, ProfileSection, RevisedWeek, ShoppingDay, WeekPage,
    };
    let validator = |schema: &Value| jsonschema::validator_for(schema).unwrap();
    let old_schemas = |tool: &str| -> Value {
        match tool {
            "get_context" => serde_json::to_value(old::<Context>()),
            "list_weeks" => serde_json::to_value(old::<WeekPage>()),
            "search_meals" => serde_json::to_value(old::<MealPage>()),
            "get_meal" => serde_json::to_value(old::<MealRecord>()),
            "get_shopping_lists" => serde_json::to_value(old::<ShoppingDay>()),
            "update_profile_section" => serde_json::to_value(old::<ProfileSection>()),
            _ => serde_json::to_value(old::<RevisedWeek>()),
        }
        .unwrap()
    };

    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let listed = modern(&app, "tools/list", json!({})).await["tools"].clone();
    let example: Value = serde_json::from_str(SAVE_EXAMPLE).unwrap();
    let household_revision =
        ok(&app, "get_context", json!({})).await["profile"]["sections"][0]["revision"].clone();
    // (tool, arguments, JSON pointer to a value that is set to a wrong type)
    let calls = [
        (
            "update_profile_section",
            json!({"section": "household", "text": "Two adults.", "expectedRevision": household_revision}),
            "/revision",
        ),
        ("get_context", json!({}), "/today"),
        ("get_context", json!({}), "/profile/sections/0/text"),
        ("list_weeks", json!({}), "/weeks/0/isoYear"),
        ("get_week", week_args(44), "/document/week/id"),
        ("search_meals", json!({"query": "casserole"}), "/meals/0/id"),
        ("get_meal", json!({"mealId": CASSEROLE_DINNER}), "/weekday"),
        (
            "get_shopping_lists",
            json!({"date": "2026-10-26"}),
            "/planExists",
        ),
        (
            "save_week",
            save_args_omitting_revision(46, example),
            "/document/prepSessions/0/weekday",
        ),
    ];
    for (tool, args, pointer) in calls {
        let published = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == tool)
            .unwrap()["outputSchema"]
            .clone();
        let validators = [validator(&published), validator(&old_schemas(tool))];
        let result = ok(&app, tool, args).await;
        let mut broken = result.clone();
        *broken
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("{tool}: no {pointer}")) = json!(7.5);
        for v in &validators {
            assert!(v.is_valid(&result), "{tool}: {result}");
            assert!(!v.is_valid(&broken), "{tool}: {broken}");
        }
    }
}

#[tokio::test]
async fn save_week_rejects_a_document_sent_as_a_string() {
    let example: Value = serde_json::from_str(SAVE_EXAMPLE).unwrap();
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let before = env.dump().await;
    let mut args = week_args(46);
    args["document"] = json!(serde_json::to_string(&example).unwrap());
    assert_eq!(err(&app, "save_week", args).await, "invalid_input");
    assert_eq!(env.dump().await, before);
}

fn week_args(iso_week: u32) -> Value {
    json!({"isoYear": 2026, "isoWeek": iso_week})
}

#[tokio::test]
async fn save_week_without_expected_revision_creates_a_new_week() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    assert_eq!(err(&app, "get_week", week_args(45)).await, "week_not_found");

    let saved = ok(&app, "save_week", save_args_omitting_revision(45, week45())).await;
    assert_eq!(saved["document"]["week"]["isoWeek"], 45);
    assert_subset(&week45(), &saved["document"], "document");

    let fetched = ok(&app, "get_week", week_args(45)).await;
    assert_eq!(fetched["document"], saved["document"]);
    assert_eq!(fetched["revision"], saved["revision"]);

    let (status, api) = get_json(&app, "/api/2026/45").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(api, saved["document"]);
    let page = send(
        &app,
        Request::get("/2026/45")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    let payload = page_json(&page.text);
    assert_eq!(payload["kind"], "week");
    assert_eq!(payload["document"], saved["document"]);
}

#[tokio::test]
async fn save_week_without_expected_revision_on_a_saved_week_conflicts() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    ok(&app, "save_week", save_args_omitting_revision(45, week45())).await;
    let before = env.dump().await;
    let mut edited = week45();
    edited["days"][0]["meals"][0]["name"] = json!("Porridge");
    assert_conflict(&app, save_args_omitting_revision(45, edited), ALREADY_SAVED).await;
    assert_eq!(env.dump().await, before);
}

#[tokio::test]
async fn save_week_with_null_expected_revision_is_unchanged() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let saved = ok(&app, "save_week", save_args(45, Value::Null, week45())).await;
    assert_eq!(saved["document"]["week"]["isoWeek"], 45);
    let fetched = ok(&app, "get_week", week_args(45)).await;
    assert_eq!(fetched["revision"], saved["revision"]);

    let before = env.dump().await;
    let mut edited = week45();
    edited["days"][0]["meals"][0]["name"] = json!("Porridge");
    assert_conflict(&app, save_args(45, Value::Null, edited), ALREADY_SAVED).await;
    assert_eq!(env.dump().await, before);
}

#[tokio::test]
async fn save_week_revision_for_an_unsaved_week_conflicts() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let before = env.dump().await;
    for revision in ["null", "deadbeef", ""] {
        assert_conflict(&app, save_args(45, json!(revision), week45()), NOT_SAVED).await;
        assert_eq!(env.dump().await, before, "{revision:?}");
        assert_eq!(err(&app, "get_week", week_args(45)).await, "week_not_found");
    }
}

#[tokio::test]
async fn save_week_matching_and_stale_revisions() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let saved = ok(&app, "save_week", save_args(45, Value::Null, week45())).await;
    let revision = saved["revision"].as_str().unwrap().to_owned();
    let mut edited = week45();
    edited["days"][0]["meals"][0]["name"] = json!("Porridge");

    let before = env.dump().await;
    for stale in ["stale", "null"] {
        assert_conflict(&app, save_args(45, json!(stale), edited.clone()), STALE).await;
    }
    assert_eq!(env.dump().await, before);

    let revised = ok(
        &app,
        "save_week",
        save_args(45, json!(revision), edited.clone()),
    )
    .await;
    assert_ne!(revised["revision"], saved["revision"]);
    assert_eq!(
        revised["document"]["days"][0]["meals"][0]["name"],
        "Porridge"
    );

    // The old revision is now stale.
    let after = env.dump().await;
    assert_conflict(&app, save_args(45, json!(revision), edited), STALE).await;
    assert_eq!(env.dump().await, after);
}

/// Every key of `input` must appear in `actual` with an equal value; read-only extras are allowed.
fn assert_subset(input: &Value, actual: &Value, path: &str) {
    match (input, actual) {
        (Value::Object(i), Value::Object(a)) => {
            for (key, value) in i {
                let other = a.get(key).unwrap_or_else(|| panic!("{path}.{key} missing"));
                assert_subset(value, other, &format!("{path}.{key}"));
            }
        }
        (Value::Array(i), Value::Array(a)) => {
            // The stored document always lists all seven days; absent ones come back as empty.
            if path == "document.days" {
                assert_eq!(a.len(), 7);
            } else {
                assert_eq!(i.len(), a.len(), "{path} length");
            }
            for (n, (i, a)) in i.iter().zip(a).enumerate() {
                assert_subset(i, a, &format!("{path}[{n}]"));
            }
        }
        (Value::Number(i), Value::Number(a)) => assert_eq!(i.as_f64(), a.as_f64(), "{path}"),
        _ => assert_eq!(input, actual, "{path}"),
    }
}

#[tokio::test]
async fn save_week_example_round_trips() {
    let example: Value = serde_json::from_str(SAVE_EXAMPLE).expect("example is JSON");
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let saved = ok(
        &app,
        "save_week",
        save_args(46, Value::Null, example.clone()),
    )
    .await;
    let fetched = ok(&app, "get_week", json!({"isoYear": 2026, "isoWeek": 46})).await;
    assert_eq!(saved, fetched);
    assert_subset(&example, &fetched["document"], "document");

    // The example uses every optional part of the document.
    let days = example["days"].as_array().unwrap();
    assert!(days.iter().any(|d| !d["reminder"].is_null()));
    assert!(
        days.iter()
            .any(|d| d["meals"].as_array().is_some_and(|m| !m.is_empty()))
    );
    assert!(
        example["shoppingLists"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["categories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["items"].as_array().unwrap().is_empty()))
    );
    assert!(!example["prepSessions"].as_array().unwrap().is_empty());
    assert!(example.get("strategyCards").is_none());
    let shared = example["shoppingLists"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| {
            days.iter()
                .filter(|d| d["shoppingListIds"].as_array().unwrap().contains(&l["id"]))
                .count()
                > 1
        })
        .count();
    assert!(shared >= 2, "two lists are shared across days");

    // Re-saving the same document with the new revision is accepted and changes nothing.
    let again = ok(
        &app,
        "save_week",
        save_args(46, fetched["revision"].clone(), example),
    )
    .await;
    assert_eq!(again, fetched);
}

// ---- Household profile (feature 17) ----

const FOOD: &str = "food";

fn profile_revision(key: &str, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher.update([0u8]);
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

const KEYS: [&str; 5] = ["household", "food", "kitchen", "planning", "other"];
const TITLES: [&str; 5] = [
    "People",
    "Food preferences",
    "Kitchen",
    "Planning habits",
    "Other context",
];
const BASE_HEADERS: [(&str, &str); 2] =
    [("host", "localhost"), ("content-type", "application/json")];

/// A PUT with exactly these headers; every response must be `no-store` and without CORS headers.
async fn put_raw(app: &Router, uri: &str, headers: &[(&str, &str)], body: Body) -> Reply {
    let mut builder = Request::put(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let reply = send(app, builder.body(body).unwrap()).await;
    assert_eq!(reply.headers["cache-control"], "no-store", "{uri}");
    reply
}

/// A PUT of `body` to `/api/profile/{key}` with the standard headers.
async fn put_json(app: &Router, key: &str, body: Value) -> Reply {
    put_raw(
        app,
        &format!("/api/profile/{key}"),
        &BASE_HEADERS,
        Body::from(body.to_string()),
    )
    .await
}

fn put_body(text: &str, revision: &str) -> Value {
    json!({"text": text, "expectedRevision": revision})
}

async fn http_profile(app: &Router) -> Value {
    let (status, value) = get_json(app, "/api/profile").await;
    assert_eq!(status, StatusCode::OK);
    value
}

fn section(profile: &Value, key: &str) -> Value {
    profile["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap_or_else(|| panic!("no section {key}"))
        .clone()
}

fn error_code(reply: &Reply) -> String {
    reply.json()["error"]["code"].as_str().unwrap().to_owned()
}

/// `env.dump()` without the profile table, so week data can be compared across profile writes.
fn without_profile(dump: Vec<String>) -> Vec<String> {
    let mut skipping = false;
    dump.into_iter()
        .filter(|line| {
            if line.starts_with("== ") {
                skipping = line.starts_with("== profile_sections");
            }
            !skipping
        })
        .collect()
}

#[tokio::test]
async fn ac1_a_fresh_agent_gets_texts_saved_over_mcp_and_http() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let start = http_profile(&app).await;
    let household = section(&start, "household");
    let food = section(&start, FOOD);

    let saved = ok(
        &app,
        "update_profile_section",
        json!({"section": "household", "text": "Two adults, one child.\nNo pork.",
               "expectedRevision": household["revision"]}),
    )
    .await;
    assert_eq!(saved["key"], "household");
    assert_eq!(saved["title"], "People");
    assert_eq!(saved["text"], "Two adults, one child.\nNo pork.");
    let reply = put_json(
        &app,
        FOOD,
        put_body("Loves lentils.", food["revision"].as_str().unwrap()),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let saved_food = reply.json();

    // A new app and a new MCP request know nothing of the saves except the database.
    let fresh = env.app(journey_clock());
    let context = ok(&fresh, "get_context", json!({})).await;
    let sections = context["profile"]["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 5);
    let order: Vec<&str> = sections
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert_eq!(order, KEYS);
    let titles: Vec<&str> = sections
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, TITLES);
    assert_eq!(sections[0], saved);
    assert_eq!(sections[1], saved_food);
    assert_eq!(
        saved_food["revision"],
        profile_revision(FOOD, "Loves lentils.")
    );
    for section in &sections[2..] {
        assert_eq!(section["text"], "");
    }
    // The original context fields are unchanged.
    assert_eq!(context["today"], "2026-10-26");
    assert_eq!(context["currentWeek"]["path"], "/2026/44");
}

#[tokio::test]
async fn ac2_other_sections_and_weeks_survive_writes_and_a_restart() {
    let env = Env::with_fixtures().await;
    let app = env.app(journey_clock());
    let weeks_before = without_profile(env.dump().await);
    let week = ok(&app, "get_week", json!({})).await;
    let start = http_profile(&app).await;

    let saved = ok(
        &app,
        "update_profile_section",
        json!({"section": "kitchen", "text": "Small oven.", "expectedRevision": section(&start, "kitchen")["revision"]}),
    )
    .await;
    let reply = put_json(
        &app,
        "other",
        put_body(
            "Allergic to nuts.",
            section(&start, "other")["revision"].as_str().unwrap(),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let saved_other = reply.json();

    let after = http_profile(&app).await;
    for key in ["household", FOOD, "planning"] {
        assert_eq!(section(&after, key), section(&start, key), "{key}");
    }
    assert_eq!(section(&after, "kitchen"), saved);
    assert_eq!(section(&after, "other"), saved_other);
    // Every week table row and every week revision is unchanged, over both transports.
    assert_eq!(without_profile(env.dump().await), weeks_before);
    assert_eq!(ok(&app, "get_week", json!({})).await, week);

    // Close everything, reopen the same file with a fresh app: the text is still there.
    env.read.close().await;
    env.write.close().await;
    let read = db::open_read_pool(&env.path).await.unwrap();
    let write = db::open_write_pool(&env.path).await.unwrap();
    let reopened = sous::app_with_mcp(
        read,
        journey_clock(),
        McpConfig {
            write_pool: write,
            allowed_hosts: vec![],
            allowed_origins: vec![],
        },
    );
    assert_eq!(http_profile(&reopened).await, after);
    let context = ok(&reopened, "get_context", json!({})).await;
    assert_eq!(context["profile"], after);
}

#[tokio::test]
async fn ac3_a_stale_revision_conflicts_over_mcp_and_http_and_writes_nothing() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let start = section(&http_profile(&app).await, FOOD);
    let stale = start["revision"].as_str().unwrap();
    let first = ok(
        &app,
        "update_profile_section",
        json!({"section": FOOD, "text": "First.", "expectedRevision": stale}),
    )
    .await;
    let before = env.dump().await;

    // MCP.
    let result = call(
        &app,
        "update_profile_section",
        json!({"section": FOOD, "text": "Second.", "expectedRevision": stale}),
    )
    .await;
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["structuredContent"]["error"],
        json!({"code": "revision_conflict", "message":
            "The section changed since it was read. Call get_context again, reconcile with the user, and resubmit."})
    );
    // HTTP: 409 with the section as now saved.
    let reply = put_json(&app, FOOD, put_body("Second.", stale)).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(
        reply.json(),
        json!({"error": {"code": "revision_conflict",
                         "message": "The section changed since it was read."},
               "current": first})
    );
    assert_eq!(env.dump().await, before);

    // The revision of an empty section is not a wildcard, and an empty revision never matches.
    for revision in ["", "null", "0"] {
        let reply = put_json(&app, FOOD, put_body("x", revision)).await;
        assert_eq!(reply.status, StatusCode::CONFLICT, "{revision:?}");
    }
    assert_eq!(env.dump().await, before);
}

#[tokio::test]
async fn ac3_writes_to_different_sections_never_conflict() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let start = http_profile(&app).await;
    let rev = |key: &str| {
        section(&start, key)["revision"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    ok(
        &app,
        "update_profile_section",
        json!({"section": "household", "text": "A", "expectedRevision": rev("household")}),
    )
    .await;
    // Food is saved from the revision read before the household save.
    let reply = put_json(&app, FOOD, put_body("B", &rev(FOOD))).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    ok(
        &app,
        "update_profile_section",
        json!({"section": "kitchen", "text": "C", "expectedRevision": rev("kitchen")}),
    )
    .await;
}

/// A second write pool on the same file, so two writers really contend for SQLite's write lock.
async fn second_write_pool(env: &Env) -> SqlitePool {
    db::open_write_pool(&env.path).await.unwrap()
}

/// Hold a section write inside its transaction (as the MCP tool's writer would), start another write over
/// the other transport from the same revision, then release. Returns the paused write's result and the
/// other transport's reply.
async fn race(
    env: &Env,
    other: &'static str,
) -> (
    Result<sous::model::ProfileSection, sous::profile::UpdateError>,
    Value,
) {
    let held_pool = second_write_pool(env).await;
    let app = sous::app_with_mcp(
        env.read.clone(),
        journey_clock(),
        McpConfig {
            write_pool: env.write.clone(),
            allowed_hosts: vec![],
            allowed_origins: vec![],
        },
    );
    let revision = section(&http_profile(&app).await, "other")["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let held = tokio::spawn({
        let revision = revision.clone();
        async move {
            sous::profile::update_section_with_pause(
                &held_pool,
                sous::model::SectionKey::Other,
                "Held writer",
                &revision,
                async {
                    let _ = entered_tx.send(());
                    let _ = release_rx.await;
                },
            )
            .await
        }
    });
    entered_rx.await.unwrap();
    let contender = tokio::spawn({
        let app = app.clone();
        async move {
            if other == "http" {
                let reply = put_json(&app, "other", put_body("Contender", &revision)).await;
                json!({"status": reply.status.as_u16(), "body": reply.json()})
            } else {
                call(
                    &app,
                    "update_profile_section",
                    json!({"section": "other", "text": "Contender", "expectedRevision": revision}),
                )
                .await
            }
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !contender.is_finished(),
        "the contender waits for the write lock"
    );
    release_tx.send(()).unwrap();
    (held.await.unwrap(), contender.await.unwrap())
}

#[tokio::test]
async fn ac3_a_held_write_makes_a_simultaneous_http_put_conflict() {
    let env = Env::empty().await;
    let (held, contender) = race(&env, "http").await;
    let winner = held.expect("the held write wins");
    assert_eq!(contender["status"], 409);
    assert_eq!(contender["body"]["error"]["code"], "revision_conflict");
    assert_eq!(
        contender["body"]["current"],
        serde_json::to_value(&winner).unwrap()
    );
    let app = env.app(journey_clock());
    assert_eq!(
        section(&http_profile(&app).await, "other")["text"],
        "Held writer"
    );
}

#[tokio::test]
async fn ac3_a_held_write_makes_a_simultaneous_mcp_call_conflict() {
    let env = Env::empty().await;
    let (held, contender) = race(&env, "mcp").await;
    held.expect("the held write wins");
    assert_eq!(contender["isError"], true, "{contender}");
    assert_eq!(
        contender["structuredContent"]["error"]["code"],
        "revision_conflict"
    );
    let app = env.app(journey_clock());
    assert_eq!(
        section(&http_profile(&app).await, "other")["text"],
        "Held writer"
    );
}

#[tokio::test]
async fn ac3_the_http_write_wins_when_it_commits_first() {
    // The other way round: HTTP commits first, so the MCP writer from the same revision conflicts.
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let revision = section(&http_profile(&app).await, "other")["revision"].clone();
    let reply = put_json(&app, "other", put_body("HTTP", revision.as_str().unwrap())).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        err(
            &app,
            "update_profile_section",
            json!({"section": "other", "text": "MCP", "expectedRevision": revision}),
        )
        .await,
        "revision_conflict"
    );
    assert_eq!(section(&http_profile(&app).await, "other")["text"], "HTTP");
}

#[tokio::test]
async fn ac5_an_empty_profile_is_normal() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let context = ok(&app, "get_context", json!({})).await;
    let sections = context["profile"]["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 5);
    for (i, section) in sections.iter().enumerate() {
        assert_eq!(section["key"], KEYS[i]);
        assert_eq!(section["text"], "");
        assert_eq!(section["revision"], profile_revision(KEYS[i], ""));
    }
    // Planning works: save a week, and both pages render.
    let saved = ok(&app, "save_week", save_args_omitting_revision(45, week45())).await;
    assert_eq!(saved["document"]["week"]["isoWeek"], 45);
    for uri in ["/", "/profile"] {
        let page = send(
            &app,
            Request::get(uri)
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(page.status, StatusCode::OK, "{uri}");
    }
    let api = http_profile(&app).await;
    assert_eq!(api["sections"].as_array().unwrap().len(), 5);
    assert!(
        api["sections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["text"] == "")
    );
}

const HOUSEHOLD_EMPTY_REVISION: &str =
    "b917a034b171297d58219707fac1eb9699d8479627683283b1590c7f4e90db0e";

#[tokio::test]
async fn feature_18_household_section_is_titled_people_everywhere() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());

    // get_context and GET /api/profile, on an empty database.
    let context = ok(&app, "get_context", json!({})).await;
    assert_eq!(context["profile"]["sections"][0]["key"], "household");
    assert_eq!(context["profile"]["sections"][0]["title"], "People");
    let http = http_profile(&app).await;
    assert_eq!(section(&http, "household")["title"], "People");
    assert_eq!(http, context["profile"]);

    // The revision hashes key and text, never the title: it equals the pre-change value.
    assert_eq!(
        context["profile"]["sections"][0]["revision"],
        HOUSEHOLD_EMPTY_REVISION
    );
    assert_eq!(
        section(&http, "household")["revision"],
        HOUSEHOLD_EMPTY_REVISION
    );

    // The profile page's initial JSON.
    let page = send(
        &app,
        Request::get("/profile")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page_json(&page.text)["sections"][0]["title"], "People");

    // The server instructions name the sections by title.
    let server = modern(&app, "server/discover", json!({})).await;
    assert!(
        server["instructions"]
            .as_str()
            .unwrap()
            .contains("(people, food preferences, kitchen, planning habits, other context)")
    );

    // A PUT with the pre-change revision succeeds, and its 200 body says "People".
    let reply = put_json(
        &app,
        "household",
        put_body("Two adults.", HOUSEHOLD_EMPTY_REVISION),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert_eq!(reply.json()["title"], "People");
    assert_eq!(reply.json()["key"], "household");

    // The 409 body's current section.
    let reply = put_json(
        &app,
        "household",
        put_body("Late.", HOUSEHOLD_EMPTY_REVISION),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.json()["current"]["title"], "People");

    // The MCP update result.
    let revision = profile_revision("household", "Two adults.");
    let saved = ok(
        &app,
        "update_profile_section",
        json!({"section": "household", "text": "Three.", "expectedRevision": revision}),
    )
    .await;
    assert_eq!(saved["title"], "People");
}

#[tokio::test]
async fn profile_page_embeds_the_sections() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let start = http_profile(&app).await;
    let text = "Line one\n</script><img src=x onerror=alert(1)>";
    put_json(
        &app,
        "other",
        put_body(text, section(&start, "other")["revision"].as_str().unwrap()),
    )
    .await;
    let page = send(
        &app,
        Request::get("/profile")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.headers["cache-control"], "no-store");
    assert!(page.text.contains("<title>Household — Meal Plan</title>"));
    assert!(!page.text.contains("</script><img"), "the text is escaped");
    let payload = page_json(&page.text);
    assert_eq!(payload["kind"], "profile");
    assert_eq!(payload["sections"], http_profile(&app).await["sections"]);
    assert_eq!(payload["sections"][4]["text"], text);
}

#[tokio::test]
async fn profile_page_back_is_built_by_the_server() {
    let env = Env::empty().await;
    // Tuesday 2026-11-17 in Berlin.
    let app = env.app(clock_at("Europe/Berlin", 2026, 11, 17, 12, 0, 0));
    let today = json!({"kind": "today", "href": "/2026-11-17"});
    let day = json!({"kind": "day", "href": "/2026-11-11", "date": "2026-11-11"});
    let cases: Vec<(&str, Value)> = vec![
        ("?from=/2026-11-11", day.clone()),
        ("?from=/2026-11-17", today.clone()),
        (
            "?from=/2026",
            json!({"kind": "year", "href": "/2026", "year": 2026}),
        ),
        (
            "?from=/0001",
            json!({"kind": "year", "href": "/0001", "year": 1}),
        ),
        ("", today.clone()),
        ("?from=", today.clone()),
        ("?from=/2026/44", today.clone()),
        ("?from=/2026-02-30", today.clone()),
        ("?from=//example.com", today.clone()),
        ("?from=https://example.com/2026-11-11", today.clone()),
        ("?from=/2026-11-11/extra", today.clone()),
        ("?from=%2F2026-11-11", day.clone()),
        ("?from=%FF", today.clone()),
        ("?from=/2026-11-11&from=/2026", today.clone()),
        ("?from=/2026-11-11%3Cscript%3E", today.clone()),
    ];
    for (query, expected) in cases {
        let uri = format!("/profile{query}");
        let page = send(
            &app,
            Request::get(uri.as_str())
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(page.status, StatusCode::OK, "{uri}");
        let payload = page_json(&page.text);
        assert_eq!(payload["kind"], "profile", "{uri}");
        assert_eq!(payload["back"], expected, "{uri}");
        let href = payload["back"]["href"].as_str().unwrap();
        assert!(
            ["/2026-11-11", "/2026", "/0001", "/2026-11-17"].contains(&href),
            "{uri}: {href}"
        );
        for needle in ["example.com", "2026-11-11<script>", "/extra"] {
            assert!(!page.text.contains(needle), "{uri} echoes {needle}");
        }
    }
}

// ---- the PUT contract, in the spec's order ----

#[tokio::test]
async fn put_checks_run_in_the_documented_order() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let rev = profile_revision(FOOD, "");
    let valid = put_body("x", &rev).to_string();
    let put = |uri: &'static str, headers: Vec<(&'static str, &'static str)>, body: String| {
        let app = app.clone();
        async move { put_raw(&app, uri, &headers, Body::from(body)).await }
    };

    // 1. forbidden beats everything: unknown key, wrong media type, huge body, bad JSON.
    let evil = vec![
        ("host", "localhost"),
        ("origin", "http://evil.example"),
        ("content-type", "text/plain"),
    ];
    let reply = put("/api/profile/nope", evil.clone(), "x".repeat(70_000)).await;
    assert_eq!(
        (reply.status, error_code(&reply).as_str()),
        (StatusCode::FORBIDDEN, "forbidden")
    );
    let reply = put(
        "/api/profile/food",
        vec![
            ("host", "evil.example"),
            ("content-type", "application/json"),
        ],
        valid.clone(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    // 2. section_not_found beats the media type, the size and the body.
    let reply = put(
        "/api/profile/nope",
        vec![("host", "localhost"), ("content-type", "text/plain")],
        "x".repeat(70_000),
    )
    .await;
    assert_eq!(
        (reply.status, error_code(&reply).as_str()),
        (StatusCode::NOT_FOUND, "section_not_found")
    );
    // 3. unsupported_media_type beats the size and the body.
    let reply = put(
        "/api/profile/food",
        vec![("host", "localhost"), ("content-type", "text/plain")],
        "x".repeat(70_000),
    )
    .await;
    assert_eq!(
        (reply.status, error_code(&reply).as_str()),
        (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type")
    );
    // 4. payload_too_large beats invalid JSON.
    let reply = put(
        "/api/profile/food",
        BASE_HEADERS.to_vec(),
        "x".repeat(70_000),
    )
    .await;
    assert_eq!(
        (reply.status, error_code(&reply).as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
    );
    // 5. invalid_input beats the revision conflict.
    let reply = put(
        "/api/profile/food",
        BASE_HEADERS.to_vec(),
        put_body(&"x".repeat(4001), "stale").to_string(),
    )
    .await;
    assert_eq!(
        (reply.status, error_code(&reply).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_input")
    );
    // 6. revision_conflict.
    let reply = put(
        "/api/profile/food",
        BASE_HEADERS.to_vec(),
        put_body("x", "stale").to_string(),
    )
    .await;
    assert_eq!(
        (reply.status, error_code(&reply).as_str()),
        (StatusCode::CONFLICT, "revision_conflict")
    );
    // 8. 200.
    let reply = put("/api/profile/food", BASE_HEADERS.to_vec(), valid).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.json(),
        json!({"key": "food", "title": "Food preferences", "text": "x",
               "revision": profile_revision(FOOD, "x")})
    );
    assert_eq!(reply.headers["content-type"], "application/json");
}

#[tokio::test]
async fn put_503_when_the_database_fails_or_there_is_no_write_configuration() {
    let env = Env::empty().await;
    let rev = profile_revision(FOOD, "");
    // The writable pool is closed: a database failure, with no internals in the message.
    let dead = db::open_write_pool(&env.path).await.unwrap();
    dead.close().await;
    let app = sous::app_with_mcp(
        env.read.clone(),
        journey_clock(),
        McpConfig {
            write_pool: dead,
            allowed_hosts: vec![],
            allowed_origins: vec![],
        },
    );
    let reply = put_json(&app, FOOD, put_body("x", &rev)).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "service_unavailable");
    let text = reply.text.to_lowercase();
    for leak in ["sql", "pool", "test.db", "/var/", "/tmp"] {
        assert!(!text.contains(leak), "leaked {leak:?}: {text}");
    }
    // The read-only router has no write configuration.
    let read_only = sous::app(env.read.clone(), journey_clock());
    let reply = put_json(&read_only, FOOD, put_body("x", &rev)).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&reply), "service_unavailable");
    // ... and nothing was written.
    assert_eq!(section(&http_profile(&read_only).await, FOOD)["text"], "");
}

#[tokio::test]
async fn put_accepts_json_media_types_in_any_case_with_parameters() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let mut revision = profile_revision(FOOD, "");
    for (i, content_type) in [
        "application/json",
        "application/json; charset=utf-8",
        "Application/JSON",
        "APPLICATION/JSON ;charset=UTF-8",
    ]
    .into_iter()
    .enumerate()
    {
        let text = format!("Try {i}");
        let reply = put_raw(
            &app,
            "/api/profile/food",
            &[("host", "localhost"), ("content-type", content_type)],
            Body::from(put_body(&text, &revision).to_string()),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{content_type}: {}",
            reply.text
        );
        revision = reply.json()["revision"].as_str().unwrap().to_owned();
    }
    for content_type in [
        None,
        Some("text/plain"),
        Some("application/jsonp"),
        Some("application/x-www-form-urlencoded"),
        Some("application/json-patch+json"),
        Some("multipart/form-data; boundary=x"),
        Some(""),
    ] {
        let mut headers = vec![("host", "localhost")];
        if let Some(content_type) = content_type {
            headers.push(("content-type", content_type));
        }
        let reply = put_raw(
            &app,
            "/api/profile/food",
            &headers,
            Body::from(put_body("x", &revision).to_string()),
        )
        .await;
        assert_eq!(
            (reply.status, error_code(&reply).as_str()),
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type"),
            "{content_type:?}"
        );
    }
}

#[tokio::test]
async fn put_limits_the_body_to_64_kib_with_or_without_content_length() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let mut revision = profile_revision(FOOD, "");
    let body = |len: usize, revision: &str| {
        let json = put_body("x", "REV").to_string();
        let json = json.replace("REV", revision);
        // Pad with insignificant whitespace to reach exactly `len` bytes.
        assert!(json.len() <= len);
        format!("{json}{}", " ".repeat(len - json.len()))
    };

    for len in [65_537usize, 70_000, 1_000_000] {
        let text = body(len, &revision);
        let reply = put_raw(&app, "/api/profile/food", &BASE_HEADERS, streamed(&text)).await;
        assert_eq!(
            (reply.status, error_code(&reply).as_str()),
            (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            "{len} bytes, no Content-Length"
        );
        let length = len.to_string();
        let reply = put_raw(
            &app,
            "/api/profile/food",
            &[
                ("host", "localhost"),
                ("content-type", "application/json"),
                ("content-length", &length),
            ],
            Body::from(text),
        )
        .await;
        assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE, "{len} bytes");
    }
    // Exactly 65,536 bytes is accepted, streamed or not.
    let text = body(65_536, &revision);
    let reply = put_raw(&app, "/api/profile/food", &BASE_HEADERS, Body::from(text)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    revision = reply.json()["revision"].as_str().unwrap().to_owned();
    let text = body(65_536, &revision);
    let reply = put_raw(&app, "/api/profile/food", &BASE_HEADERS, streamed(&text)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
}

/// A streamed body of 8 KiB chunks: it has no `Content-Length` and no exact size hint.
fn streamed(text: &str) -> Body {
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = text
        .as_bytes()
        .chunks(8192)
        .map(|chunk| Ok(chunk.to_vec()))
        .collect();
    Body::from_stream(futures_util::stream::iter(chunks))
}

#[tokio::test]
async fn put_rejects_malformed_bodies_as_invalid_input() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let rev = profile_revision(FOOD, "");
    let before = env.dump().await;
    let bad: Vec<(&str, Vec<u8>)> = vec![
        ("empty body", vec![]),
        ("not JSON", b"text=x".to_vec()),
        ("truncated", br#"{"text":"x""#.to_vec()),
        ("array", b"[]".to_vec()),
        ("string", br#""x""#.to_vec()),
        ("null", b"null".to_vec()),
        (
            "invalid UTF-8",
            b"{\"text\":\"\xff\",\"expectedRevision\":\"r\"}".to_vec(),
        ),
        (
            "missing text",
            json!({"expectedRevision": rev}).to_string().into_bytes(),
        ),
        (
            "missing revision",
            json!({"text": "x"}).to_string().into_bytes(),
        ),
        ("empty object", b"{}".to_vec()),
        (
            "text number",
            json!({"text": 1, "expectedRevision": rev})
                .to_string()
                .into_bytes(),
        ),
        (
            "text null",
            json!({"text": null, "expectedRevision": rev})
                .to_string()
                .into_bytes(),
        ),
        (
            "revision number",
            json!({"text": "x", "expectedRevision": 1})
                .to_string()
                .into_bytes(),
        ),
        (
            "revision null",
            json!({"text": "x", "expectedRevision": null})
                .to_string()
                .into_bytes(),
        ),
        (
            "unknown field",
            json!({"text": "x", "expectedRevision": rev, "force": true})
                .to_string()
                .into_bytes(),
        ),
        (
            "snake case revision",
            json!({"text": "x", "expected_revision": rev})
                .to_string()
                .into_bytes(),
        ),
        (
            "4001 characters",
            put_body(&"x".repeat(4001), &rev).to_string().into_bytes(),
        ),
        (
            "4001 multi-byte characters",
            put_body(&"é".repeat(4001), &rev).to_string().into_bytes(),
        ),
        (
            "2001 CRLF pairs, 4002 characters as submitted",
            put_body(&"\r\n".repeat(2001), &rev)
                .to_string()
                .into_bytes(),
        ),
    ];
    for (name, body) in bad {
        let reply = put_raw(&app, "/api/profile/food", &BASE_HEADERS, Body::from(body)).await;
        assert_eq!(
            (reply.status, error_code(&reply).as_str()),
            (StatusCode::BAD_REQUEST, "invalid_input"),
            "{name}: {}",
            reply.text
        );
        assert!(
            reply.json()["error"]["message"].as_str().unwrap().len() > 10,
            "{name}"
        );
    }
    assert_eq!(env.dump().await, before, "nothing was written");
    assert_eq!(
        put_json(&app, FOOD, put_body(&"x".repeat(4001), &rev))
            .await
            .json()["error"]["message"],
        "Text must be at most 4,000 characters."
    );

    // Exactly 4,000 multi-byte characters are accepted, and an empty text clears a section.
    let text = "é".repeat(4000);
    let reply = put_json(&app, FOOD, put_body(&text, &rev)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    let saved = reply.json();
    assert_eq!(saved["text"], text.as_str());
    let reply = put_json(
        &app,
        FOOD,
        put_body("", saved["revision"].as_str().unwrap()),
    )
    .await;
    assert_eq!(reply.json()["revision"], rev);
}

#[tokio::test]
async fn put_turns_crlf_into_lf_and_changes_nothing_else() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let rev = profile_revision("kitchen", "");
    let text = "  Oven.\r\nHob.\rGrill\n\r\n  ";
    let reply = put_json(&app, "kitchen", put_body(text, &rev)).await;
    assert_eq!(reply.status, StatusCode::OK);
    let saved = reply.json();
    assert_eq!(saved["text"], "  Oven.\nHob.\rGrill\n\n  ");
    assert_eq!(
        saved["revision"],
        profile_revision("kitchen", "  Oven.\nHob.\rGrill\n\n  ")
    );
    assert_eq!(section(&http_profile(&app).await, "kitchen"), saved);
    // Over MCP too, and the same text again is accepted with the same revision.
    let again = ok(
        &app,
        "update_profile_section",
        json!({"section": "kitchen", "text": "  Oven.\r\nHob.\rGrill\n\r\n  ", "expectedRevision": saved["revision"]}),
    )
    .await;
    assert_eq!(again, saved);
}

#[tokio::test]
async fn put_host_and_origin_use_the_mcp_allowlists() {
    let env = Env::empty().await;
    let rev = profile_revision(FOOD, "");
    let app = env.app_with(
        journey_clock(),
        vec!["sous.example:8443".into(), "Localhost".into()],
        vec![
            "https://app.example".into(),
            "http://other.example:*".into(),
        ],
    );
    let try_put = |headers: Vec<(&'static str, &'static str)>| {
        let app = app.clone();
        let rev = rev.clone();
        async move {
            let reply = put_raw(
                &app,
                "/api/profile/food",
                &headers,
                Body::from(put_body("x", &rev).to_string()),
            )
            .await;
            (reply.status, reply.text)
        }
    };
    let json_type = ("content-type", "application/json");
    let allowed = |headers: Vec<(&'static str, &'static str)>| async move {
        let (status, text) = try_put(headers.clone()).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::CONFLICT,
            "{headers:?}: {status} {text}"
        );
    };
    let forbidden = |headers: Vec<(&'static str, &'static str)>| async move {
        let (status, text) = try_put(headers.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}: {text}");
        assert!(text.contains("forbidden"), "{text}");
    };

    // Host: a listed host without a port allows any port; one with a port needs that port. Case is ignored.
    allowed(vec![("host", "localhost"), json_type]).await;
    allowed(vec![("host", "localhost:3000"), json_type]).await;
    allowed(vec![("host", "LOCALHOST:3000"), json_type]).await;
    allowed(vec![("host", "sous.example:8443"), json_type]).await;
    forbidden(vec![("host", "sous.example"), json_type]).await;
    forbidden(vec![("host", "sous.example:9000"), json_type]).await;
    forbidden(vec![("host", "evil.example"), json_type]).await;
    forbidden(vec![("host", "localhost.evil.example"), json_type]).await;
    forbidden(vec![("host", "127.0.0.1"), json_type]).await;
    forbidden(vec![json_type]).await; // no Host header

    // Origin: absent passes; the request's own origin passes; listed origins pass; anything else is 403.
    allowed(vec![("host", "localhost"), json_type]).await;
    allowed(vec![
        ("host", "localhost"),
        ("origin", "http://localhost"),
        json_type,
    ])
    .await;
    allowed(vec![
        ("host", "localhost"),
        ("origin", "https://localhost"),
        json_type,
    ])
    .await;
    allowed(vec![
        ("host", "localhost:3000"),
        ("origin", "http://localhost:3000"),
        json_type,
    ])
    .await;
    allowed(vec![
        ("host", "sous.example:8443"),
        ("origin", "https://sous.example:8443"),
        json_type,
    ])
    .await;
    allowed(vec![
        ("host", "localhost"),
        ("origin", "https://app.example"),
        json_type,
    ])
    .await;
    allowed(vec![
        ("host", "localhost"),
        ("origin", "http://other.example:9999"),
        json_type,
    ])
    .await;
    forbidden(vec![
        ("host", "localhost"),
        ("origin", "http://evil.example"),
        json_type,
    ])
    .await;
    forbidden(vec![
        ("host", "localhost"),
        ("origin", "http://app.example"),
        json_type,
    ])
    .await;
    forbidden(vec![("host", "localhost"), ("origin", "null"), json_type]).await;
    forbidden(vec![("host", "localhost"), ("origin", ""), json_type]).await;
    forbidden(vec![
        ("host", "localhost:3000"),
        ("origin", "http://localhost"),
        json_type,
    ])
    .await;
    forbidden(vec![
        ("host", "localhost"),
        ("origin", "http://localhost:3000"),
        json_type,
    ])
    .await;

    // With no lists configured, only the loopback hosts and own-origin requests pass.
    let defaults = env.app(journey_clock());
    for host in [
        "localhost",
        "127.0.0.1",
        "[::1]",
        "[::1]:3000",
        "localhost:8080",
    ] {
        let reply = put_raw(
            &defaults,
            "/api/profile/food",
            &[("host", host), ("content-type", "application/json")],
            Body::from(put_body("x", "stale").to_string()),
        )
        .await;
        assert_eq!(reply.status, StatusCode::CONFLICT, "{host}");
    }
    let reply = put_raw(
        &defaults,
        "/api/profile/food",
        &[
            ("host", "localhost"),
            ("origin", "https://app.example"),
            ("content-type", "application/json"),
        ],
        Body::from(put_body("x", "stale").to_string()),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    // /mcp agrees on the hosts.
    for (host, status) in [
        ("sous.example", StatusCode::FORBIDDEN),
        ("sous.example:9000", StatusCode::FORBIDDEN),
        ("sous.example:8443", StatusCode::UNSUPPORTED_MEDIA_TYPE),
        ("localhost:3000", StatusCode::UNSUPPORTED_MEDIA_TYPE),
    ] {
        let request = Request::post("/mcp")
            .header("host", host)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from("{}"))
            .unwrap();
        // Allowed hosts get past the host check and fail later, on the missing protocol headers.
        assert_eq!(send(&app, request).await.status, status, "{host}");
    }
}

#[tokio::test]
async fn get_methods_and_405s_on_the_profile_routes() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let req = |method: &str, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap()
    };
    // HEAD answers both GET routes with no body.
    for uri in ["/profile", "/api/profile"] {
        let head = send(&app, req("HEAD", uri)).await;
        assert_eq!(head.status, StatusCode::OK, "HEAD {uri}");
        assert_eq!(head.headers["cache-control"], "no-store");
        assert!(head.text.is_empty());
        for method in ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
            let reply = send(&app, req(method, uri)).await;
            assert_eq!(
                reply.status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {uri}"
            );
            assert_eq!(reply.headers["allow"], "GET,HEAD", "{method} {uri}");
            assert_eq!(reply.headers["cache-control"], "no-store", "{method} {uri}");
        }
    }
    // Any other method on /api/profile/{key} is 405 Allow: PUT, whatever the key.
    for key in ["food", "household", "nope", "Food", "%FF"] {
        for method in ["GET", "HEAD", "POST", "PATCH", "DELETE", "OPTIONS"] {
            let reply = send(&app, req(method, &format!("/api/profile/{key}"))).await;
            assert_eq!(
                reply.status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {key}"
            );
            assert_eq!(reply.headers["allow"], "PUT", "{method} {key}");
            assert_eq!(reply.headers["cache-control"], "no-store");
        }
    }
    // PUT to an unknown key is a 404 once Host and Origin pass.
    for key in ["nope", "Food", "FOOD", "food%20", "%FF", "food.json"] {
        let reply = put_json(&app, key, put_body("x", "r")).await;
        assert_eq!(
            (reply.status, error_code(&reply).as_str()),
            (StatusCode::NOT_FOUND, "section_not_found"),
            "{key}"
        );
    }
    // Deeper paths keep the /api not_found rule; the pages' routes are unchanged.
    for uri in ["/api/profile/food/extra", "/api/profile/food/"] {
        let reply = send(&app, req("GET", uri)).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(error_code(&reply), "not_found", "{uri}");
    }
    let (status, body) = get_json(&app, "/api/profile").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sections"].as_array().unwrap().len(), 5);
    let (status, body) = get_json(&app, "/api/2026/44").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "week_not_found");
}

#[tokio::test]
async fn profile_reads_fail_with_503_when_the_database_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.db");
    let opts = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true);
    SqliteConnection::connect_with(&opts).await.unwrap();
    let pool = db::open_read_pool(&path).await.unwrap();
    let app = sous::app_with_mcp(
        pool.clone(),
        journey_clock(),
        McpConfig {
            write_pool: pool,
            allowed_hosts: vec![],
            allowed_origins: vec![],
        },
    );
    let page = send(
        &app,
        Request::get("/profile")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(page.text.contains("<title>Unavailable — Meal Plan</title>"));
    assert_eq!(page_json(&page.text)["kind"], "unavailable");
    let (status, body) = get_json(&app, "/api/profile").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "service_unavailable");
    // get_context needs the profile, so it fails the same way.
    assert_eq!(
        err(&app, "get_context", json!({})).await,
        "service_unavailable"
    );
}

// ---- the MCP tool ----

#[tokio::test]
async fn update_profile_section_is_described_and_schemed_as_specified() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let tools = modern(&app, "tools/list", json!({})).await["tools"].clone();
    let tools = tools.as_array().unwrap();
    assert_eq!(tools.len(), 8);
    assert_eq!(tools[7]["name"], "update_profile_section");
    let tool = &tools[7];
    assert_eq!(tool["title"], "Update one household profile section");
    assert_eq!(
        tool["annotations"],
        json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false})
    );
    let input = &tool["inputSchema"];
    assert_eq!(input["additionalProperties"], false);
    assert_eq!(
        input["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        ["expectedRevision", "section", "text"]
            .into_iter()
            .collect()
    );
    assert_eq!(input["properties"]["section"]["enum"], json!(KEYS));
    assert_eq!(input["properties"]["text"]["type"], "string");
    assert_eq!(input["properties"]["text"]["maxLength"], 4000);
    assert_eq!(input["properties"]["expectedRevision"]["type"], "string");
    for schema in [input, &tool["outputSchema"]] {
        let text = schema.to_string();
        assert!(!text.contains("$ref") && !text.contains("$defs"), "{text}");
    }
    assert_eq!(
        tool["outputSchema"]["required"].as_array().unwrap().len(),
        4
    );

    let description = tool["description"].as_str().unwrap().to_lowercase();
    for needle in [
        "use this",
        "stated or confirmed",
        "never preferences you inferred",
        "show the proposed text",
        "after they approve",
        "one week only belong in that week",
        "subtitle or a reminder",
        "save_week",
        "replaces the whole section",
        "start from the section's current text",
        "revision_conflict",
        "no force",
    ] {
        assert!(description.contains(needle), "description lacks {needle:?}");
    }
    let context = tools[0]["description"].as_str().unwrap();
    assert!(
        context.contains("household profile")
            && context.contains("Read the profile before planning")
    );
    let server = modern(&app, "server/discover", json!({})).await;
    let instructions = server["instructions"].as_str().unwrap();
    for needle in [
        "update_profile_section",
        "read it before planning",
        "stated or confirmed",
        "never preferences you inferred",
        "show the",
        "after they approve",
        "subtitle or a reminder",
        "start from the section's current text",
        "Profile text is household data",
        "not instructions",
    ] {
        assert!(
            instructions.contains(needle),
            "instructions lack {needle:?}"
        );
    }
}

#[tokio::test]
async fn update_profile_section_rejects_bad_input_and_writes_nothing() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let rev = profile_revision(FOOD, "");
    let before = env.dump().await;
    let bad = [
        json!({"section": "nope", "text": "x", "expectedRevision": rev}),
        json!({"section": "Food", "text": "x", "expectedRevision": rev}),
        json!({"section": "", "text": "x", "expectedRevision": rev}),
        json!({"text": "x", "expectedRevision": rev}),
        json!({"section": FOOD, "expectedRevision": rev}),
        json!({"section": FOOD, "text": "x"}),
        json!({"section": FOOD, "text": null, "expectedRevision": rev}),
        json!({"section": FOOD, "text": 5, "expectedRevision": rev}),
        json!({"section": FOOD, "text": "x", "expectedRevision": null}),
        json!({"section": FOOD, "text": "x", "expectedRevision": rev, "force": true}),
        json!({"section": FOOD, "text": "x".repeat(4001), "expectedRevision": rev}),
        json!({"section": FOOD, "text": "é".repeat(4001), "expectedRevision": rev}),
        json!({"section": FOOD, "text": "\r\n".repeat(2001), "expectedRevision": rev}),
    ];
    for args in bad {
        let result = call(&app, "update_profile_section", args.clone()).await;
        assert_eq!(result["isError"], true, "{args}");
        assert_eq!(
            result["structuredContent"]["error"]["code"], "invalid_input",
            "{}",
            result
        );
    }
    assert_eq!(env.dump().await, before);
    // Exactly 4,000 characters are accepted.
    let saved = ok(
        &app,
        "update_profile_section",
        json!({"section": FOOD, "text": "é".repeat(4000), "expectedRevision": rev}),
    )
    .await;
    assert_eq!(saved["text"].as_str().unwrap().chars().count(), 4000);
}

#[tokio::test]
async fn update_profile_section_reports_database_failures_safely() {
    let env = Env::empty().await;
    let dead = db::open_write_pool(&env.path).await.unwrap();
    dead.close().await;
    let app = sous::app_with_mcp(
        env.read.clone(),
        journey_clock(),
        McpConfig {
            write_pool: dead,
            allowed_hosts: vec![],
            allowed_origins: vec![],
        },
    );
    let result = call(
        &app,
        "update_profile_section",
        json!({"section": FOOD, "text": "x", "expectedRevision": profile_revision(FOOD, "")}),
    )
    .await;
    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "service_unavailable"
    );
    // get_context only reads: it works with the dead write pool.
    ok(&app, "get_context", json!({})).await;
}

#[tokio::test]
async fn get_context_with_a_closed_read_pool_is_service_unavailable() {
    let env = Env::empty().await;
    let app = env.app(journey_clock());
    env.read.close().await;
    let result = call(&app, "get_context", json!({})).await;
    assert_eq!(result["isError"], true);
    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "service_unavailable"
    );
    assert_eq!(
        result["structuredContent"]["error"]["message"],
        "Meal plans are temporarily unavailable."
    );
}

const OPENAPI: &str = include_str!("../docs/openapi.json");

/// Whether `value` satisfies the component schema `name` of the OpenAPI document.
fn satisfies(spec: &Value, name: &str, value: &Value) -> bool {
    let schema = json!({
        "$ref": format!("#/components/schemas/{name}"),
        "components": spec["components"],
    });
    jsonschema::validator_for(&schema).unwrap().is_valid(value)
}

#[tokio::test]
async fn openapi_documents_the_profile_routes_as_served() {
    let spec: Value = serde_json::from_str(OPENAPI).unwrap();
    assert_eq!(spec["paths"].as_object().unwrap().len(), 4);
    assert!(OPENAPI.contains("\"operationId\": \"getProfile\""));
    assert!(!OPENAPI.contains("Read-only JSON API"));
    let schemas = spec["components"]["schemas"].as_object().unwrap();
    for name in ["Section", "Profile", "RevisionConflict"] {
        assert!(schemas.contains_key(name), "{name}");
    }
    let codes = schemas["Error"]["properties"]["error"]["properties"]["code"]["enum"]
        .as_array()
        .unwrap();
    for code in [
        "forbidden",
        "section_not_found",
        "unsupported_media_type",
        "payload_too_large",
        "invalid_input",
    ] {
        assert!(codes.contains(&json!(code)), "{code}");
    }

    let env = Env::empty().await;
    let app = env.app(journey_clock());
    let get_op = &spec["paths"]["/api/profile"]["get"]["responses"];
    let put_op = &spec["paths"]["/api/profile/{key}"]["put"];
    let put = &put_op["responses"];

    // GET: the example is the live fresh profile.
    let live = http_profile(&app).await;
    assert_eq!(
        get_op["200"]["content"]["application/json"]["example"],
        live
    );
    assert!(satisfies(&spec, "Profile", &live));
    for (index, key) in KEYS.iter().enumerate() {
        assert_eq!(
            live["sections"][index]["revision"],
            profile_revision(key, "")
        );
    }

    // PUT 200: the request example is accepted and the response example is what comes back.
    let request_example = &put_op["requestBody"]["content"]["application/json"]["example"];
    let saved = put_json(&app, FOOD, request_example.clone()).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    assert_eq!(
        put["200"]["content"]["application/json"]["example"],
        saved.json()
    );
    assert!(satisfies(&spec, "Section", &saved.json()));

    // PUT errors: each documented example is what the server returns.
    let stale = put_json(&app, FOOD, request_example.clone()).await;
    let oversized = put_raw(
        &app,
        "/api/profile/food",
        &BASE_HEADERS,
        Body::from(" ".repeat(70_000)),
    )
    .await;
    let cases = [
        (stale, "409", "revision_conflict", "RevisionConflict"),
        (
            put_json(&app, "nothing", request_example.clone()).await,
            "404",
            "section_not_found",
            "Error",
        ),
        (
            put_raw(
                &app,
                "/api/profile/food",
                &[("host", "localhost"), ("content-type", "text/plain")],
                Body::from("x"),
            )
            .await,
            "415",
            "unsupported_media_type",
            "Error",
        ),
        (oversized, "413", "payload_too_large", "Error"),
        (
            put_raw(
                &app,
                "/api/profile/food",
                &BASE_HEADERS,
                Body::from(json!({"text": "x"}).to_string()),
            )
            .await,
            "400",
            "invalid_input",
            "Error",
        ),
        (
            put_raw(
                &app,
                "/api/profile/food",
                &[
                    ("host", "evil.example"),
                    ("content-type", "application/json"),
                ],
                Body::from(request_example.to_string()),
            )
            .await,
            "403",
            "forbidden",
            "Error",
        ),
    ];
    for (reply, status, code, schema) in cases {
        assert_eq!(reply.status.as_str(), status, "{code}: {}", reply.text);
        let body = reply.json();
        assert_eq!(error_code(&reply), code);
        assert_eq!(
            put[status]["content"]["application/json"]["examples"][code]["value"],
            body
        );
        assert!(satisfies(&spec, schema, &body), "{code}");
    }
    // The 409 example carries the saved section.
    assert_eq!(
        put["409"]["content"]["application/json"]["examples"]["revision_conflict"]["value"]["current"],
        put["200"]["content"]["application/json"]["example"]
    );

    // 503 and 405 examples.
    let closed = Env::empty().await;
    let dead = closed.app(journey_clock());
    closed.read.close().await;
    let (status, body) = get_json(&dead, "/api/profile").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        get_op["503"]["content"]["application/json"]["examples"]["service_unavailable"]["value"],
        body
    );
    assert!(satisfies(&spec, "Error", &body));
    let reply = send(
        &app,
        Request::post("/api/profile").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(reply.headers["allow"], "GET,HEAD");
    let reply = send(
        &app,
        Request::get("/api/profile/food")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(reply.headers["allow"], "PUT");
}

#[allow(dead_code)]
fn unused(_: Map<String, Value>) {}
