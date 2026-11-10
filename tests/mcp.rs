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
const TOOLS_DOC: &str = include_str!("../docs/mcp-tools.json");
const SAVE_EXAMPLE: &str = include_str!("../docs/examples/save-week.json");

const CASSEROLE_DINNER: &str = "000000c3-0000-4000-8000-000000000001";
const CASSEROLE_LEFTOVERS: &str = "000000c3-0000-4000-8000-000000000002";
const TOOL_NAMES: [&str; 7] = [
    "get_context",
    "list_weeks",
    "get_week",
    "search_meals",
    "get_meal",
    "get_shopping_lists",
    "save_week",
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
            {"id": id(60), "title": "Sunday prep", "covers": "Mon–Tue", "tasks": [
                {"id": id(61), "name": "Cook lentils", "time": "20 min", "instructions": "Boil and drain."},
            ]},
        ],
        "strategyCards": [
            {"id": id(70), "title": "Batch cooking", "icon": "🍲", "theme": "green", "items": [
                {"id": id(71), "text": "Double the soup."},
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
async fn stateless_flow_lists_seven_tools_and_calls_them() {
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
        let writes = name == "save_week";
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
    assert_eq!(tools.as_array().unwrap().len(), 7);

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
    assert_eq!(api["strategyCards"][0]["theme"], "green");
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
    assert_eq!(tools.len(), 7);
    legacy.cancel().await.ok();
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
    assert!(!example["strategyCards"].as_array().unwrap().is_empty());
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

#[allow(dead_code)]
fn unused(_: Map<String, Value>) {}
