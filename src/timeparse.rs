//! Parsing of the user-supplied "when" — either an absolute date/time or a
//! relative duration from now. Both resolve to an absolute local timestamp.

use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone};

/// Parse a complete date and time in the local timezone.
///
/// Accepts `YYYY-MM-DD HH:MM` and `YYYY-MM-DD HH:MM:SS`.
pub fn parse_absolute(input: &str) -> Result<DateTime<Local>> {
    let input = input.trim();
    const FORMATS: &[&str] = &["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"];

    for fmt in FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(input, fmt) {
            return match Local.from_local_datetime(&naive).single() {
                Some(dt) => Ok(dt),
                // Ambiguous/nonexistent local time (DST edge). Fall back to the
                // earliest interpretation rather than failing outright.
                None => Local
                    .from_local_datetime(&naive)
                    .earliest()
                    .ok_or_else(|| anyhow::anyhow!("invalid local time: {input}")),
            };
        }
    }
    bail!("expected `YYYY-MM-DD HH:MM[:SS]`");
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
///   `3 minutes`, `3 min`, `3m`, `90s`, `2 hours`, `1 day`.
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
    if amount <= 0 {
        bail!("duration must be positive");
    }

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
    fn relative_rejects_bad_input() {
        assert!(parse_duration("minutes").is_err());
        assert!(parse_duration("0 minutes").is_err());
        assert!(parse_duration("3 lightyears").is_err());
    }
}
