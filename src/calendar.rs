//! ISO week parsing and date derivation.
use chrono::{Datelike, NaiveDate, Weekday};

/// Parse `/{year}/{week}` path segments. Only canonical forms are accepted:
/// four ASCII digits for the year, one or two digits without leading zero for the week.
pub fn parse_week_path(year: &str, week: &str) -> Option<(i32, u32)> {
    if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if week.is_empty()
        || week.len() > 2
        || week.starts_with('0')
        || !week.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let week: u32 = week.parse().ok()?;
    week_dates(year, week)?;
    Some((year, week))
}

/// Monday..Sunday dates of an ISO week, or `None` if the week does not exist
/// or is not fully representable in 0001-01-01..=9999-12-31.
pub fn week_dates(iso_year: i32, iso_week: u32) -> Option<[NaiveDate; 7]> {
    if !(1..=9999).contains(&iso_year) {
        return None;
    }
    let monday = NaiveDate::from_isoywd_opt(iso_year, iso_week, Weekday::Mon)?;
    let sunday = NaiveDate::from_isoywd_opt(iso_year, iso_week, Weekday::Sun)?;
    if monday.year() < 1 || sunday.year() > 9999 {
        return None;
    }
    let mut days = [monday; 7];
    for (i, d) in days.iter_mut().enumerate() {
        *d = monday + chrono::Days::new(i as u64);
    }
    Some(days)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical() {
        assert_eq!(parse_week_path("2026", "44"), Some((2026, 44)));
        assert_eq!(parse_week_path("2026", "1"), Some((2026, 1)));
        assert_eq!(parse_week_path("2026", "53"), Some((2026, 53)));
        assert_eq!(parse_week_path("0001", "1"), Some((1, 1)));
    }

    #[test]
    fn rejects_invalid() {
        for (y, w) in [
            ("2025", "53"),
            ("2026", "04"),
            ("2026", "0"),
            ("2026", "54"),
            ("2026", "abc"),
            ("2026", "+4"),
            ("0000", "1"),
            ("10000", "1"),
            ("26", "1"),
            ("９９９９", "1"),
            ("9999", "52"),
        ] {
            assert_eq!(parse_week_path(y, w), None, "{y}/{w}");
        }
    }

    #[test]
    fn last_representable_week() {
        assert!(parse_week_path("9999", "51").is_some());
    }
}
