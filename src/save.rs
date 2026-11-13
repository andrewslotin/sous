//! Transactional full-week replacement for the MCP `save_week` tool.
use crate::calendar::week_dates;
use crate::model::{DayInput, MealType, RevisedWeek, WeekInput};
use crate::queries::{Conflict, QueryError, is_canonical_uuid, load_week_in, week_revision};
use sqlx::{SqliteConnection, SqlitePool};
use std::collections::HashSet;

/// Validate and save one complete week in a single write transaction.
/// `expected_revision`: `None` creates only if the week does not exist (else `AlreadySaved`); `Some`
/// replaces only if it matches the current revision (`Stale` on mismatch, `NotSaved` if the week does
/// not exist). Any string, including `"null"`, is an ordinary revision. Returns the committed document and its new revision.
pub async fn save_week(
    write_pool: &SqlitePool,
    iso_year: i32,
    iso_week: u32,
    expected_revision: Option<&str>,
    input: WeekInput,
) -> Result<RevisedWeek, QueryError> {
    save_week_with_pause(
        write_pool,
        iso_year,
        iso_week,
        expected_revision,
        input,
        std::future::ready(()),
    )
    .await
}

/// Test seam: `pause` is awaited inside the write transaction after the revision check and the
/// deletion of the old children, before any new rows are inserted.
#[doc(hidden)]
pub async fn save_week_with_pause(
    write_pool: &SqlitePool,
    iso_year: i32,
    iso_week: u32,
    expected_revision: Option<&str>,
    input: WeekInput,
    pause: impl std::future::Future<Output = ()>,
) -> Result<RevisedWeek, QueryError> {
    let ids = validate(iso_year, iso_week, &input)?;

    // BEGIN IMMEDIATE takes the write lock before reading, so the revision check is atomic with the write.
    let mut tx = write_pool.begin_with("BEGIN IMMEDIATE").await?;
    let outcome = replace_week(
        &mut tx,
        iso_year,
        iso_week,
        expected_revision,
        &input,
        &ids,
        pause,
    )
    .await;
    match outcome {
        Ok(saved) => {
            tx.commit().await?;
            Ok(saved)
        }
        Err(error) => {
            // Roll back now so the write lock is released before the caller sees the error.
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

/// A submitted child id and where it came from.
struct ChildId<'a> {
    table: &'static str,
    path: String,
    id: &'a str,
}

/// Ids seen while validating; rejects malformed and repeated ids.
struct IdSet<'a> {
    seen: HashSet<&'a str>,
    children: Vec<ChildId<'a>>,
}

impl<'a> IdSet<'a> {
    fn add(&mut self, table: &'static str, path: String, id: &'a str) -> Result<(), QueryError> {
        if !is_canonical_uuid(id) {
            return invalid(format!("{path} must be a lowercase UUID"));
        }
        if !self.seen.insert(id) {
            return invalid(format!(
                "{path} repeats an id used elsewhere in the document"
            ));
        }
        if !table.is_empty() {
            self.children.push(ChildId { table, path, id });
        }
        Ok(())
    }
}

fn invalid<T>(message: String) -> Result<T, QueryError> {
    Err(QueryError::InvalidInput(message))
}

/// Pure validation, no database access. Returns every child id for the cross-week check.
fn validate(
    iso_year: i32,
    iso_week: u32,
    input: &WeekInput,
) -> Result<Vec<ChildId<'_>>, QueryError> {
    if week_dates(iso_year, iso_week).is_none() {
        return invalid("isoYear and isoWeek are not a valid ISO week".into());
    }
    let mut ids = IdSet {
        seen: HashSet::new(),
        children: Vec::new(),
    };
    ids.add("", "week.id".into(), &input.week.id)?;

    let mut weekdays = HashSet::new();
    for (d, day) in input.days.iter().enumerate() {
        ids.add("days", format!("days[{d}].id"), &day.id)?;
        if !(1..=7).contains(&day.weekday) {
            return invalid(format!("days[{d}].weekday must be 1–7"));
        }
        if !weekdays.insert(day.weekday) {
            return invalid(format!("days[{d}].weekday repeats an earlier day"));
        }
        for (m, meal) in day.meals.iter().enumerate() {
            let at = format!("days[{d}].meals[{m}]");
            ids.add("meals", format!("{at}.id"), &meal.id)?;
            for (name, value) in [
                ("kcal", meal.kcal),
                ("proteinG", meal.protein_g),
                ("fatG", meal.fat_g),
                ("carbsG", meal.carbs_g),
                ("fiberG", meal.fiber_g),
            ] {
                if !value.is_finite() || value < 0.0 {
                    return invalid(format!("{at}.{name} must be a finite number >= 0"));
                }
            }
            for (s, step) in meal.steps.iter().enumerate() {
                ids.add("meal_steps", format!("{at}.steps[{s}].id"), &step.id)?;
            }
        }
    }
    for (l, list) in input.shopping_lists.iter().enumerate() {
        ids.add("shopping_lists", format!("shoppingLists[{l}].id"), &list.id)?;
        for (c, category) in list.categories.iter().enumerate() {
            let at = format!("shoppingLists[{l}].categories[{c}]");
            ids.add("shopping_categories", format!("{at}.id"), &category.id)?;
            for (i, item) in category.items.iter().enumerate() {
                ids.add("shopping_items", format!("{at}.items[{i}].id"), &item.id)?;
            }
        }
    }
    for (s, session) in input.prep_sessions.iter().enumerate() {
        ids.add(
            "prep_sessions",
            format!("prepSessions[{s}].id"),
            &session.id,
        )?;
        if session.weekday.is_some_and(|n| !(1..=7).contains(&n)) {
            return invalid(format!("prepSessions[{s}].weekday must be 1–7 or null"));
        }
        for (t, task) in session.tasks.iter().enumerate() {
            ids.add(
                "prep_tasks",
                format!("prepSessions[{s}].tasks[{t}].id"),
                &task.id,
            )?;
        }
    }

    let list_ids: HashSet<&str> = input.shopping_lists.iter().map(|l| l.id.as_str()).collect();
    for (d, day) in input.days.iter().enumerate() {
        let mut linked = HashSet::new();
        for (i, id) in day.shopping_list_ids.iter().enumerate() {
            if !list_ids.contains(id.as_str()) {
                return invalid(format!(
                    "days[{d}].shoppingListIds[{i}] names no list in shoppingLists"
                ));
            }
            if !linked.insert(id.as_str()) {
                return invalid(format!(
                    "days[{d}].shoppingListIds[{i}] repeats a list already on this day"
                ));
            }
        }
    }
    Ok(ids.children)
}

