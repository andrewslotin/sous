//! Week document served to the page (serde camelCase).
use crate::calendar::{self, DatePath};
use chrono::NaiveDate;
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
    /// 1 (Monday) ..= 7, or null: shown on no day.
    pub weekday: Option<u32>,
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

/// `schemaVersion` of the week document (page data, `/api/<year>/<week>`, `get_week`).
pub const WEEK_SCHEMA_VERSION: u32 = 3;

/// `schemaVersion` of the year document (`/api/<year>`).
pub const YEAR_SCHEMA_VERSION: u32 = 1;

/// `GET /api/<year>`: the year page's data as JSON.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct YearDocument {
    pub schema_version: u32,
    pub year: i32,
    pub previous_year: Option<i32>,
    pub next_year: Option<i32>,
    /// Newest first.
    pub weeks: Vec<WeekSummary>,
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

/// One stored week in the page data's `plannedWeeks`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedWeek {
    pub iso_year: i32,
    pub iso_week: u32,
}

/// One date in the page data's `markedDays`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkedDay {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub shopping: bool,
    pub prep: bool,
}

/// Initial payload embedded in every HTML page; the frontend renders by `kind`.
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Page {
    /// A stored plan. `today` is the server's today in `APP_TIMEZONE` (`YYYY-MM-DD`); `planned_weeks` lists
    /// every stored week, ascending, and `marked_days` every date with a shopping or prep mark, for the
    /// calendar. `displayed_date` is the day the page shows (`YYYY-MM-DD`, inside the document's week).
    Week {
        today: String,
        planned_weeks: Vec<PlannedWeek>,
        marked_days: Vec<MarkedDay>,
        displayed_date: String,
        document: WeekDocument,
    },
    /// A valid week with no stored plan.
    Missing {
        today: String,
        planned_weeks: Vec<PlannedWeek>,
        marked_days: Vec<MarkedDay>,
        displayed_date: String,
        week: WeekSummary,
    },
    /// `/<year>`: stored plans of one ISO week-year, newest first, and the adjacent years that have plans.
    Year {
        year: i32,
        previous_year: Option<i32>,
        next_year: Option<i32>,
        weeks: Vec<WeekSummary>,
    },
    /// Invalid year/week path. `displayed_date` is always `None` (serialized as null).
    Invalid {
        today: String,
        planned_weeks: Vec<PlannedWeek>,
        marked_days: Vec<MarkedDay>,
        displayed_date: Option<String>,
    },
    /// Date-shaped segment that is not a valid date path. `displayed_date` is always `None`.
    InvalidDate {
        today: String,
        planned_weeks: Vec<PlannedWeek>,
        marked_days: Vec<MarkedDay>,
        displayed_date: Option<String>,
    },
    /// `/profile`: the five household profile sections, in display order, and the back link's target.
    Profile {
        sections: Vec<ProfileSection>,
        back: Back,
    },
    /// Database failure.
    Unavailable,
}

/// Where the household page's back link goes. The server builds `href`; it never echoes the `from` query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Back {
    /// A valid date path other than today: `href` is `date_path(date)`, `date` is `YYYY-MM-DD`.
    Day { href: String, date: String },
    /// A valid year path: `href` is `year_path(year)`.
    Year { href: String, year: i32 },
    /// Fallback, and a day equal to today: `href` is today's date path.
    Today { href: String },
}

impl Back {
    /// Classify the raw `from` value (already query-decoded; `None` when absent or unreadable) against `today`.
    pub fn from_query(from: Option<&str>, today: NaiveDate) -> Back {
        if let Some(rest) = from.and_then(|value| value.strip_prefix('/')) {
            if let Some(year) = calendar::parse_year_path(rest) {
                return Back::Year {
                    href: calendar::year_path(year),
                    year,
                };
            }
            if let Some(DatePath::Valid(date)) = calendar::classify_date_path(rest)
                && date != today
            {
                return Back::Day {
                    href: calendar::date_path(date),
                    date: date.format("%Y-%m-%d").to_string(),
                };
            }
        }
        Back::Today {
            href: calendar::date_path(today),
        }
    }
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
    /// The household profile: always five sections, in display order.
    pub profile: Profile,
}

