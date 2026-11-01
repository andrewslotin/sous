//! Application queries.
use crate::calendar::{iso_week_of, week_dates, week_path};
use crate::model::{
    Day, Meal, MealHit, MealPage, MealRecord, PrepSession, PrepTask, Reminder, RevisedWeek,
    ShoppingCategory, ShoppingDay, ShoppingItem, ShoppingList, Step, StoredWeek, StrategyCard,
    StrategyItem, Week, WeekDocument, WeekPage, WeekSummary, YearListing,
};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, SqlitePool};

/// Failure of an MCP-facing query or save. `code()` is the stable application error code.
#[derive(Debug)]
pub enum QueryError {
    /// Bad arguments or document; the message is safe to show and names the problem.
    InvalidInput(String),
    WeekNotFound,
    MealNotFound,
    RevisionConflict,
    /// Logged server-side; never shown.
    Database(sqlx::Error),
}

impl From<sqlx::Error> for QueryError {
    fn from(error: sqlx::Error) -> Self {
        QueryError::Database(error)
    }
}

impl QueryError {
    /// `invalid_input`, `week_not_found`, `meal_not_found`, `revision_conflict` or `service_unavailable`.
    pub fn code(&self) -> &'static str {
        match self {
            QueryError::InvalidInput(_) => "invalid_input",
            QueryError::WeekNotFound => "week_not_found",
            QueryError::MealNotFound => "meal_not_found",
            QueryError::RevisionConflict => "revision_conflict",
            QueryError::Database(_) => "service_unavailable",
        }
    }

    /// Safe message: no SQL, paths or internals.
    pub fn message(&self) -> String {
        match self {
            QueryError::InvalidInput(message) => message.clone(),
            QueryError::WeekNotFound => "No meal plan for this week.".into(),
            QueryError::MealNotFound => "No meal with this id.".into(),
            QueryError::RevisionConflict => "The week changed since it was read, or already exists. Fetch it with get_week and resubmit.".into(),
            QueryError::Database(_) => "Meal plans are temporarily unavailable.".into(),
        }
    }
}

fn invalid<T>(message: impl Into<String>) -> Result<T, QueryError> {
    Err(QueryError::InvalidInput(message.into()))
}

/// True for a canonical lowercase UUID text (`8-4-4-4-12` hex digits), the rule of the id `GLOB` checks.
pub(crate) fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