async fn replace_week(
    conn: &mut SqliteConnection,
    iso_year: i32,
    iso_week: u32,
    expected_revision: Option<&str>,
    input: &WeekInput,
    children: &[ChildId<'_>],
    pause: impl std::future::Future<Output = ()>,
) -> Result<RevisedWeek, QueryError> {
    let current = load_week_in(&mut *conn, iso_year, iso_week).await?;
    match (&current, expected_revision) {
        (None, None) => {}
        (Some(_), None) => return Err(QueryError::RevisionConflict(Conflict::AlreadySaved)),
        (None, Some(_)) => return Err(QueryError::RevisionConflict(Conflict::NotSaved)),
        (Some(document), Some(expected)) => {
            if week_revision(&mut *conn, &document.week.id).await? != expected {
                return Err(QueryError::RevisionConflict(Conflict::Stale));
            }
        }
    }

    match &current {
        Some(document) => {
            if document.week.id != input.week.id {
                return invalid("week.id must equal the id of the stored week".into());
            }
            sqlx::query("UPDATE weeks SET subtitle = ?, prep_intro = ? WHERE id = ?")
                .bind(&input.week.subtitle)
                .bind(&input.week.prep_intro)
                .bind(&input.week.id)
                .execute(&mut *conn)
                .await?;
            for table in ["days", "shopping_lists", "prep_sessions"] {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DELETE FROM {table} WHERE week_id = ?"
                )))
                .bind(&input.week.id)
                .execute(&mut *conn)
                .await?;
            }
        }
        None => {
            let taken = sqlx::query("SELECT 1 FROM weeks WHERE id = ?")
                .bind(&input.week.id)
                .fetch_optional(&mut *conn)
                .await?;
            if taken.is_some() {
                return invalid("week.id is already used by another week".into());
            }
            sqlx::query(
                "INSERT INTO weeks (id, iso_year, iso_week, subtitle, prep_intro) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&input.week.id)
            .bind(iso_year)
            .bind(iso_week)
            .bind(&input.week.subtitle)
            .bind(&input.week.prep_intro)
            .execute(&mut *conn)
            .await?;
        }
    }

    // Anything still present now belongs to another week.
    for child in children {
        let found = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT 1 FROM {} WHERE id = ?",
            child.table
        )))
        .bind(child.id)
        .fetch_optional(&mut *conn)
        .await?;
        if found.is_some() {
            return invalid(format!(
                "{}: id {} belongs to another week",
                child.path, child.id
            ));
        }
    }

    pause.await;

    insert_content(&mut *conn, input).await?;

    let document = load_week_in(&mut *conn, iso_year, iso_week)
        .await?
        .ok_or(QueryError::WeekNotFound)?;
    Ok(RevisedWeek {
        revision: week_revision(&mut *conn, &document.week.id).await?,
        document,
    })
}

