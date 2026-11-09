//! When a schedule runs: its repeat rule and the slot arithmetic, in a
//! time zone, using the bundled tz database so every device agrees.

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// `{"every": "minutes", "minutes": 60}`, `{"every": "day", "at": "07:00"}`,
/// `{"every": "week", "days": ["mon", "fri"], "at": "18:00"}` or
/// `{"every": "month", "day": 1, "at": "09:00"}` (`"day": "last"` for the
/// last day; a day past the month's end means its last day).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "every", rename_all = "snake_case", deny_unknown_fields)]
pub enum Repeat {
    Minutes { minutes: u32 },
    Day { at: String },
    Week { days: Vec<String>, at: String },
    Month { day: MonthDay, at: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MonthDay {
    Day(u8),
    Last(Last),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Last {
    Last,
}

/// Where a schedule may run. Phones run schedules only while the app is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunOn {
    #[default]
    Any,
    Desktop,
}

fn bad(message: impl Into<String>) -> String {
    message.into()
}

fn parse_time(at: &str) -> Result<Time, String> {
    let (h, m) = at
        .split_once(':')
        .ok_or_else(|| bad(format!("time `{at}` must be HH:MM")))?;
    let h: i8 = h
        .parse()
        .map_err(|_| bad(format!("time `{at}` must be HH:MM")))?;
    let m: i8 = m
        .parse()
        .map_err(|_| bad(format!("time `{at}` must be HH:MM")))?;
    Time::new(h, m, 0, 0).map_err(|_| bad(format!("time `{at}` must be HH:MM")))
}

fn zone(tz: &str) -> Result<TimeZone, String> {
    TimeZone::get(tz).map_err(|_| bad(format!("unknown time zone `{tz}`")))
}

fn local_ms(date: Date, time: Time, tz: &TimeZone) -> Result<u64, String> {
    let zoned = date
        .to_datetime(time)
        .to_zoned(tz.clone())
        .map_err(|e| e.to_string())?;
    Ok(zoned.timestamp().as_millisecond().max(0) as u64)
}

fn local_date(ms: u64, tz: &TimeZone) -> Result<Date, String> {
    let ts = Timestamp::from_millisecond(ms as i64).map_err(|e| e.to_string())?;
    Ok(ts.to_zoned(tz.clone()).date())
}

impl Repeat {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Repeat::Minutes { minutes } => {
                if !(5..=60 * 24 * 7).contains(minutes) {
                    return Err("`minutes` must be between 5 and 10080".into());
                }
            }
            Repeat::Day { at } => {
                parse_time(at)?;
            }
            Repeat::Week { days, at } => {
                parse_time(at)?;
                if days.is_empty() {
                    return Err("pick at least one weekday".into());
                }
                if let Some(d) = days.iter().find(|d| !DAYS.contains(&d.as_str())) {
                    return Err(format!("unknown weekday `{d}`; use mon … sun"));
                }
            }
            Repeat::Month { day, at } => {
                parse_time(at)?;
                if let MonthDay::Day(d) = day {
                    if !(1..=31).contains(d) {
                        return Err("`day` must be 1–31 or \"last\"".into());
                    }
                }
            }
        }
        Ok(())
    }

    fn matches(&self, date: Date) -> bool {
        match self {
            Repeat::Minutes { .. } | Repeat::Day { .. } => true,
            Repeat::Week { days, .. } => {
                let index = date.weekday().to_monday_zero_offset() as usize;
                days.iter().any(|d| d == DAYS[index])
            }
            Repeat::Month { day, .. } => {
                let last = date.days_in_month();
                let want = match day {
                    MonthDay::Day(d) => (*d as i8).min(last),
                    MonthDay::Last(_) => last,
                };
                date.day() == want
            }
        }
    }

    fn at(&self) -> Option<&str> {
        match self {
            Repeat::Minutes { .. } => None,
            Repeat::Day { at } | Repeat::Week { at, .. } | Repeat::Month { at, .. } => Some(at),
        }
    }

    /// The latest slot at or before `now_ms`. `anchor_ms` (the schedule's
    /// creation) starts the minute grid.
    pub fn latest(&self, now_ms: u64, anchor_ms: u64, tz: &str) -> Result<Option<u64>, String> {
        if let Repeat::Minutes { minutes } = self {
            let interval = u64::from(*minutes) * 60_000;
            if now_ms < anchor_ms + interval {
                return Ok(None);
            }
            return Ok(Some(anchor_ms + (now_ms - anchor_ms) / interval * interval));
        }
        let tz = zone(tz)?;
        let time = parse_time(self.at().unwrap_or("00:00"))?;
        let mut day = local_date(now_ms, &tz)?;
        for _ in 0..400 {
            if self.matches(day) {
                let slot = local_ms(day, time, &tz)?;
                if slot <= now_ms {
                    return Ok(Some(slot));
                }
            }
            day = day.yesterday().map_err(|e| e.to_string())?;
        }
        Ok(None)
    }

    /// The first slot after `after_ms`.
    pub fn next(&self, after_ms: u64, anchor_ms: u64, tz: &str) -> Result<Option<u64>, String> {
        if let Repeat::Minutes { minutes } = self {
            let interval = u64::from(*minutes) * 60_000;
            if after_ms < anchor_ms {
                return Ok(Some(anchor_ms + interval));
            }
            return Ok(Some(
                anchor_ms + ((after_ms - anchor_ms) / interval + 1) * interval,
            ));
        }
        let tz = zone(tz)?;
        let time = parse_time(self.at().unwrap_or("00:00"))?;
        let mut day = local_date(after_ms, &tz)?;
        for _ in 0..400 {
            if self.matches(day) {
                let slot = local_ms(day, time, &tz)?;
                if slot > after_ms {
                    return Ok(Some(slot));
                }
            }
            day = day.tomorrow().map_err(|e| e.to_string())?;
        }
        Ok(None)
    }

