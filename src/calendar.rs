//! ISO week parsing, date derivation and the service clock.
use chrono::{DateTime, Datelike, NaiveDate, Utc, Weekday};
use chrono_tz::Tz;
use std::sync::Arc;

/// Service timezone when `APP_TIMEZONE` is unset.
pub const DEFAULT_TIMEZONE: Tz = chrono_tz::Europe::Berlin;

/// Injectable source of "now" plus the configured service timezone.
#[derive(Clone)]
pub struct Clock {
    tz: Tz,
    now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl Clock {
    /// Clock with a custom time source (tests can make it mutable).
    pub fn new(tz: Tz, now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static) -> Self {
        Clock {
            tz,
            now: Arc::new(now),
        }
    }

    /// Wall clock.
    pub fn system(tz: Tz) -> Self {
        Self::new(tz, Utc::now)
    }

    /// Always returns `instant`.
    pub fn fixed(tz: Tz, instant: DateTime<Utc>) -> Self {
        Self::new(tz, move || instant)
    }

    /// Today's date in the service timezone.
    pub fn today(&self) -> NaiveDate {
        (self.now)().with_timezone(&self.tz).date_naive()
    }
}

/// Parse an `APP_TIMEZONE` value. `None` means unset and yields [`DEFAULT_TIMEZONE`];
/// any value that is not an IANA zone name (including empty) is an error.
pub fn parse_timezone(value: Option<&str>) -> Result<Tz, String> {
    match value {
        None => Ok(DEFAULT_TIMEZONE),
        Some(name) => name
            .parse::<Tz>()
            .map_err(|_| format!("invalid APP_TIMEZONE {name:?}: expected an IANA zone name")),
    }
}

/// ISO (week-year, week) containing `date`.
pub fn iso_week_of(date: NaiveDate) -> (i32, u32) {
    let iso = date.iso_week();
    (iso.year(), iso.week())
}

/// Outcome of matching `/{year}/{week}` path segments.
#[derive(Debug, PartialEq, Eq)]
pub enum WeekPath {
    /// Already canonical and valid.
    Canonical(i32, u32),
    /// Valid week written with leading zeros; redirect to the canonical path.
    Redirect(i32, u32),
    /// Not a representable ISO week.
    Invalid,
}

/// Classify path segments; see [`WeekPath`].
pub fn classify_week_path(year: &str, week: &str) -> WeekPath {
    if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return WeekPath::Invalid;
    }
    if week.is_empty() || !week.bytes().all(|b| b.is_ascii_digit()) {
        return WeekPath::Invalid;
    }
    // Strip zeros before parsing so arbitrarily long inputs cannot overflow.
    let stripped = week.trim_start_matches('0');
    if stripped.is_empty() || stripped.len() > 2 {
        return WeekPath::Invalid;
    }
    let (Ok(iso_year), Ok(iso_week)) = (year.parse::<i32>(), stripped.parse::<u32>()) else {
        return WeekPath::Invalid;
    };
    if week_dates(iso_year, iso_week).is_none() {
        return WeekPath::Invalid;
    }
    if stripped.len() == week.len() {
        WeekPath::Canonical(iso_year, iso_week)
    } else {
        WeekPath::Redirect(iso_year, iso_week)
    }
}