fn meal_type(kind: MealType) -> &'static str {
    match kind {
        MealType::Breakfast => "Breakfast",
        MealType::Lunch => "Lunch",
        MealType::Dinner => "Dinner",
        MealType::Snack => "Snack",
    }
}

async fn insert_day(
    conn: &mut SqliteConnection,
    week_id: &str,
    day: &DayInput,
) -> Result<(), sqlx::Error> {
    let reminder = day.reminder.as_ref();
    sqlx::query(
        "INSERT INTO days (id, week_id, weekday, reminder_title, reminder_body, reminder_icon) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&day.id)
    .bind(week_id)
    .bind(day.weekday)
    .bind(reminder.map(|r| &r.title))
    .bind(reminder.and_then(|r| r.body.as_ref()))
    .bind(reminder.and_then(|r| r.icon.as_ref()))
    .execute(&mut *conn)
    .await?;
    for (m, meal) in day.meals.iter().enumerate() {
        sqlx::query(
            "INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, \
                                fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&meal.id)
        .bind(&day.id)
        .bind(m as i64)
        .bind(meal_type(meal.kind))
        .bind(&meal.name)
        .bind(&meal.description)
        .bind(meal.kcal)
        .bind(meal.protein_g)
        .bind(meal.fat_g)
        .bind(meal.carbs_g)
        .bind(meal.fiber_g)
        .bind(meal.for_two)
        .bind(&meal.assembly_time)
        .bind(&meal.cook_time)
        .bind(&meal.tip)
        .execute(&mut *conn)
        .await?;
        for (s, step) in meal.steps.iter().enumerate() {
            sqlx::query("INSERT INTO meal_steps (id, meal_id, position, text) VALUES (?, ?, ?, ?)")
                .bind(&step.id)
                .bind(&meal.id)
                .bind(s as i64)
                .bind(&step.text)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

/// Insert every child row with `position` = array index. Lists are inserted before the day links.
async fn insert_content(conn: &mut SqliteConnection, input: &WeekInput) -> Result<(), sqlx::Error> {
    let week_id = &input.week.id;
    for day in &input.days {
        insert_day(&mut *conn, week_id, day).await?;
    }
    for (l, list) in input.shopping_lists.iter().enumerate() {
        sqlx::query(
            "INSERT INTO shopping_lists (id, week_id, position, label, note) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&list.id)
        .bind(week_id)
        .bind(l as i64)
        .bind(&list.label)
        .bind(&list.note)
        .execute(&mut *conn)
        .await?;
        for (c, category) in list.categories.iter().enumerate() {
            sqlx::query(
                "INSERT INTO shopping_categories (id, shopping_list_id, position, name) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&category.id)
            .bind(&list.id)
            .bind(c as i64)
            .bind(&category.name)
            .execute(&mut *conn)
            .await?;
            for (i, item) in category.items.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO shopping_items (id, category_id, position, text) \
                     VALUES (?, ?, ?, ?)",
                )
                .bind(&item.id)
                .bind(&category.id)
                .bind(i as i64)
                .bind(&item.text)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    for day in &input.days {
        for (i, list_id) in day.shopping_list_ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO day_shopping_lists (day_id, shopping_list_id, position) \
                 VALUES (?, ?, ?)",
            )
            .bind(&day.id)
            .bind(list_id)
            .bind(i as i64)
            .execute(&mut *conn)
            .await?;
        }
    }
    for (s, session) in input.prep_sessions.iter().enumerate() {
        sqlx::query(
            "INSERT INTO prep_sessions (id, week_id, position, title, covers, weekday) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&session.id)
        .bind(week_id)
        .bind(s as i64)
        .bind(&session.title)
        .bind(&session.covers)
        .bind(session.weekday)
        .execute(&mut *conn)
        .await?;
        for (t, task) in session.tasks.iter().enumerate() {
            sqlx::query(
                "INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&task.id)
            .bind(&session.id)
            .bind(t as i64)
            .bind(&task.name)
            .bind(&task.time)
            .bind(&task.instructions)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}