fn date_text(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Paging arguments shared by collection queries.
#[derive(Debug, Default, Clone)]
pub struct PageRequest {
    /// Default 20, 1..=100 else `InvalidInput`.
    pub limit: Option<u32>,
    /// Opaque cursor from a previous page with the same filters.
    pub cursor: Option<String>,
}

/// `load_week` on a caller-provided connection/transaction (no transaction of its own).
pub async fn load_week_in(
    conn: &mut SqliteConnection,
    iso_year: i32,
    iso_week: u32,
) -> Result<Option<WeekDocument>, sqlx::Error> {
    load_week_paused(conn, iso_year, iso_week, std::future::ready(())).await
}

/// Every table that holds a week's content, with the rows that belong to week `?1` (a `weeks.id`).
const WEEK_ROWS: [(&str, &str); 12] = [
    ("weeks", "SELECT * FROM weeks WHERE id = ?1"),
    ("days", "SELECT * FROM days WHERE week_id = ?1"),
    (
        "meals",
        "SELECT * FROM meals WHERE day_id IN (SELECT id FROM days WHERE week_id = ?1)",
    ),
    (
        "meal_steps",
        "SELECT * FROM meal_steps WHERE meal_id IN (SELECT m.id FROM meals m \
         JOIN days d ON d.id = m.day_id WHERE d.week_id = ?1)",
    ),
    (
        "shopping_lists",
        "SELECT * FROM shopping_lists WHERE week_id = ?1",
    ),
    (
        "day_shopping_lists",
        "SELECT * FROM day_shopping_lists WHERE day_id IN (SELECT id FROM days WHERE week_id = ?1)",
    ),
    (
        "shopping_categories",
        "SELECT * FROM shopping_categories WHERE shopping_list_id IN \
         (SELECT id FROM shopping_lists WHERE week_id = ?1)",
    ),
    (
        "shopping_items",
        "SELECT * FROM shopping_items WHERE category_id IN (SELECT c.id FROM shopping_categories c \
         JOIN shopping_lists l ON l.id = c.shopping_list_id WHERE l.week_id = ?1)",
    ),
    (
        "prep_sessions",
        "SELECT * FROM prep_sessions WHERE week_id = ?1",
    ),
    (
        "prep_tasks",
        "SELECT * FROM prep_tasks WHERE session_id IN (SELECT id FROM prep_sessions WHERE week_id = ?1)",
    ),
    (
        "strategy_cards",
        "SELECT * FROM strategy_cards WHERE week_id = ?1",
    ),
    (
        "strategy_items",
        "SELECT * FROM strategy_items WHERE card_id IN (SELECT id FROM strategy_cards WHERE week_id = ?1)",
    ),
];

fn hash_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Opaque revision token: SHA-256 over every column of every persisted row of the week, so any
/// change to the stored data (including values the document hides) changes it. Rows are ordered by
/// their leading key columns and each value is tagged with its SQLite type. Call it inside the
/// transaction that reads the document.
pub async fn week_revision(
    conn: &mut SqliteConnection,
    week_id: &str,
) -> Result<String, sqlx::Error> {
    use sqlx::{TypeInfo, ValueRef};
    use std::fmt::Write;
    let mut hasher = Sha256::new();
    for (table, select) in WEEK_ROWS {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("{select} ORDER BY 1, 2")))
            .bind(week_id)
            .fetch_all(&mut *conn)
            .await?;
        hash_bytes(&mut hasher, table.as_bytes());
        hasher.update((rows.len() as u64).to_le_bytes());
        for row in &rows {
            for index in 0..row.len() {
                let value = row.try_get_raw(index)?;
                if value.is_null() {
                    hasher.update(b"N");
                    continue;
                }
                match value.type_info().name() {
                    "INTEGER" => {
                        hasher.update(b"I");
                        hasher.update(row.try_get::<i64, _>(index)?.to_le_bytes());
                    }
                    "REAL" => {
                        hasher.update(b"R");
                        hasher.update(row.try_get::<f64, _>(index)?.to_bits().to_le_bytes());
                    }
                    "BLOB" => {
                        hasher.update(b"B");
                        hash_bytes(&mut hasher, &row.try_get::<Vec<u8>, _>(index)?);
                    }
                    _ => {
                        hasher.update(b"T");
                        hash_bytes(&mut hasher, row.try_get::<String, _>(index)?.as_bytes());
                    }
                }
            }
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        }))
}

/// One week with its revision, in one read snapshot. Unrepresentable week: `InvalidInput`; no plan: `WeekNotFound`.
pub async fn get_week(
    pool: &SqlitePool,
    iso_year: i32,
    iso_week: u32,
) -> Result<RevisedWeek, QueryError> {
    if week_dates(iso_year, iso_week).is_none() {
        return invalid("isoYear and isoWeek are not a valid ISO week");
    }
    let mut tx = pool.begin().await?;
    let Some(document) = load_week_in(&mut tx, iso_year, iso_week).await? else {
        return Err(QueryError::WeekNotFound);
    };
    let revision = week_revision(&mut tx, &document.week.id).await?;
    tx.commit().await?;
    Ok(RevisedWeek { revision, document })
}

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;

fn page_limit(page: &PageRequest) -> Result<usize, QueryError> {
    match page.limit {
        None => Ok(DEFAULT_LIMIT as usize),
        Some(limit) if (1..=MAX_LIMIT).contains(&limit) => Ok(limit as usize),
        Some(_) => invalid("limit must be between 1 and 100"),
    }
}

fn check_range(from: Option<NaiveDate>, to: Option<NaiveDate>) -> Result<(), QueryError> {
    match (from, to) {
        (Some(from), Some(to)) if from > to => invalid("fromDate must not be after toDate"),
        _ => Ok(()),
    }
}

/// The part of a cursor that must match the current call.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Filters {
    tool: String,
    from: Option<String>,
    to: Option<String>,
    query: Option<String>,
}

