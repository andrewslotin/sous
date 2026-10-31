//! Week document served to the page (serde camelCase).
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeekDocument {
    pub schema_version: u32,
    pub week: Week,
    pub days: Vec<Day>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Week {
    pub id: String,
    pub iso_year: i32,
    pub iso_week: u32,
    pub start_date: String,
    pub end_date: String,
    pub subtitle: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub id: Option<String>,
    pub date: String,
    pub weekday: u32,
    pub meals: Vec<Meal>,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct Step {
    pub id: String,
    pub text: String,
}

/// One archive entry / available-week link.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeekSummary {
    pub iso_year: i32,
    pub iso_week: u32,
    pub start_date: String,
    pub end_date: String,
    /// Canonical page path, e.g. `/2026/44`.
    pub path: String,
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
    /// `/weeks`: stored plans, newest first.
    Archive { weeks: Vec<WeekSummary> },
    /// Invalid year/week path.
    Invalid,
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
