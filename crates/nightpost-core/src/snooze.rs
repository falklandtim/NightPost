//! Snooze presets (Mailspring-style) and a small "when" parser for the MCP server.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozePreset {
    LaterToday,
    Tonight,
    Tomorrow,
    ThisWeekend,
    NextWeek,
    NextMonth,
}

impl SnoozePreset {
    pub const ALL: [SnoozePreset; 6] = [
        SnoozePreset::LaterToday,
        SnoozePreset::Tonight,
        SnoozePreset::Tomorrow,
        SnoozePreset::ThisWeekend,
        SnoozePreset::NextWeek,
        SnoozePreset::NextMonth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SnoozePreset::LaterToday => "Later today",
            SnoozePreset::Tonight => "Tonight",
            SnoozePreset::Tomorrow => "Tomorrow",
            SnoozePreset::ThisWeekend => "This weekend",
            SnoozePreset::NextWeek => "Next week",
            SnoozePreset::NextMonth => "Next month",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            SnoozePreset::LaterToday => "🕑",
            SnoozePreset::Tonight => "🌙",
            SnoozePreset::Tomorrow => "☀",
            SnoozePreset::ThisWeekend => "🏖",
            SnoozePreset::NextWeek => "📅",
            SnoozePreset::NextMonth => "🗓",
        }
    }

    pub fn resolve(self, now: DateTime<Local>) -> DateTime<Utc> {
        let at = |days: i64, h: u32| -> DateTime<Utc> {
            let d = now.date_naive() + Duration::days(days);
            let t = d.and_time(NaiveTime::from_hms_opt(h, 0, 0).unwrap());
            Local.from_local_datetime(&t).earliest().unwrap_or(now).with_timezone(&Utc)
        };
        match self {
            SnoozePreset::LaterToday => (now + Duration::hours(3)).with_timezone(&Utc),
            SnoozePreset::Tonight => {
                if now.time() < NaiveTime::from_hms_opt(19, 0, 0).unwrap() { at(0, 20) } else { at(1, 20) }
            }
            SnoozePreset::Tomorrow => at(1, 9),
            SnoozePreset::ThisWeekend => {
                let wd = now.weekday().num_days_from_monday() as i64; // Mon=0
                let to_sat = (5 - wd).rem_euclid(7);
                at(if to_sat == 0 && now.time().hour_ge(9) { 7 } else { to_sat }, 9)
            }
            SnoozePreset::NextWeek => {
                let wd = now.weekday().num_days_from_monday() as i64;
                at(7 - wd, 9)
            }
            SnoozePreset::NextMonth => {
                let (y, m) = if now.month() == 12 { (now.year() + 1, 1) } else { (now.year(), now.month() + 1) };
                let d = chrono::NaiveDate::from_ymd_opt(y, m, 1).unwrap();
                let t = d.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap());
                Local.from_local_datetime(&t).earliest().unwrap_or(now).with_timezone(&Utc)
            }
        }
    }
}

trait HourGe {
    fn hour_ge(&self, h: u32) -> bool;
}
impl HourGe for NaiveTime {
    fn hour_ge(&self, h: u32) -> bool {
        use chrono::Timelike;
        self.hour() >= h
    }
}

/// Parse a snooze time: a preset name (`tomorrow`, `next week`, …), a
/// relative duration (`30m`, `2h`, `3d`, `1w`), a weekday (`monday`), or an
/// RFC 3339 / `YYYY-MM-DD HH:MM` timestamp in local time.
pub fn parse_when(input: &str, now: DateTime<Local>) -> Result<DateTime<Utc>> {
    let s = input.trim().to_ascii_lowercase();
    for p in SnoozePreset::ALL {
        if s == p.label().to_ascii_lowercase() || s.replace(['_', '-'], " ") == p.label().to_ascii_lowercase() {
            return Ok(p.resolve(now));
        }
    }
    if s == "weekend" {
        return Ok(SnoozePreset::ThisWeekend.resolve(now));
    }
    let s2 = s.trim_start_matches("in ").replace(' ', "");
    let (num, unit) = s2.split_at(s2.find(|c: char| !c.is_ascii_digit()).unwrap_or(s2.len()));
    if let (Ok(n), true) = (num.parse::<i64>(), unit.chars().all(|c| c.is_ascii_alphabetic())) {
        let d = match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(n),
            "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(n),
            "d" | "day" | "days" => Duration::days(n),
            "w" | "wk" | "week" | "weeks" => Duration::weeks(n),
            _ => return Err(anyhow!("unknown unit in {input:?}")),
        };
        return Ok((now + d).with_timezone(&Utc));
    }
    let weekdays = [
        ("monday", Weekday::Mon), ("tuesday", Weekday::Tue), ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu), ("friday", Weekday::Fri), ("saturday", Weekday::Sat), ("sunday", Weekday::Sun),
    ];
    if let Some((_, wd)) = weekdays.iter().find(|(n, _)| s == *n || s == n[..3]) {
        let cur = now.weekday().num_days_from_monday() as i64;
        let mut delta = (wd.num_days_from_monday() as i64 - cur).rem_euclid(7);
        if delta == 0 {
            delta = 7;
        }
        let d = now.date_naive() + Duration::days(delta);
        let t = d.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap());
        return Ok(Local.from_local_datetime(&t).earliest().unwrap_or(now).with_timezone(&Utc));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(input.trim()) {
        return Ok(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%d"] {
        let parsed = if fmt == "%Y-%m-%d" {
            chrono::NaiveDate::parse_from_str(input.trim(), fmt)
                .map(|d| d.and_time(NaiveTime::from_hms_opt(9, 0, 0).unwrap()))
        } else {
            chrono::NaiveDateTime::parse_from_str(input.trim(), fmt)
        };
        if let Ok(ndt) = parsed {
            if let Some(dt) = Local.from_local_datetime(&ndt).earliest() {
                return Ok(dt.with_timezone(&Utc));
            }
        }
    }
    Err(anyhow!(
        "couldn't understand snooze time {input:?}; try 'tomorrow', 'next week', '3h', 'friday' or '2026-11-20 09:00'"
    ))
}