impl Filters {
    fn new(
        tool: &str,
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
        query: Option<&str>,
    ) -> Self {
        Filters {
            tool: tool.to_string(),
            from: from.map(date_text),
            to: to.map(date_text),
            query: query.map(str::to_string),
        }
    }
}

/// Opaque cursor: lowercase hex of this JSON object.
#[derive(Serialize, Deserialize)]
struct Cursor<K> {
    #[serde(flatten)]
    filters: Filters,
    last: K,
}

fn encode_cursor<K: Serialize>(filters: Filters, last: K) -> String {
    use std::fmt::Write;
    let json = serde_json::to_vec(&Cursor { filters, last }).expect("cursor serializes");
    json.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

fn decode_cursor<K: serde::de::DeserializeOwned>(
    text: &str,
    expected: &Filters,
) -> Result<K, QueryError> {
    const BAD: &str = "cursor is not valid for this request";
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let raw = text.as_bytes();
    if raw.is_empty() || !raw.len().is_multiple_of(2) {
        return invalid(BAD);
    }
    let mut json = Vec::with_capacity(raw.len() / 2);
    for pair in raw.chunks(2) {
        match (digit(pair[0]), digit(pair[1])) {
            (Some(high), Some(low)) => json.push(high << 4 | low),
            _ => return invalid(BAD),
        }
    }
    match serde_json::from_slice::<Cursor<K>>(&json) {
        Ok(cursor) if cursor.filters == *expected => Ok(cursor.last),
        _ => invalid(BAD),
    }
}

/// Stored weeks overlapping the inclusive date range, newest first, paged.
pub async fn list_weeks_page(
    pool: &SqlitePool,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    page: PageRequest,
) -> Result<WeekPage, QueryError> {
    check_range(from, to)?;
    let limit = page_limit(&page)?;
    let filters = Filters::new("list_weeks", from, to, None);
    let after: (i32, u32) = match &page.cursor {
        Some(cursor) => decode_cursor(cursor, &filters)?,
        None => (10_000, 0),
    };
    let lower = from.map_or((0, 0), iso_week_of);
    let upper = to.map_or((9_999, 99), iso_week_of);

    let rows = sqlx::query(
        "SELECT id, iso_year, iso_week FROM weeks \
         WHERE (iso_year, iso_week) >= (?1, ?2) AND (iso_year, iso_week) <= (?3, ?4) \
           AND (iso_year, iso_week) < (?5, ?6) \
         ORDER BY iso_year DESC, iso_week DESC LIMIT ?7",
    )
    .bind(lower.0)
    .bind(lower.1)
    .bind(upper.0)
    .bind(upper.1)
    .bind(after.0)
    .bind(after.1)
    .bind(limit as i64 + 1)
    .fetch_all(pool)
    .await?;

    let has_more = rows.len() > limit;
    let mut weeks = Vec::with_capacity(rows.len().min(limit));
    for row in rows.into_iter().take(limit) {
        let iso_year: i32 = row.get("iso_year");
        let iso_week: u32 = row.get("iso_week");
        match week_summary(iso_year, iso_week) {
            Some(summary) => weeks.push(StoredWeek {
                id: row.get("id"),
                iso_year,
                iso_week,
                start_date: summary.start_date,
                end_date: summary.end_date,
                path: summary.path,
            }),
            None => tracing::warn!(
                iso_year,
                iso_week,
                "skipping stored week with unrepresentable dates"
            ),
        }
    }
    let next_cursor = match weeks.last() {
        Some(last) if has_more => Some(encode_cursor(filters, (last.iso_year, last.iso_week))),
        _ => None,
    };
    Ok(WeekPage { weeks, next_cursor })
}

/// Position of a meal in the search order.
type MealKey = (i32, u32, u32, i64, String);

const MEAL_BATCH: i64 = 200;

fn meal_bound(date: NaiveDate) -> (i32, u32, u32) {
    let (year, week) = iso_week_of(date);
    (year, week, date.weekday().number_from_monday())
}

/// Literal, Unicode case-insensitive substring search over meal names and descriptions.
pub async fn search_meals(
    pool: &SqlitePool,
    query: Option<&str>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    page: PageRequest,
) -> Result<MealPage, QueryError> {
    check_range(from, to)?;
    let limit = page_limit(&page)?;
    let query = query.filter(|q| !q.is_empty());
    if query.is_none() && from.is_none() && to.is_none() {
        return invalid("query, fromDate or toDate is required");
    }
    let filters = Filters::new("search_meals", from, to, query);
    let mut after: MealKey = match &page.cursor {
        Some(cursor) => decode_cursor(cursor, &filters)?,
        None => (10_000, 0, 0, -1, String::new()),
    };
    let needle = query.map(caseless::default_case_fold_str);
    let lower = from.map_or((0, 0, 0), meal_bound);
    let upper = to.map_or((9_999, 99, 9), meal_bound);

    let mut tx = pool.begin().await?;
    let mut found: Vec<(MealHit, MealKey)> = Vec::new();
    'scan: loop {
        let rows = sqlx::query(
            "SELECT m.id, m.name, m.description, m.type, m.position, \
                    w.iso_year, w.iso_week, d.weekday \
             FROM meals m JOIN days d ON d.id = m.day_id JOIN weeks w ON w.id = d.week_id \
             WHERE (w.iso_year, w.iso_week, d.weekday) >= (?1, ?2, ?3) \
               AND (w.iso_year, w.iso_week, d.weekday) <= (?4, ?5, ?6) \
               AND ((w.iso_year, w.iso_week, d.weekday) < (?7, ?8, ?9) \
                    OR ((w.iso_year, w.iso_week, d.weekday) = (?7, ?8, ?9) \
                        AND (m.position > ?10 OR (m.position = ?10 AND m.id > ?11)))) \
             ORDER BY w.iso_year DESC, w.iso_week DESC, d.weekday DESC, m.position ASC, m.id ASC \
             LIMIT ?12",
        )
        .bind(lower.0)
        .bind(lower.1)
        .bind(lower.2)
        .bind(upper.0)
        .bind(upper.1)
        .bind(upper.2)
        .bind(after.0)
        .bind(after.1)
        .bind(after.2)
        .bind(after.3)
        .bind(&after.4)
        .bind(MEAL_BATCH)
        .fetch_all(&mut *tx)
        .await?;
        let fetched = rows.len() as i64;
        for row in rows {
            let key: MealKey = (
                row.get("iso_year"),
                row.get("iso_week"),
                row.get("weekday"),
                row.get("position"),
                row.get("id"),
            );
            after = key.clone();
            let name: String = row.get("name");
            let description: String = row.get("description");
            if let Some(needle) = &needle
                && !caseless::default_case_fold_str(&name).contains(needle)
                && !caseless::default_case_fold_str(&description).contains(needle)
            {
                continue;
            }
            let Some(dates) = week_dates(key.0, key.1) else {
                continue;
            };
            found.push((
                MealHit {
                    id: key.4.clone(),
                    name,
                    description,
                    kind: row.get("type"),
                    date: date_text(dates[key.2 as usize - 1]),
                    iso_year: key.0,
                    iso_week: key.1,
                    week_path: week_path(key.0, key.1),
                },
                key,
            ));
            if found.len() > limit {
                break 'scan;
            }
        }
        if fetched < MEAL_BATCH {
            break;
        }
    }
    tx.commit().await?;

    let has_more = found.len() > limit;
    found.truncate(limit);
    let next_cursor = match found.last() {
        Some((_, key)) if has_more => Some(encode_cursor(filters, key.clone())),
        _ => None,
    };
    Ok(MealPage {
        meals: found.into_iter().map(|(hit, _)| hit).collect(),
        next_cursor,
    })
}

