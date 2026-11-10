//! Calendar arithmetic with a bundled time-zone database for the plugin
//! clock interface, so every device computes the same local dates and
//! times. Schedules are timed by the scheduler crate.

use jiff::civil::{Date, Time};
use jiff::tz::{Disambiguation, TimeZone};
use jiff::{Timestamp, ToSpan};

pub(crate) fn zone(tz: &str) -> Result<TimeZone, String> {
    TimeZone::get(tz).map_err(|_| format!("unknown timezone `{tz}`"))
}

pub(crate) fn date(value: &str) -> Result<Date, String> {
    let parsed: Date = value
        .parse()
        .map_err(|_| format!("`{value}` is not a date like 2026-11-01"))?;
    if parsed.to_string() != value {
        return Err(format!("`{value}` is not a date like 2026-11-01"));
    }
    Ok(parsed)
}

pub(crate) fn time(value: &str) -> Result<Time, String> {
    let ok = value.len() == 5 && value.as_bytes()[2] == b':';
    let parsed: Option<Time> = ok.then(|| value.parse().ok()).flatten();
    parsed.ok_or_else(|| format!("`{value}` is not a time like 07:30"))
}

fn instant(ms: u64) -> Result<Timestamp, String> {
    Timestamp::from_millisecond(ms as i64).map_err(|e| e.to_string())
}

pub(crate) fn today(now_ms: u64, tz: &str) -> Result<String, String> {
    local_date(now_ms, tz)
}

/// Milliseconds to add to a UTC instant to get local wall-clock time.
pub(crate) fn utc_offset_ms(at_ms: u64, tz: &str) -> Result<i64, String> {
    Ok(i64::from(zone(tz)?.to_offset(instant(at_ms)?).seconds()) * 1000)
}

pub(crate) fn local_date(at_ms: u64, tz: &str) -> Result<String, String> {
    Ok(instant(at_ms)?.to_zoned(zone(tz)?).date().to_string())
}

/// Skipped (spring-forward) times move later; repeated (fall-back) times
/// take the earlier instant.
pub(crate) fn local_to_utc(day: &str, at: &str, tz: &str) -> Result<u64, String> {
    let civil = date(day)?.to_datetime(time(at)?);
    let zoned = zone(tz)?
        .to_ambiguous_zoned(civil)
        .disambiguate(Disambiguation::Compatible)
        .map_err(|e| e.to_string())?;
    Ok(zoned.timestamp().as_millisecond() as u64)
}

pub(crate) fn weekday(day: &str) -> Result<u8, String> {
    Ok(date(day)?.weekday().to_monday_zero_offset() as u8)
}

pub(crate) fn add_days(day: &str, days: i32) -> Result<String, String> {
    let d = date(day)?;
    d.checked_add(days.days())
        .map(|d| d.to_string())
        .map_err(|e| e.to_string())
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

pub(crate) fn weekday_name(name: &str) -> Option<u8> {
    DAYS.iter().position(|d| *d == name).map(|i| i as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KOLKATA: &str = "Asia/Kolkata";
    const NEW_YORK: &str = "America/New_York";

    fn utc(s: &str) -> u64 {
        s.parse::<Timestamp>().unwrap().as_millisecond() as u64
    }

    #[test]
    fn local_dates_and_times() {
        // 20:00 UTC on 1 Nov is 01:30 on 2 Nov in Kolkata (UTC+5:30).
        assert_eq!(
            local_date(utc("2026-11-01T20:00:00Z"), KOLKATA).unwrap(),
            "2026-11-02"
        );
        assert_eq!(
            local_to_utc("2026-11-02", "07:00", KOLKATA).unwrap(),
            utc("2026-11-02T01:30:00Z")
        );
        assert_eq!(weekday("2026-11-01").unwrap(), 6, "1 Nov 2026 is a Sunday");
        assert_eq!(add_days("2026-02-27", 2).unwrap(), "2026-03-01");
        assert!(date("2026-2-1").is_err() && time("7:00").is_err() && zone("Mars/Base").is_err());
    }

    #[test]
    fn clock_changes_resolve_predictably() {
        // 8 Mar 2026: New York skips 02:00–03:00; 1 Nov 2026 repeats 01:00–02:00.
        assert_eq!(
            local_to_utc("2026-03-08", "02:30", NEW_YORK).unwrap(),
            utc("2026-03-08T07:30:00Z")
        );
        assert_eq!(
            local_to_utc("2026-11-01", "01:30", NEW_YORK).unwrap(),
            utc("2026-11-01T05:30:00Z")
        );
    }
}
