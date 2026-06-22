//! Parsing of the user-supplied "when" — either an absolute date/time or a
//! relative duration from now. Both resolve to an absolute local timestamp.

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Local, NaiveDateTime, NaiveTime, TimeZone};

/// Parse a complete date and time in the local timezone.
///
/// Accepts `YYYY-MM-DD HH:MM[:SS]`, or a time only (`HH:MM[:SS]`) in which case
/// the date defaults to today.
pub fn parse_absolute(input: &str) -> Result<DateTime<Local>> {
    let input = input.trim();
    let naive = parse_naive(input)?;
    // Map the naive local time to a concrete instant. On a DST edge where the
    // time is ambiguous or nonexistent, fall back to the earliest reading.
    match Local.from_local_datetime(&naive).single() {
        Some(dt) => Ok(dt),
        None => Local
            .from_local_datetime(&naive)
            .earliest()
            .ok_or_else(|| anyhow::anyhow!("invalid local time: {input}")),
    }
}

/// Parse `input` into a naive local date-time, defaulting the date to today
/// when only a time is given.
fn parse_naive(input: &str) -> Result<NaiveDateTime> {
    const DATETIME_FORMATS: &[&str] = &["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"];
    for fmt in DATETIME_FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(input, fmt) {
            return Ok(naive);
        }
    }

    const TIME_FORMATS: &[&str] = &["%H:%M:%S", "%H:%M"];
    for fmt in TIME_FORMATS {
        if let Ok(time) = NaiveTime::parse_from_str(input, fmt) {
            return Ok(Local::now().date_naive().and_time(time));
        }
    }

    bail!("expected `YYYY-MM-DD HH:MM[:SS]` or `HH:MM[:SS]`");
}

/// Parse a relative duration like `3 minutes later`, `90s`, or `2 hours`,
/// returning the absolute time it points to (now + duration).
pub fn parse_relative(input: &str) -> Result<DateTime<Local>> {
    let dur = parse_duration(input)?;
    Ok(Local::now() + dur)
}

/// Parse a human duration into a [`chrono::Duration`].
///
/// Forms accepted (case-insensitive, optional trailing "later"/"from now"):
///   `3 minutes`, `3 min`, `3m`, `90s`, `2 hours`, `1 day`. A zero duration
///   (e.g. `0s`) is accepted and denotes immediate delivery.
fn parse_duration(input: &str) -> Result<Duration> {
    let mut s = input.trim().to_lowercase();
    for suffix in [" later", " from now"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.trim_end().to_string();
        }
    }

    // Split into the leading numeric part and the trailing unit, tolerating an
    // optional space between them ("3 min" or "3min").
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '-' && c != '+')
        .unwrap_or(s.len());
    let (num_part, unit_part) = s.split_at(split);
    let num_part = num_part.trim();
    let unit = unit_part.trim();

    if num_part.is_empty() {
        bail!("expected a number, e.g. `3 minutes`");
    }
    let amount: i64 = num_part.parse()?;
    if amount < 0 {
        bail!("duration must not be negative");
    }
    // A zero duration is allowed and means "now" (immediate delivery).

    let dur = match unit {
        "s" | "sec" | "secs" | "second" | "seconds" => Duration::seconds(amount),
        "" | "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(amount),
        "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(amount),
        "d" | "day" | "days" => Duration::days(amount),
        other => bail!("unknown time unit: {other:?} (use s/min/hour/day)"),
    };
    Ok(dur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_with_seconds() {
        let dt = parse_absolute("2026-06-22 15:30:45").unwrap();
        assert_eq!(
            dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2026-06-22 15:30:45"
        );
    }

    #[test]
    fn absolute_without_seconds() {
        let dt = parse_absolute("2026-06-22 15:30").unwrap();
        assert_eq!(dt.format("%H:%M:%S").to_string(), "15:30:00");
    }

    #[test]
    fn time_only_defaults_to_today() {
        let today = Local::now().date_naive();
        let dt = parse_absolute("15:30").unwrap();
        assert_eq!(dt.date_naive(), today);
        assert_eq!(dt.format("%H:%M:%S").to_string(), "15:30:00");
    }

    #[test]
    fn time_only_with_seconds() {
        let today = Local::now().date_naive();
        let dt = parse_absolute("09:05:07").unwrap();
        assert_eq!(dt.date_naive(), today);
        assert_eq!(dt.format("%H:%M:%S").to_string(), "09:05:07");
    }

    #[test]
    fn absolute_rejects_garbage() {
        assert!(parse_absolute("not a date").is_err());
    }

    #[test]
    fn relative_variants() {
        assert_eq!(
            parse_duration("3 minutes later").unwrap(),
            Duration::minutes(3)
        );
        assert_eq!(parse_duration("3 min").unwrap(), Duration::minutes(3));
        assert_eq!(parse_duration("3m").unwrap(), Duration::minutes(3));
        assert_eq!(parse_duration("90s").unwrap(), Duration::seconds(90));
        assert_eq!(parse_duration("2 hours").unwrap(), Duration::hours(2));
        assert_eq!(parse_duration("1 day from now").unwrap(), Duration::days(1));
        assert_eq!(parse_duration("5").unwrap(), Duration::minutes(5));
    }

    #[test]
    fn relative_zero_means_now() {
        assert_eq!(parse_duration("0s").unwrap(), Duration::zero());
        assert_eq!(parse_duration("0").unwrap(), Duration::zero());
        assert_eq!(parse_duration("0 minutes").unwrap(), Duration::zero());
    }

    #[test]
    fn relative_rejects_bad_input() {
        assert!(parse_duration("minutes").is_err());
        assert!(parse_duration("-5m").is_err());
        assert!(parse_duration("3 lightyears").is_err());
    }
}
