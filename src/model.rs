//! Week document served to the page (serde camelCase).
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeekDocument {
    pub schema_version: u32,
    pub week: Week,
    pub days: Vec<Day>,
    /// Every list of the week, by `shopping_lists.position`, including unassigned ones.
    pub shopping_lists: Vec<ShoppingList>,
    /// By `prep_sessions.position`.
    pub prep_sessions: Vec<PrepSession>,
    /// By `strategy_cards.position`.
    pub strategy_cards: Vec<StrategyCard>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Week {
    pub id: String,
    pub iso_year: i32,
    pub iso_week: u32,
    pub start_date: String,
    pub end_date: String,
    pub subtitle: Option<String>,
    pub prep_intro: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub id: Option<String>,
    pub date: String,
    pub weekday: u32,
    pub meals: Vec<Meal>,
    /// Lists shown on this day, by `day_shopping_lists.position`.
    pub shopping_list_ids: Vec<String>,
    /// Present only when `days.reminder_title` is non-null.
    pub reminder: Option<Reminder>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Reminder {
    pub title: String,
    pub body: Option<String>,
    pub icon: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Meal {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub description: String,
    pub kcal: f64,
    pub protein_g: f64,
    pub fat_g: f64,
    pub carbs_g: f64,
    pub fiber_g: f64,
    pub for_two: bool,
    pub assembly_time: Option<String>,
    pub cook_time: Option<String>,
    pub tip: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Step {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ShoppingList {
    pub id: String,
    pub label: String,
    pub note: Option<String>,
    pub categories: Vec<ShoppingCategory>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ShoppingCategory {
    pub id: String,
    pub name: String,
    pub items: Vec<ShoppingItem>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ShoppingItem {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrepSession {
    pub id: String,
    pub title: String,
    pub covers: Option<String>,
    /// By `prep_tasks.position`.
    pub tasks: Vec<PrepTask>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PrepTask {
    pub id: String,
    pub name: String,
    /// Display text, never parsed.
    pub time: String,
    pub instructions: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StrategyCard {
    pub id: String,
    pub title: String,
    pub icon: Option<String>,
    /// One of `green`, `blue`, `purple`, `yellow` (enforced by the database).
    pub theme: String,
    /// By `strategy_items.position`.
    pub items: Vec<StrategyItem>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StrategyItem {
    pub id: String,
    pub text: String,
}

/// One archive entry / available-week link.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeekSummary {
    pub iso_year: i32,
    pub iso_week: u32,
    pub start_date: String,
    pub end_date: String,
    /// Canonical page path, e.g. `/2026/44`.
    pub path: String,
}

/// Stored plans of one ISO week-year plus whether the adjacent years have plans.
#[derive(Debug)]
pub struct YearListing {
    /// Newest first.
    pub weeks: Vec<WeekSummary>,
    /// `iso_year - 1` when it has at least one stored plan.
    pub previous_year: Option<i32>,
    /// `iso_year + 1` when it has at least one stored plan.
    pub next_year: Option<i32>,
}

/// Initial payload embedded in every HTML page; the frontend renders by `kind`.
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Page {
    /// A stored plan. `selected_weekday` is 1 (Monday) ..= 7, chosen by the server.
    Week {
        selected_weekday: u32,
        document: WeekDocument,
    },
    /// A valid week with no stored plan.
    Missing { week: WeekSummary },
    /// `/<year>`: stored plans of one ISO week-year, newest first, and the adjacent years that have plans.
    Year {
        year: i32,
        previous_year: Option<i32>,
        next_year: Option<i32>,
        weeks: Vec<WeekSummary>,
    },
    /// Invalid year/week path. `current_year` is the ISO week-year of today, for the "All weeks" link.
    Invalid { current_year: i32 },
    /// Database failure.
    Unavailable,
}

impl Page {
    /// JSON safe to embed inside a `<script>` element.
    pub fn to_script_json(&self) -> String {
        let json = serde_json::to_string(self).expect("page serializes");
        let mut out = String::with_capacity(json.len());
        for c in json.chars() {
            match c {
                '<' => out.push_str("\\u003c"),
                '>' => out.push_str("\\u003e"),
                '&' => out.push_str("\\u0026"),
                '\u{2028}' => out.push_str("\\u2028"),
                '\u{2029}' => out.push_str("\\u2029"),
                c => out.push(c),
            }
        }
        out
    }
}

// ---- Feature 6: MCP results (camelCase, nullable fields always present) ----

/// `get_context`: server-local date and current ISO week.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// `YYYY-MM-DD` in the service timezone.
    pub today: String,
    /// IANA name of `APP_TIMEZONE`, e.g. `Europe/Berlin`.
    pub timezone: String,
    pub current_week: WeekSummary,
}

/// One stored week in `list_weeks`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoredWeek {
    pub id: String,
    pub iso_year: i32,
    pub iso_week: u32,
    pub start_date: String,
    pub end_date: String,
    /// Canonical page path, e.g. `/2026/44`.
    pub path: String,
}

/// `list_weeks` page: newest first.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeekPage {
    pub weeks: Vec<StoredWeek>,
    /// Null when there are no further results.
    pub next_cursor: Option<String>,
}

/// `get_week` / `save_week`: a complete week document and its revision token.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RevisedWeek {
    pub document: WeekDocument,
    /// Opaque; derived from all persisted week content.
    pub revision: String,
}

/// One `search_meals` match.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MealHit {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// `YYYY-MM-DD` of the meal's day.
    pub date: String,
    pub iso_year: i32,
    pub iso_week: u32,
    pub week_path: String,
}

/// `search_meals` page: date descending, then meal position, then id.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MealPage {
    pub meals: Vec<MealHit>,
    /// Null when there are no further results.
    pub next_cursor: Option<String>,
}

/// `get_meal`: every stored meal field plus where it sits in the calendar.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MealRecord {
    #[serde(flatten)]
    pub meal: Meal,
    pub date: String,
    pub weekday: u32,
    pub iso_year: i32,
    pub iso_week: u32,
    pub week_path: String,
}

/// `get_shopping_lists`: the lists assigned to one date.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShoppingDay {
    pub date: String,
    pub timezone: String,
    pub plan_exists: bool,
    pub iso_year: i32,
    pub iso_week: u32,
    pub week_path: String,
    /// Assigned lists in `day_shopping_lists.position` order; empty when none or no plan.
    pub lists: Vec<ShoppingList>,
}

// ---- Feature 6: writable week document for `save_week` ----
// Mirrors `WeekDocument` without derived fields (dates, schemaVersion, isoYear/isoWeek).
// Every field is required; optional values are explicit nulls. Array order is stored order.

/// Schema of a required field whose value may be an explicit `null`.
pub(crate) fn nullable<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    <Option<T>>::json_schema(generator)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeekInput {
    pub week: WeekFieldsInput,
    /// Day records, at most one per weekday. A weekday that is absent has no day record.
    pub days: Vec<DayInput>,
    pub shopping_lists: Vec<ShoppingListInput>,
    pub prep_sessions: Vec<PrepSessionInput>,
    pub strategy_cards: Vec<StrategyCardInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeekFieldsInput {
    /// Canonical lowercase UUID; must equal the stored id when replacing an existing week.
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub subtitle: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub prep_intro: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DayInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    /// 1 (Monday) ..= 7 (Sunday).
    #[schemars(range(min = 1, max = 7))]
    pub weekday: u32,
    pub meals: Vec<MealInput>,
    /// Ids of lists in this document's `shoppingLists`, in display order.
    #[schemars(inner(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    )))]
    pub shopping_list_ids: Vec<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<ReminderInput>")]
    pub reminder: Option<ReminderInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReminderInput {
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub body: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
pub enum MealType {
    Breakfast,
    Lunch,
    Dinner,
    Snack,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MealInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    #[serde(rename = "type")]
    pub kind: MealType,
    pub name: String,
    pub description: String,
    #[schemars(range(min = 0.0))]
    pub kcal: f64,
    #[schemars(range(min = 0.0))]
    pub protein_g: f64,
    #[schemars(range(min = 0.0))]
    pub fat_g: f64,
    #[schemars(range(min = 0.0))]
    pub carbs_g: f64,
    #[schemars(range(min = 0.0))]
    pub fiber_g: f64,
    pub for_two: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub assembly_time: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub cook_time: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub tip: Option<String>,
    pub steps: Vec<TextInput>,
}

/// A step, shopping item or strategy item.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShoppingListInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    pub label: String,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub note: Option<String>,
    pub categories: Vec<ShoppingCategoryInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShoppingCategoryInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    pub name: String,
    pub items: Vec<TextInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepSessionInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub covers: Option<String>,
    pub tasks: Vec<PrepTaskInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepTaskInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    pub name: String,
    pub time: String,
    pub instructions: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Green,
    Blue,
    Purple,
    Yellow,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StrategyCardInput {
    #[schemars(regex(
        pattern = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
    ))]
    pub id: String,
    pub title: String,
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "nullable::<String>")]
    pub icon: Option<String>,
    pub theme: Theme,
    pub items: Vec<TextInput>,
}