/// One saved meal with its date and week, in one read snapshot.
pub async fn get_meal(pool: &SqlitePool, meal_id: &str) -> Result<MealRecord, QueryError> {
    if !is_canonical_uuid(meal_id) {
        return invalid("mealId must be a lowercase UUID");
    }
    let mut tx = pool.begin().await?;
    let Some(location) = sqlx::query(
        "SELECT w.iso_year, w.iso_week, d.weekday FROM meals m \
         JOIN days d ON d.id = m.day_id JOIN weeks w ON w.id = d.week_id WHERE m.id = ?",
    )
    .bind(meal_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(QueryError::MealNotFound);
    };
    let iso_year: i32 = location.get("iso_year");
    let iso_week: u32 = location.get("iso_week");
    let weekday: u32 = location.get("weekday");
    let document = load_week_in(&mut tx, iso_year, iso_week).await?;
    tx.commit().await?;

    let day = document
        .and_then(|document| document.days.into_iter().nth(weekday as usize - 1))
        .ok_or(QueryError::MealNotFound)?;
    let meal = day
        .meals
        .into_iter()
        .find(|meal| meal.id == meal_id)
        .ok_or(QueryError::MealNotFound)?;
    Ok(MealRecord {
        meal,
        date: day.date,
        weekday,
        iso_year,
        iso_week,
        week_path: week_path(iso_year, iso_week),
    })
}

