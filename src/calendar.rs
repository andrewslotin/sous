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

    /// The configured service timezone.
    pub fn timezone(&self) -> Tz {
        self.tz
    }
}

/// Parse a strict `YYYY-MM-DD` calendar date (four-digit year 0001–9999, two-digit month and day).
/// Anything else, including `2026-1-5`, surrounding whitespace and impossible dates, is `None`.
pub fn parse_date(value: &str) -> Option<NaiveDate> {
    let b = value.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = &b[range];
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(part.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0')))
    };
    let year = digits(0..4)?;
    if year == 0 {
        return None;
    }
    NaiveDate::from_ymd_opt(year as i32, digits(5..7)?, digits(8..10)?)
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

/// Parse a year path segment: exactly four ASCII digits, 0001–9999.
pub fn parse_year_path(segment: &str) -> Option<i32> {
    if segment.len() != 4 || !segment.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    segment.parse().ok().filter(|year| *year >= 1)
}

/// Canonical page path of a week, e.g. `/2026/44`. The year is zero-padded to four digits
/// (`/0001/1`) so that every generated path is accepted by [`classify_week_path`].
pub fn week_path(iso_year: i32, iso_week: u32) -> String {
    format!("/{iso_year:04}/{iso_week}")
}

/// Page path of a year, e.g. `/2026`; zero-padded to four digits (`/0001`), accepted by [`parse_year_path`].
pub fn year_path(year: i32) -> String {
    format!("/{year:04}")
}

/// Outcome of matching a single path segment as a date.
#[derive(Debug, PartialEq, Eq)]
pub enum DatePath {
    /// A real Gregorian date whose whole ISO week is representable (`week_dates` is `Some`).
    Valid(NaiveDate),
    /// Date-shaped but not a valid date path, e.g. `2026-02-30`, `0000-01-01`, `9999-12-27`..`9999-12-31`.
    Invalid,
}

/// `None` when `segment` is not date-shaped: exactly `DDDD-DD-DD` with ASCII digits.
pub fn classify_date_path(segment: &str) -> Option<DatePath> {
    let b = segment.as_bytes();
    let shaped = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        });
    if !shaped {
        return None;
    }
    Some(match parse_date(segment) {
        Some(date) => {
            let (iso_year, iso_week) = iso_week_of(date);
            if week_dates(iso_year, iso_week).is_some() {
                DatePath::Valid(date)
            } else {
                DatePath::Invalid
            }
        }
        None => DatePath::Invalid,
    })
}

/// Page path of a date, e.g. `/2026-11-11`; the year is zero-padded to four digits (`/0001-01-01`).
pub fn date_path(date: NaiveDate) -> String {
    format!("/{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
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
    fn parses_year_paths() {
        for (segment, expected) in [("0001", 1), ("0999", 999), ("2026", 2026), ("9999", 9999)] {
            assert_eq!(parse_year_path(segment), Some(expected), "{segment}");
        }
        for bad in [
            "",
            "0000",
            "26",
            "026",
            "02026",
            "10000",
            "+026",
            "-026",
            "２０２６",
            "202 6",
            " 2026",
            "2026 ",
            "%202026",
            "weeks",
            "nothing",
            "abcd",
            "2026\n",
        ] {
            assert_eq!(parse_year_path(bad), None, "{bad:?}");
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
    fn year_paths_round_trip() {
        assert_eq!(year_path(1), "/0001");
        assert_eq!(year_path(2026), "/2026");
        for year in [1, 999, 2026, 9999] {
            let path = year_path(year);
            assert_eq!(parse_year_path(path.trim_start_matches('/')), Some(year));
        }
    }

    #[test]
    fn classifies_date_paths() {
        for (segment, expected) in [
            ("2026-11-11", d(2026, 11, 11)),
            ("2025-12-29", d(2025, 12, 29)),
            ("2024-02-29", d(2024, 2, 29)),
            ("0001-01-01", d(1, 1, 1)),
            ("9999-12-26", d(9999, 12, 26)),
        ] {
            assert_eq!(
                classify_date_path(segment),
                Some(DatePath::Valid(expected)),
                "{segment}"
            );
        }
        for segment in [
            "2026-02-30",
            "2025-02-29",
            "0000-01-01",
            "2026-13-01",
            "2026-00-10",
            "2026-01-00",
            "9999-12-27",
            "9999-12-31",
        ] {
            assert_eq!(
                classify_date_path(segment),
                Some(DatePath::Invalid),
                "{segment}"
            );
        }
        for segment in [
            "",
            "2026",
            "weeks",
            "2026-1-5",
            "2026-11",
            "2026-11-111",
            "20261111",
            "2026/11/11",
            "２０２６-11-11",
            " 2026-11-11",
            "2026-11-11 ",
            "+026-11-11",
            "2026-+1-11",
        ] {
            assert_eq!(classify_date_path(segment), None, "{segment:?}");
        }
    }

    #[test]
    fn date_paths_round_trip() {
        assert_eq!(date_path(d(2026, 11, 11)), "/2026-11-11");
        assert_eq!(date_path(d(1, 1, 1)), "/0001-01-01");
        assert_eq!(date_path(d(999, 3, 4)), "/0999-03-04");
        for date in [d(1, 1, 1), d(2026, 11, 11), d(2024, 2, 29), d(9999, 12, 26)] {
            let path = date_path(date);
            assert_eq!(
                classify_date_path(path.trim_start_matches('/')),
                Some(DatePath::Valid(date))
            );
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
    fn parse_date_is_strict() {
        assert_eq!(parse_date("2026-10-26"), Some(d(2026, 10, 26)));
        assert_eq!(parse_date("0001-01-01"), Some(d(1, 1, 1)));
        assert_eq!(parse_date("9999-12-31"), Some(d(9999, 12, 31)));
        assert_eq!(parse_date("2024-02-29"), Some(d(2024, 2, 29)));
        for bad in [
            "",
            "2026-1-5",
            "2026-01-5",
            " 2026-01-05",
            "2026-01-05 ",
            "2026-02-30",
            "2025-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-01-00",
            "0000-01-01",
            "10000-01-01",
            "2026/01/05",
            "20260105",
            "+026-01-05",
            "2026-01-+5",
            "２０２６-01-05",
            "2026-01-05T00:00:00",
        ] {
            assert_eq!(parse_date(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn clock_reports_its_timezone() {
        let instant = utc(2026, 10, 26, 9, 0, 0);
        assert_eq!(
            Clock::fixed(chrono_tz::Europe::Berlin, instant).timezone(),
            chrono_tz::Europe::Berlin
        );
        assert_eq!(
            Clock::system(chrono_tz::Pacific::Kiritimati)
                .timezone()
                .name(),
            "Pacific/Kiritimati"
        );
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