/// "Tomorrow, 9:00" / "Sat 14 Nov, 9:00" style label.
pub fn describe(until: DateTime<Utc>) -> String {
    let l = until.with_timezone(&Local);
    let today = Local::now().date_naive();
    let d = l.date_naive();
    let day = if d == today {
        "Today".to_string()
    } else if d == today + Duration::days(1) {
        "Tomorrow".to_string()
    } else if (d - today).num_days() < 7 {
        l.format("%A").to_string()
    } else {
        l.format("%a %e %b").to_string()
    };
    format!("{day}, {}", l.format("%H:%M"))
}

/// The weeks of a month for a date picker, Monday first. Days outside the
/// month are `None`.
pub fn month_grid(year: i32, month: u32) -> Vec<[Option<chrono::NaiveDate>; 7]> {
    let Some(first) = chrono::NaiveDate::from_ymd_opt(year, month, 1) else { return vec![] };
    let mut weeks = vec![];
    let mut week = [None; 7];
    let mut col = first.weekday().num_days_from_monday() as usize;
    let mut d = first;
    while d.month() == month {
        week[col] = Some(d);
        col += 1;
        if col == 7 {
            weeks.push(week);
            week = [None; 7];
            col = 0;
        }
        d = d.succ_opt().expect("valid date");
    }
    if col > 0 {
        weeks.push(week);
    }
    weeks
}

/// The first day of the month `n` months after `first`'s.
pub fn add_months(first: chrono::NaiveDate, n: i32) -> chrono::NaiveDate {
    let m0 = first.year() * 12 + first.month0() as i32 + n;
    chrono::NaiveDate::from_ymd_opt(m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1, 1).unwrap_or(first)
}

/// A local wall-clock time on `day`. When a clock change skips that time,
/// the first valid moment after it is used.
pub fn at_local(day: chrono::NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Utc>> {
    let t = day.and_time(NaiveTime::from_hms_opt(hour, minute, 0)?);
    (0..=120)
        .step_by(30)
        .find_map(|extra| Local.from_local_datetime(&(t + Duration::minutes(extra))).earliest())
        .map(|l| l.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_months() {
        use chrono::NaiveDate;
        // November 2026 starts on a Sunday: five weeks, Monday first.
        let g = month_grid(2026, 11);
        assert_eq!(g.len(), 6);
        assert_eq!(g[0][6], NaiveDate::from_ymd_opt(2026, 11, 1));
        assert_eq!(g[0][0], None);
        assert_eq!(g[5][0], NaiveDate::from_ymd_opt(2026, 11, 30));
        assert_eq!(g.iter().flatten().flatten().count(), 30);
        // February 2027 starts on a Monday and fills four rows exactly.
        assert_eq!(month_grid(2027, 2).len(), 4);
        let nov = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        assert_eq!(add_months(nov, 2), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        assert_eq!(add_months(nov, -11), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
        let t = at_local(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(), 9, 30).unwrap().with_timezone(&Local);
        assert_eq!(t.format("%Y-%m-%d %H:%M").to_string(), "2026-12-01 09:30");
    }

    fn fri_10am() -> DateTime<Local> {
        // 2026-11-06 is a Friday.
        Local.with_ymd_and_hms(2026, 11, 6, 10, 0, 0).unwrap()
    }

    #[test]
    fn presets() {
        let now = fri_10am();
        let l = |p: SnoozePreset| p.resolve(now).with_timezone(&Local);
        assert_eq!(l(SnoozePreset::LaterToday).format("%H").to_string(), "13");
        assert_eq!(l(SnoozePreset::Tonight).format("%d %H").to_string(), "06 20");
        assert_eq!(l(SnoozePreset::Tomorrow).format("%d %H").to_string(), "07 09");
        assert_eq!(l(SnoozePreset::ThisWeekend).format("%a %H").to_string(), "Sat 09");
        assert_eq!(l(SnoozePreset::NextWeek).format("%a %d").to_string(), "Mon 09");
        assert_eq!(l(SnoozePreset::NextMonth).format("%m-%d").to_string(), "12-01");
    }

    #[test]
    fn parses_when() {
        let now = fri_10am();
        assert_eq!(parse_when("2h", now).unwrap(), (now + Duration::hours(2)).with_timezone(&Utc));
        assert_eq!(parse_when("in 3 days", now).unwrap(), (now + Duration::days(3)).with_timezone(&Utc));
        assert_eq!(parse_when("tuesday", now).unwrap().with_timezone(&Local).format("%a %d").to_string(), "Tue 10");
        assert_eq!(parse_when("next week", now).unwrap(), SnoozePreset::NextWeek.resolve(now));
        assert!(parse_when("2026-12-01 08:30", now).is_ok());
        assert!(parse_when("whenever", now).is_err());
    }
}