/// Lists assigned to `date`; `timezone` is echoed into the result.
pub async fn shopping_lists_on(
    pool: &SqlitePool,
    date: NaiveDate,
    timezone: &str,
) -> Result<ShoppingDay, QueryError> {
    let (iso_year, iso_week) = iso_week_of(date);
    if week_dates(iso_year, iso_week).is_none() {
        return invalid("date is outside the supported range");
    }
    let mut tx = pool.begin().await?;
    let document = load_week_in(&mut tx, iso_year, iso_week).await?;
    tx.commit().await?;

    let plan_exists = document.is_some();
    let mut lists = Vec::new();
    if let Some(document) = document {
        let weekday = date.weekday().number_from_monday() as usize;
        let mut by_id: std::collections::HashMap<String, ShoppingList> = document
            .shopping_lists
            .into_iter()
            .map(|list| (list.id.clone(), list))
            .collect();
        if let Some(day) = document.days.into_iter().nth(weekday - 1) {
            lists.extend(
                day.shopping_list_ids
                    .iter()
                    .filter_map(|id| by_id.remove(id)),
            );
        }
    }
    Ok(ShoppingDay {
        date: date_text(date),
        timezone: timezone.to_string(),
        plan_exists,
        iso_year,
        iso_week,
        week_path: week_path(iso_year, iso_week),
        lists,
    })
}

/// Load one week inside a single read transaction. `None` if the week row does not exist.
pub async fn load_week(
    pool: &SqlitePool,
    iso_year: i32,
    iso_week: u32,
) -> Result<Option<WeekDocument>, sqlx::Error> {
    load_week_with_pause(pool, iso_year, iso_week, std::future::ready(())).await
}