    /// Plain words, e.g. "Every Mon, Fri at 18:00".
    pub fn describe(&self) -> String {
        match self {
            Repeat::Minutes { minutes } if minutes % 60 == 0 => match minutes / 60 {
                1 => "Every hour".into(),
                h => format!("Every {h} hours"),
            },
            Repeat::Minutes { minutes } => format!("Every {minutes} minutes"),
            Repeat::Day { at } => format!("Every day at {at}"),
            Repeat::Week { days, at } => {
                let mut names: Vec<&str> = DAYS
                    .iter()
                    .filter(|d| days.iter().any(|x| x == *d))
                    .copied()
                    .collect();
                if names.len() == 7 {
                    return format!("Every day at {at}");
                }
                let label: Vec<String> = names
                    .drain(..)
                    .map(|d| {
                        let mut c = d.chars();
                        c.next()
                            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                            .unwrap_or_default()
                    })
                    .collect();
                format!("Every {} at {at}", label.join(", "))
            }
            Repeat::Month { day, at } => match day {
                MonthDay::Last(_) => format!("Last day of every month at {at}"),
                MonthDay::Day(d) => {
                    let suffix = match (d % 10, d % 100) {
                        (1, x) if x != 11 => "st",
                        (2, x) if x != 12 => "nd",
                        (3, x) if x != 13 => "rd",
                        _ => "th",
                    };
                    format!("Every month on the {d}{suffix} at {at}")
                }
            },
        }
    }
}

/// Local `YYYY-MM-DD` and `HH:MM` of an instant.
pub fn local_parts(ms: u64, tz: &str) -> Result<(String, String), String> {
    let tz = zone(tz)?;
    let ts = Timestamp::from_millisecond(ms as i64).map_err(|e| e.to_string())?;
    let z = ts.to_zoned(tz);
    Ok((
        z.date().to_string(),
        format!("{:02}:{:02}", z.hour(), z.minute()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: &str = "Asia/Kolkata";
    const NY: &str = "America/New_York";

    fn at(date: &str, time: &str, tz: &str) -> u64 {
        let d: Date = date.parse().unwrap();
        local_ms(d, parse_time(time).unwrap(), &zone(tz).unwrap()).unwrap()
    }

    fn week(days: &[&str], t: &str) -> Repeat {
        Repeat::Week {
            days: days.iter().map(|d| d.to_string()).collect(),
            at: t.into(),
        }
    }

    #[test]
    fn weekly_latest_and_next() {
        // 2026-11-01 is a Sunday.
        let r = week(&["sun"], "18:00");
        let now = at("2026-11-03", "10:00", TZ);
        assert_eq!(
            r.latest(now, 0, TZ).unwrap(),
            Some(at("2026-11-01", "18:00", TZ))
        );
        assert_eq!(
            r.next(now, 0, TZ).unwrap(),
            Some(at("2026-11-08", "18:00", TZ))
        );
        let two = week(&["mon", "fri"], "09:30");
        assert_eq!(
            two.latest(now, 0, TZ).unwrap(),
            Some(at("2026-11-02", "09:30", TZ))
        );
        assert_eq!(two.describe(), "Every Mon, Fri at 09:30");
    }

    #[test]
    fn monthly_clamps_to_the_months_end() {
        let r = Repeat::Month {
            day: MonthDay::Day(31),
            at: "09:00".into(),
        };
        let now = at("2027-03-05", "00:00", TZ);
        assert_eq!(
            r.latest(now, 0, TZ).unwrap(),
            Some(at("2027-02-28", "09:00", TZ))
        );
        let last: Repeat =
            serde_json::from_str(r#"{"every":"month","day":"last","at":"20:00"}"#).unwrap();
        assert_eq!(
            last.next(now, 0, TZ).unwrap(),
            Some(at("2027-03-31", "20:00", TZ))
        );
        assert_eq!(last.describe(), "Last day of every month at 20:00");
    }

    #[test]
    fn daily_across_a_clock_change() {
        let r = Repeat::Day { at: "02:30".into() };
        // 2026-03-08: New York skips 02:00–03:00; the slot moves forward.
        let slot = r
            .next(at("2026-03-07", "12:00", NY), 0, NY)
            .unwrap()
            .unwrap();
        assert_eq!(
            local_parts(slot, NY).unwrap(),
            ("2026-03-08".into(), "03:30".into())
        );
    }

    #[test]
    fn minutes_grid_from_the_anchor() {
        let r = Repeat::Minutes { minutes: 60 };
        let anchor = 1_000_000;
        assert_eq!(r.latest(anchor + 30 * 60_000, anchor, TZ).unwrap(), None);
        assert_eq!(
            r.latest(anchor + 150 * 60_000, anchor, TZ).unwrap(),
            Some(anchor + 120 * 60_000)
        );
        assert_eq!(
            r.next(anchor + 150 * 60_000, anchor, TZ).unwrap(),
            Some(anchor + 180 * 60_000)
        );
    }

    #[test]
    fn validation() {
        assert!(week(&[], "18:00").validate().is_err());
        assert!(week(&["sunday"], "18:00").validate().is_err());
        assert!(Repeat::Day { at: "25:00".into() }.validate().is_err());
        assert!(Repeat::Minutes { minutes: 1 }.validate().is_err());
        assert!(
            serde_json::from_str::<Repeat>(r#"{"every":"month","day":"first","at":"1:00"}"#)
                .is_err()
        );
    }
}
