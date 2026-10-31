//! Application queries.
use crate::calendar::{week_dates, week_path};
use crate::model::{
    Day, Meal, ShoppingCategory, ShoppingItem, ShoppingList, Step, Week, WeekDocument, WeekSummary,
};
use sqlx::{Row, SqlitePool};

/// Load one week inside a single read transaction. `None` if the week row does not exist.
pub async fn load_week(
    pool: &SqlitePool,
    iso_year: i32,
    iso_week: u32,
) -> Result<Option<WeekDocument>, sqlx::Error> {
    let Some(dates) = week_dates(iso_year, iso_week) else {
        return Ok(None);
    };
    let mut tx = pool.begin().await?;

    let Some(week_row) =
        sqlx::query("SELECT id, subtitle FROM weeks WHERE iso_year = ? AND iso_week = ?")
            .bind(iso_year)
            .bind(iso_week)
            .fetch_optional(&mut *tx)
            .await?
    else {
        return Ok(None);
    };
    let week_id: String = week_row.get("id");

    let mut days: Vec<Day> = dates
        .iter()
        .enumerate()
        .map(|(i, d)| Day {
            id: None,
            date: d.format("%Y-%m-%d").to_string(),
            weekday: i as u32 + 1,
            meals: Vec::new(),
            shopping_list_ids: Vec::new(),
        })
        .collect();

    for row in sqlx::query("SELECT id, weekday FROM days WHERE week_id = ? ORDER BY weekday")
        .bind(&week_id)
        .fetch_all(&mut *tx)
        .await?
    {
        let weekday: i64 = row.get("weekday");
        days[(weekday - 1) as usize].id = Some(row.get("id"));
    }

    let meal_rows = sqlx::query(
        "SELECT m.id, d.weekday, m.type, m.name, m.description, m.kcal, m.protein_g, m.fat_g, \
                m.carbs_g, m.fiber_g, m.for_two, m.assembly_time, m.cook_time, m.tip \
         FROM meals m JOIN days d ON d.id = m.day_id \
         WHERE d.week_id = ? ORDER BY d.weekday, m.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *tx)
    .await?;

    let step_rows = sqlx::query(
        "SELECT s.id, s.meal_id, s.text FROM meal_steps s \
         JOIN meals m ON m.id = s.meal_id JOIN days d ON d.id = m.day_id \
         WHERE d.week_id = ? ORDER BY s.meal_id, s.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *tx)
    .await?;

    let list_rows = sqlx::query(
        "SELECT id, label, note FROM shopping_lists WHERE week_id = ? ORDER BY position",
    )
    .bind(&week_id)
    .fetch_all(&mut *tx)
    .await?;

    let category_rows = sqlx::query(
        "SELECT c.id, c.shopping_list_id, c.name FROM shopping_categories c \
         JOIN shopping_lists l ON l.id = c.shopping_list_id \
         WHERE l.week_id = ? ORDER BY c.shopping_list_id, c.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *tx)
    .await?;

    let item_rows = sqlx::query(
        "SELECT i.id, i.category_id, i.text FROM shopping_items i \
         JOIN shopping_categories c ON c.id = i.category_id \
         JOIN shopping_lists l ON l.id = c.shopping_list_id \
         WHERE l.week_id = ? ORDER BY i.category_id, i.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *tx)
    .await?;

    let link_rows = sqlx::query(
        "SELECT d.weekday, x.shopping_list_id FROM day_shopping_lists x \
         JOIN days d ON d.id = x.day_id \
         WHERE d.week_id = ? ORDER BY d.weekday, x.position",
    )
    .bind(&week_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

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
        schema_version: 1,
        week: Week {
            id: week_id,
            iso_year,
            iso_week,
            start_date: days[0].date.clone(),
            end_date: days[6].date.clone(),
            subtitle: week_row.get("subtitle"),
        },
        days,
        shopping_lists,
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

/// Stored weeks in descending ISO year/week order.
pub async fn list_weeks(pool: &SqlitePool) -> Result<Vec<WeekSummary>, sqlx::Error> {
    let rows =
        sqlx::query("SELECT iso_year, iso_week FROM weeks ORDER BY iso_year DESC, iso_week DESC")
            .fetch_all(pool)
            .await?;
    let mut weeks = Vec::with_capacity(rows.len());
    for row in rows {
        let iso_year: i32 = row.get("iso_year");
        let iso_week: u32 = row.get("iso_week");
        match week_summary(iso_year, iso_week) {
            Some(summary) => weeks.push(summary),
            None => tracing::warn!(
                iso_year,
                iso_week,
                "skipping stored week with unrepresentable dates"
            ),
        }
    }
    Ok(weeks)
}