/// Test seam for snapshot tests: `pause` is awaited after the week row has been read (the read snapshot is
/// open) and before any related rows are read.
#[doc(hidden)]
pub async fn load_week_with_pause(
    pool: &SqlitePool,
    iso_year: i32,
    iso_week: u32,
    pause: impl std::future::Future<Output = ()>,
) -> Result<Option<WeekDocument>, sqlx::Error> {
    if week_dates(iso_year, iso_week).is_none() {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    let document = load_week_paused(&mut tx, iso_year, iso_week, pause).await?;
    tx.commit().await?;
    Ok(document)
}

async fn load_week_paused(
    conn: &mut SqliteConnection,
    iso_year: i32,
    iso_week: u32,
    pause: impl std::future::Future<Output = ()>,
) -> Result<Option<WeekDocument>, sqlx::Error> {
    let Some(dates) = week_dates(iso_year, iso_week) else {
        return Ok(None);
    };

    let Some(week_row) = sqlx::query(
        "SELECT id, subtitle, prep_intro FROM weeks WHERE iso_year = ? AND iso_week = ?",
    )
    .bind(iso_year)
    .bind(iso_week)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let week_id: String = week_row.get("id");
    pause.await;

    let mut days: Vec<Day> = dates
        .iter()
        .enumerate()
        .map(|(i, d)| Day {
            id: None,
            date: d.format("%Y-%m-%d").to_string(),
            weekday: i as u32 + 1,
            meals: Vec::new(),
            shopping_list_ids: Vec::new(),
            reminder: None,
        })
        .collect();

    for row in sqlx::query(
        "SELECT id, weekday, reminder_title, reminder_body, reminder_icon \
         FROM days WHERE week_id = ? ORDER BY weekday",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?
    {
        let weekday: i64 = row.get("weekday");
        let day = &mut days[(weekday - 1) as usize];
        day.id = Some(row.get("id"));
        day.reminder = row
            .get::<Option<String>, _>("reminder_title")
            .map(|title| Reminder {
                title,
                body: row.get("reminder_body"),
                icon: row.get("reminder_icon"),
            });
    }

    let meal_rows = sqlx::query(
        "SELECT m.id, d.weekday, m.type, m.name, m.description, m.kcal, m.protein_g, m.fat_g, \
                m.carbs_g, m.fiber_g, m.for_two, m.assembly_time, m.cook_time, m.tip \
         FROM meals m JOIN days d ON d.id = m.day_id \
         WHERE d.week_id = ? ORDER BY d.weekday, m.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let step_rows = sqlx::query(
        "SELECT s.id, s.meal_id, s.text FROM meal_steps s \
         JOIN meals m ON m.id = s.meal_id JOIN days d ON d.id = m.day_id \
         WHERE d.week_id = ? ORDER BY s.meal_id, s.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let list_rows = sqlx::query(
        "SELECT id, label, note FROM shopping_lists WHERE week_id = ? ORDER BY position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let category_rows = sqlx::query(
        "SELECT c.id, c.shopping_list_id, c.name FROM shopping_categories c \
         JOIN shopping_lists l ON l.id = c.shopping_list_id \
         WHERE l.week_id = ? ORDER BY c.shopping_list_id, c.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let item_rows = sqlx::query(
        "SELECT i.id, i.category_id, i.text FROM shopping_items i \
         JOIN shopping_categories c ON c.id = i.category_id \
         JOIN shopping_lists l ON l.id = c.shopping_list_id \
         WHERE l.week_id = ? ORDER BY i.category_id, i.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let link_rows = sqlx::query(
        "SELECT d.weekday, x.shopping_list_id FROM day_shopping_lists x \
         JOIN days d ON d.id = x.day_id \
         WHERE d.week_id = ? ORDER BY d.weekday, x.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let session_rows = sqlx::query(
        "SELECT id, title, covers FROM prep_sessions WHERE week_id = ? ORDER BY position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let task_rows = sqlx::query(
        "SELECT t.id, t.session_id, t.name, t.time, t.instructions FROM prep_tasks t \
         JOIN prep_sessions s ON s.id = t.session_id \
         WHERE s.week_id = ? ORDER BY t.session_id, t.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let card_rows = sqlx::query(
        "SELECT id, title, icon, theme FROM strategy_cards WHERE week_id = ? ORDER BY position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let card_item_rows = sqlx::query(
        "SELECT i.id, i.card_id, i.text FROM strategy_items i \
         JOIN strategy_cards c ON c.id = i.card_id \
         WHERE c.week_id = ? ORDER BY i.card_id, i.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut tasks: std::collections::HashMap<String, Vec<PrepTask>> = Default::default();
    for row in task_rows {
        tasks
            .entry(row.get("session_id"))
            .or_default()
            .push(PrepTask {
                id: row.get("id"),
                name: row.get("name"),
                time: row.get("time"),
                instructions: row.get("instructions"),
            });
    }
    let prep_sessions: Vec<PrepSession> = session_rows
        .into_iter()
        .map(|row| {
            let id: String = row.get("id");
            PrepSession {
                tasks: tasks.remove(&id).unwrap_or_default(),
                id,
                title: row.get("title"),
                covers: row.get("covers"),
            }
        })
        .collect();

    let mut card_items: std::collections::HashMap<String, Vec<StrategyItem>> = Default::default();
    for row in card_item_rows {
        card_items
            .entry(row.get("card_id"))
            .or_default()
            .push(StrategyItem {
                id: row.get("id"),
                text: row.get("text"),
            });
    }
    let strategy_cards: Vec<StrategyCard> = card_rows
        .into_iter()
        .map(|row| {
            let id: String = row.get("id");
            StrategyCard {
                items: card_items.remove(&id).unwrap_or_default(),
                id,
                title: row.get("title"),
                icon: row.get("icon"),
                theme: row.get("theme"),
            }
        })
        .collect();

    let mut items: std::collections::HashMap<String, Vec<ShoppingItem>> = Default::default();
    for row in item_rows {
        items
            .entry(row.get("category_id"))
            .or_default()
            .push(ShoppingItem {
                id: row.get("id"),
                text: row.get("text"),
            });
    }
    let mut categories: std::collections::HashMap<String, Vec<ShoppingCategory>> =
        Default::default();
    for row in category_rows {
        let id: String = row.get("id");
        categories
            .entry(row.get("shopping_list_id"))
            .or_default()
            .push(ShoppingCategory {
                items: items.remove(&id).unwrap_or_default(),
                id,
                name: row.get("name"),
            });
    }
    let shopping_lists: Vec<ShoppingList> = list_rows
        .into_iter()
        .map(|row| {
            let id: String = row.get("id");
            ShoppingList {
                categories: categories.remove(&id).unwrap_or_default(),
                id,
                label: row.get("label"),
                note: row.get("note"),
            }
        })
        .collect();
    for row in link_rows {
        let weekday: i64 = row.get("weekday");
        days[(weekday - 1) as usize]
            .shopping_list_ids
            .push(row.get("shopping_list_id"));
    }

    let mut steps: std::collections::HashMap<String, Vec<Step>> = Default::default();
    for row in step_rows {
        steps.entry(row.get("meal_id")).or_default().push(Step {
            id: row.get("id"),
            text: row.get("text"),
        });
    }

    for row in meal_rows {
        let weekday: i64 = row.get("weekday");
        let id: String = row.get("id");
        let meal = Meal {
            steps: steps.remove(&id).unwrap_or_default(),
            id,
            kind: row.get("type"),
            name: row.get("name"),
            description: row.get("description"),
            kcal: row.get("kcal"),
            protein_g: row.get("protein_g"),
            fat_g: row.get("fat_g"),
            carbs_g: row.get("carbs_g"),
            fiber_g: row.get("fiber_g"),
            for_two: row.get::<i64, _>("for_two") != 0,
            assembly_time: row.get("assembly_time"),
            cook_time: row.get("cook_time"),
            tip: row.get("tip"),
        };
        days[(weekday - 1) as usize].meals.push(meal);
    }

    Ok(Some(WeekDocument {
        schema_version: crate::model::SCHEMA_VERSION,
        week: Week {
            id: week_id,
            iso_year,
            iso_week,
            start_date: days[0].date.clone(),
            end_date: days[6].date.clone(),
            subtitle: week_row.get("subtitle"),
            prep_intro: week_row.get("prep_intro"),
        },
        days,
        shopping_lists,
        prep_sessions,
        strategy_cards,
    }))
}

/// Summary (dates and canonical path) of an ISO week, or `None` if it is not representable.
pub fn week_summary(iso_year: i32, iso_week: u32) -> Option<WeekSummary> {
    let dates = week_dates(iso_year, iso_week)?;
    Some(WeekSummary {
        iso_year,
        iso_week,
        start_date: dates[0].format("%Y-%m-%d").to_string(),
        end_date: dates[6].format("%Y-%m-%d").to_string(),
        path: week_path(iso_year, iso_week),
    })
}

/// Stored weeks of `iso_year` (newest first) and its adjacent years with plans, from one statement.
pub async fn load_year(pool: &SqlitePool, iso_year: i32) -> Result<YearListing, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT iso_year, iso_week FROM weeks WHERE iso_year BETWEEN ?1 - 1 AND ?1 + 1 \
         ORDER BY iso_year DESC, iso_week DESC",
    )
    .bind(iso_year)
    .fetch_all(pool)
    .await?;
    let mut listing = YearListing {
        weeks: Vec::new(),
        previous_year: None,
        next_year: None,
    };
    for row in rows {
        let year: i32 = row.get("iso_year");
        let iso_week: u32 = row.get("iso_week");
        if year == iso_year - 1 {
            listing.previous_year = Some(year);
        } else if year == iso_year + 1 {
            listing.next_year = Some(year);
        } else {
            match week_summary(year, iso_week) {
                Some(summary) => listing.weeks.push(summary),
                None => tracing::warn!(
                    iso_year = year,
                    iso_week,
                    "skipping stored week with unrepresentable dates"
                ),
            }
        }
    }
    Ok(listing)
}