/// Canonical page path of a week, e.g. `/2026/44`. The year is zero-padded to four digits
/// (`/0001/1`) so that every generated path is accepted by [`classify_week_path`].
pub fn week_path(iso_year: i32, iso_week: u32) -> String {
    format!("/{iso_year:04}/{iso_week}")
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
    use chrono::TimeZone;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn classifies_canonical() {
        for (y, w, expected) in [
            ("2026", "44", (2026, 44)),
            ("2026", "1", (2026, 1)),
            ("2026", "53", (2026, 53)),
            ("2020", "53", (2020, 53)),
            ("0001", "1", (1, 1)),
            ("9999", "51", (9999, 51)),
        ] {
            assert_eq!(
                classify_week_path(y, w),
                WeekPath::Canonical(expected.0, expected.1),
                "{y}/{w}"
            );
        }
    }

    #[test]
    fn classifies_zero_padded_as_redirect() {
        for (y, w, expected) in [
            ("2026", "04", (2026, 4)),
            ("2026", "004", (2026, 4)),
            ("2026", "053", (2026, 53)),
            ("9999", "051", (9999, 51)),
            ("0001", "01", (1, 1)),
        ] {
            assert_eq!(
                classify_week_path(y, w),
                WeekPath::Redirect(expected.0, expected.1),
                "{y}/{w}"
            );
        }
    }

    #[test]
    fn classifies_invalid() {
        for (y, w) in [
            ("2025", "053"),
            ("2025", "53"),
            ("2027", "53"),
            ("2026", "0"),
            ("2026", "00"),
            ("2026", "54"),
            ("2026", "99"),
            ("2026", "100"),
            ("2026", "abc"),
            ("2026", ""),
            ("2026", "+4"),
            ("2026", "-4"),
            ("2026", " 4"),
            ("2026", "4 "),
            ("2026", "４"),
            ("2026", "99999999999999999999"),
            ("2026", "00000000000000000000000000"),
            ("0000", "1"),
            ("10000", "1"),
            ("026", "1"),
            ("26", "1"),
            ("", "1"),
            ("+026", "1"),
            ("９９９９", "1"),
            ("9999", "52"),
            ("9999", "053"),
        ] {
            assert_eq!(classify_week_path(y, w), WeekPath::Invalid, "{y}/{w}");
        }
    }

    #[test]
    fn generated_paths_round_trip() {
        assert_eq!(week_path(2026, 44), "/2026/44");
        assert_eq!(week_path(1, 1), "/0001/1");
        assert_eq!(week_path(999, 3), "/0999/3");
        for (y, w) in [(1, 1), (999, 3), (2026, 53), (9999, 51)] {
            let path = week_path(y, w);
            let mut parts = path.trim_start_matches('/').split('/');
            let (ys, ws) = (parts.next().unwrap(), parts.next().unwrap());
            assert_eq!(classify_week_path(ys, ws), WeekPath::Canonical(y, w));
        }
    }

    #[test]
    fn week_dates_of_known_weeks() {
        let w44 = week_dates(2026, 44).unwrap();
        assert_eq!(w44[0], d(2026, 10, 26));
        assert_eq!(w44[6], d(2026, 11, 1));
        let w53 = week_dates(2026, 53).unwrap();
        assert_eq!(w53[0], d(2026, 12, 28));
        assert_eq!(w53[6], d(2027, 1, 3));
        assert!(week_dates(2020, 53).is_some());
        assert!(week_dates(2025, 53).is_none());
        assert!(week_dates(2027, 53).is_none());
        assert!(week_dates(9999, 51).is_some());
        assert!(week_dates(9999, 52).is_none());
        assert!(week_dates(0, 1).is_none());
        assert!(week_dates(10000, 1).is_none());
    }

    #[test]
    fn iso_week_of_table() {
        for ((y, m, day), expected) in [
            ((2026, 10, 26), (2026, 44)),
            ((2026, 11, 1), (2026, 44)),
            ((2026, 12, 31), (2026, 53)),
            ((2027, 1, 3), (2026, 53)),
            ((2027, 1, 4), (2027, 1)),
            ((2021, 1, 3), (2020, 53)),
            ((2024, 12, 30), (2025, 1)),
        ] {
            assert_eq!(iso_week_of(d(y, m, day)), expected, "{y}-{m}-{day}");
        }
    }

    fn utc(y: i32, m: u32, day: u32, h: u32, min: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, day, h, min, s).unwrap()
    }

    #[test]
    fn clock_today_in_berlin() {
        for (instant, expected, week) in [
            // CET (UTC+1)
            (utc(2026, 11, 1, 22, 59, 59), d(2026, 11, 1), (2026, 44)),
            (utc(2026, 11, 1, 23, 0, 0), d(2026, 11, 2), (2026, 45)),
            // CEST (UTC+2)
            (utc(2026, 10, 18, 21, 59, 59), d(2026, 10, 18), (2026, 42)),
            (utc(2026, 10, 18, 22, 0, 0), d(2026, 10, 19), (2026, 43)),
            // DST ends 2026-10-25 01:00Z; midnight Sunday->Monday is at 23:00Z (CET)
            (utc(2026, 10, 25, 22, 59, 59), d(2026, 10, 25), (2026, 43)),
            (utc(2026, 10, 25, 23, 0, 0), d(2026, 10, 26), (2026, 44)),
        ] {
            let clock = Clock::fixed(DEFAULT_TIMEZONE, instant);
            assert_eq!(clock.today(), expected, "{instant}");
            assert_eq!(iso_week_of(clock.today()), week, "{instant}");
        }
    }

    #[test]
    fn clock_today_depends_on_timezone() {
        let instant = utc(2026, 11, 1, 23, 30, 0);
        let berlin = Clock::fixed(DEFAULT_TIMEZONE, instant);
        let new_york = Clock::fixed(chrono_tz::America::New_York, instant);
        assert_eq!(berlin.today(), d(2026, 11, 2));
        assert_eq!(new_york.today(), d(2026, 11, 1));
    }

    #[test]
    fn clock_new_reads_the_time_source_each_call() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let secs = Arc::new(AtomicI64::new(utc(2026, 11, 1, 12, 0, 0).timestamp()));
        let source = secs.clone();
        let clock = Clock::new(chrono_tz::UTC, move || {
            Utc.timestamp_opt(source.load(Ordering::SeqCst), 0).unwrap()
        });
        assert_eq!(clock.today(), d(2026, 11, 1));
        secs.store(utc(2026, 11, 2, 0, 0, 0).timestamp(), Ordering::SeqCst);
        assert_eq!(clock.today(), d(2026, 11, 2));
    }

    #[test]
    fn parses_timezone_config() {
        assert_eq!(parse_timezone(None), Ok(chrono_tz::Europe::Berlin));
        assert_eq!(parse_timezone(Some("UTC")), Ok(chrono_tz::UTC));
        assert_eq!(
            parse_timezone(Some("America/New_York")),
            Ok(chrono_tz::America::New_York)
        );
        for bad in ["", "Mars/Olympus", "Berlin", " UTC", "Europe/Berlin "] {
            assert!(parse_timezone(Some(bad)).is_err(), "{bad:?}");
        }
    }
}