/// One of the five fixed profile sections, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SectionKey {
    Household,
    Food,
    Kitchen,
    Planning,
    Other,
}

impl SectionKey {
    /// Display order.
    pub const ALL: [SectionKey; 5] = [
        SectionKey::Household,
        SectionKey::Food,
        SectionKey::Kitchen,
        SectionKey::Planning,
        SectionKey::Other,
    ];

    /// The stored and serialized key.
    pub fn as_str(self) -> &'static str {
        match self {
            SectionKey::Household => "household",
            SectionKey::Food => "food",
            SectionKey::Kitchen => "kitchen",
            SectionKey::Planning => "planning",
            SectionKey::Other => "other",
        }
    }

    /// The section's UI title.
    pub fn title(self) -> &'static str {
        match self {
            SectionKey::Household => "People",
            SectionKey::Food => "Food preferences",
            SectionKey::Kitchen => "Kitchen",
            SectionKey::Planning => "Planning habits",
            SectionKey::Other => "Other context",
        }
    }

    /// Exact lowercase match only.
    pub fn parse(key: &str) -> Option<SectionKey> {
        SectionKey::ALL.into_iter().find(|k| k.as_str() == key)
    }
}

/// One household profile section and its revision.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSection {
    pub key: SectionKey,
    /// UI title of the section.
    pub title: String,
    /// Plain text, at most 4,000 characters; line breaks are LF.
    pub text: String,
    /// Opaque; changes whenever the section's text changes. Pass it back as `expectedRevision`.
    pub revision: String,
}

/// The household profile: always five sections, in `SectionKey::ALL` order.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub sections: Vec<ProfileSection>,
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

/// Schema of a prep session's `weekday`: an integer 1–7 or null.
fn weekday_schema(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({
        "type": ["integer", "null"],
        "minimum": 1,
        "maximum": 7
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeekInput {
    pub week: WeekFieldsInput,
    /// Day records, at most one per weekday. A weekday that is absent has no day record.
    pub days: Vec<DayInput>,
    pub shopping_lists: Vec<ShoppingListInput>,
    pub prep_sessions: Vec<PrepSessionInput>,
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

/// A step or shopping item.
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
    /// 1 (Monday) ..= 7 of the session's week, or null: the session is shown on no day.
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required, schema_with = "weekday_schema")]
    pub weekday: Option<u32>,
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

#[cfg(test)]
mod back_tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 17).unwrap()
    }

    fn today_back() -> Back {
        Back::Today {
            href: "/2026-11-17".into(),
        }
    }

    #[test]
    fn back_from_query() {
        let day = Back::Day {
            href: "/2026-11-11".into(),
            date: "2026-11-11".into(),
        };
        let year = |year: i32, href: &str| Back::Year {
            href: href.into(),
            year,
        };
        for (from, expected) in [
            (Some("/2026-11-11"), day),
            (Some("/2026-11-17"), today_back()),
            (Some("/2026"), year(2026, "/2026")),
            (Some("/0001"), year(1, "/0001")),
            (None, today_back()),
            (Some(""), today_back()),
            (Some("/"), today_back()),
            (Some("2026-11-11"), today_back()),
            (Some("/2026/44"), today_back()),
            (Some("/2026-02-30"), today_back()),
            (Some("//example.com"), today_back()),
            (Some("https://example.com/2026-11-11"), today_back()),
            (Some("/2026-11-11/extra"), today_back()),
            (Some("//2026-11-11"), today_back()),
            (Some("/9999-12-31"), today_back()),
            (Some("/2026-11-11 "), today_back()),
        ] {
            assert_eq!(Back::from_query(from, today()), expected, "{from:?}");
        }
    }

    #[test]
    fn back_serializes_to_the_documented_shapes() {
        let json = |back: Back| serde_json::to_string(&back).unwrap();
        assert_eq!(
            json(Back::from_query(Some("/2026-11-11"), today())),
            r#"{"kind":"day","href":"/2026-11-11","date":"2026-11-11"}"#
        );
        assert_eq!(
            json(Back::from_query(Some("/0001"), today())),
            r#"{"kind":"year","href":"/0001","year":1}"#
        );
        assert_eq!(
            json(Back::from_query(None, today())),
            r#"{"kind":"today","href":"/2026-11-17"}"#
        );
    }
}
