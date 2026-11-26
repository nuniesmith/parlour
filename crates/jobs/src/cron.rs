//! Five-field cron expressions (POD2): when a podcast's scheduled
//! check is due (D32: per show, hourly by default, as a cron
//! expression because Audiobookshelf's schedule is one).
//!
//! Pure: no clock, no database. The fields are minute, hour, day of
//! month, month and day of week, read in UTC. Each field is a comma
//! list of `*`, `N`, `N-M`, each with an optional `/step` (`N/step`
//! runs from N to the field's end). Months and days of the week also
//! take three-letter English names (`JAN`, `MON`); day of week 0 and
//! 7 are both Sunday. As in the classic cron, when both day fields are
//! restricted a day matching **either** counts.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Timelike, Utc};

/// A parsed schedule: one bit per allowed value in each field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: u64,
    hours: u64,
    days_of_month: u64,
    months: u64,
    days_of_week: u64,
    /// The day-of-month field was a bare `*` (or `*/1`).
    any_day_of_month: bool,
    /// The day-of-week field was a bare `*` (or `*/1`).
    any_day_of_week: bool,
}

/// Why an expression was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronError(pub String);

impl std::fmt::Display for CronError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CronError {}

const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAY_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// How far ahead `next_after` looks before giving up (a schedule such
/// as `0 0 30 2 *` never fires).
const LOOKAHEAD_YEARS: i32 = 5;

struct Field {
    name: &'static str,
    min: u32,
    max: u32,
    names: &'static [&'static str],
    /// The value the first name stands for (`jan` = 1, `sun` = 0).
    names_from: u32,
}

const FIELDS: [Field; 5] = [
    Field {
        name: "minute",
        min: 0,
        max: 59,
        names: &[],
        names_from: 0,
    },
    Field {
        name: "hour",
        min: 0,
        max: 23,
        names: &[],
        names_from: 0,
    },
    Field {
        name: "day of month",
        min: 1,
        max: 31,
        names: &[],
        names_from: 0,
    },
    Field {
        name: "month",
        min: 1,
        max: 12,
        names: &MONTH_NAMES,
        names_from: 1,
    },
    Field {
        name: "day of week",
        min: 0,
        max: 7,
        names: &DAY_NAMES,
        names_from: 0,
    },
];

impl Field {
    fn value(&self, text: &str) -> Result<u32, CronError> {
        let lower = text.to_ascii_lowercase();
        if let Some(i) = self.names.iter().position(|n| *n == lower) {
            return Ok(self.names_from + i as u32);
        }
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CronError(format!(
                "{} field: {text:?} is not a number",
                self.name
            )));
        }
        let v: u32 = text
            .parse()
            .map_err(|_| CronError(format!("{} field: {text:?} is too large", self.name)))?;
        if v < self.min || v > self.max {
            return Err(CronError(format!(
                "{} field: {v} is outside {}–{}",
                self.name, self.min, self.max
            )));
        }
        Ok(v)
    }

    /// The bit set for one field's text, and whether it was "any".
    fn parse(&self, text: &str) -> Result<(u64, bool), CronError> {
        let mut bits = 0u64;
        let mut any = false;
        for item in text.split(',') {
            let (range, step) = match item.split_once('/') {
                Some((r, s)) => {
                    let step: u32 =
                        s.parse().ok().filter(|s| *s > 0).ok_or_else(|| {
                            CronError(format!("{} field: bad step {s:?}", self.name))
                        })?;
                    (r, Some(step))
                }
                None => (item, None),
            };
            let (lo, hi) = if range == "*" {
                if step.unwrap_or(1) == 1 {
                    any = true;
                }
                (self.min, self.max)
            } else if let Some((a, b)) = range.split_once('-') {
                let (a, b) = (self.value(a)?, self.value(b)?);
                if a > b {
                    return Err(CronError(format!(
                        "{} field: range {range:?} runs backwards",
                        self.name
                    )));
                }
                (a, b)
            } else {
                let v = self.value(range)?;
                // `N/step` runs from N to the end of the field.
                (v, if step.is_some() { self.max } else { v })
            };
            let step = step.unwrap_or(1);
            let mut v = lo;
            while v <= hi {
                bits |= 1 << v;
                v += step;
            }
        }
        Ok((bits, any))
    }
}

impl CronSchedule {
    /// Parse a five-field expression. Anything else (six fields,
    /// `@hourly`, an empty string) is refused.
    pub fn parse(expr: &str) -> Result<CronSchedule, CronError> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(CronError(format!(
                "a schedule has five fields (minute hour day month weekday), not {}",
                parts.len()
            )));
        }
        let (minutes, _) = FIELDS[0].parse(parts[0])?;
        let (hours, _) = FIELDS[1].parse(parts[1])?;
        let (days_of_month, any_day_of_month) = FIELDS[2].parse(parts[2])?;
        let (months, _) = FIELDS[3].parse(parts[3])?;
        let (mut days_of_week, any_day_of_week) = FIELDS[4].parse(parts[4])?;
        // 7 is Sunday as well as 0.
        if days_of_week & (1 << 7) != 0 {
            days_of_week = (days_of_week | 1) & !(1 << 7);
        }
        Ok(CronSchedule {
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            any_day_of_month,
            any_day_of_week,
        })
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let dom = self.days_of_month & (1 << date.day()) != 0;
        let dow = self.days_of_week & (1 << date.weekday().num_days_from_sunday()) != 0;
        match (self.any_day_of_month, self.any_day_of_week) {
            (true, true) => true,
            (true, false) => dow,
            (false, true) => dom,
            (false, false) => dom || dow,
        }
    }

    /// Does the schedule fire at this minute?
    pub fn matches(&self, t: DateTime<Utc>) -> bool {
        self.minutes & (1 << t.minute()) != 0
            && self.hours & (1 << t.hour()) != 0
            && self.months & (1 << t.month()) != 0
            && self.day_matches(t.date_naive())
    }

    /// The first minute strictly after `t` at which the schedule
    /// fires, or `None` if it never does within five years.
    pub fn next_after(&self, t: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let start = t.naive_utc();
        let mut cur: NaiveDateTime =
            start.date().and_hms_opt(start.hour(), start.minute(), 0)? + Duration::minutes(1);
        let limit = start.year() + LOOKAHEAD_YEARS;
        while cur.year() <= limit {
            if self.months & (1 << cur.month()) == 0 {
                let (y, m) = if cur.month() == 12 {
                    (cur.year() + 1, 1)
                } else {
                    (cur.year(), cur.month() + 1)
                };
                cur = NaiveDate::from_ymd_opt(y, m, 1)?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if !self.day_matches(cur.date()) {
                cur = cur.date().succ_opt()?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if self.hours & (1 << cur.hour()) == 0 {
                cur = cur.date().and_hms_opt(cur.hour(), 0, 0)? + Duration::hours(1);
                continue;
            }
            if self.minutes & (1 << cur.minute()) == 0 {
                cur += Duration::minutes(1);
                continue;
            }
            return Some(cur.and_utc());
        }
        None
    }

    /// Is a check due now? It is when the schedule has fired since
    /// the last check — or there has never been one. Missed firings
    /// (the server was down) add up to one check, not several.
    pub fn is_due(&self, last_checked: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
        match last_checked {
            None => true,
            Some(last) => self.next_after(last).is_some_and(|next| next <= now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn hourly_default_fires_on_the_hour() {
        let c = CronSchedule::parse("0 * * * *").unwrap();
        assert_eq!(
            c.next_after(at(2026, 11, 26, 10, 0, 30)),
            Some(at(2026, 11, 26, 11, 0, 0))
        );
        assert_eq!(
            c.next_after(at(2026, 11, 26, 10, 59, 59)),
            Some(at(2026, 11, 26, 11, 0, 0))
        );
        // Exactly on a firing: the next one, not the same minute.
        assert_eq!(
            c.next_after(at(2026, 11, 26, 11, 0, 0)),
            Some(at(2026, 11, 26, 12, 0, 0))
        );
        // Across midnight and the year end.
        assert_eq!(
            c.next_after(at(2026, 12, 31, 23, 5, 0)),
            Some(at(2027, 1, 1, 0, 0, 0))
        );
    }

    #[test]
    fn due_when_the_schedule_fired_since_the_last_check() {
        let c = CronSchedule::parse("0 * * * *").unwrap();
        let checked = at(2026, 11, 26, 10, 0, 4);
        assert!(!c.is_due(Some(checked), at(2026, 11, 26, 10, 59, 0)));
        assert!(c.is_due(Some(checked), at(2026, 11, 26, 11, 0, 0)));
        assert!(c.is_due(Some(checked), at(2026, 11, 29, 3, 0, 0)));
        assert!(c.is_due(None, at(2026, 11, 26, 10, 1, 0)), "never checked");
    }

    #[test]
    fn lists_ranges_steps_and_names() {
        let c = CronSchedule::parse("*/15 9-17 * * MON-fri").unwrap();
        // Thursday 2026-11-26.
        assert!(c.matches(at(2026, 11, 26, 9, 45, 0)));
        assert!(!c.matches(at(2026, 11, 26, 9, 50, 0)));
        assert!(!c.matches(at(2026, 11, 26, 18, 0, 0)));
        // Saturday → Monday 09:00.
        assert_eq!(
            c.next_after(at(2026, 11, 28, 12, 0, 0)),
            Some(at(2026, 11, 30, 9, 0, 0))
        );
        let c = CronSchedule::parse("5,35 6/6 1 jan,Jul *").unwrap();
        assert_eq!(
            c.next_after(at(2026, 11, 26, 0, 0, 0)),
            Some(at(2027, 1, 1, 6, 5, 0))
        );
        assert!(c.matches(at(2027, 7, 1, 18, 35, 0)));
        assert!(!c.matches(at(2027, 7, 1, 3, 35, 0)));
    }

    #[test]
    fn both_day_fields_restricted_means_either() {
        // The 1st of the month, or any Sunday.
        let c = CronSchedule::parse("0 0 1 * 0").unwrap();
        assert!(c.matches(at(2026, 12, 1, 0, 0, 0)), "a Tuesday the 1st");
        assert!(c.matches(at(2026, 11, 29, 0, 0, 0)), "a Sunday");
        assert!(!c.matches(at(2026, 11, 30, 0, 0, 0)));
        // 7 is Sunday too.
        let c = CronSchedule::parse("0 0 * * 7").unwrap();
        assert!(c.matches(at(2026, 11, 29, 0, 0, 0)));
    }

    #[test]
    fn impossible_dates_never_fire() {
        let c = CronSchedule::parse("0 0 30 2 *").unwrap();
        assert_eq!(c.next_after(at(2026, 1, 1, 0, 0, 0)), None);
        assert!(!c.is_due(Some(at(2026, 1, 1, 0, 0, 0)), at(2036, 1, 1, 0, 0, 0)));
    }

    #[test]
    fn leap_day_is_found() {
        let c = CronSchedule::parse("0 12 29 2 *").unwrap();
        assert_eq!(
            c.next_after(at(2026, 3, 1, 0, 0, 0)),
            Some(at(2028, 2, 29, 12, 0, 0))
        );
    }

    #[test]
    fn bad_expressions_are_refused() {
        for bad in [
            "",
            "@hourly",
            "0 * * *",
            "0 0 * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "-1 * * * *",
            "1,,2 * * * *",
            "* * * smarch *",
        ] {
            assert!(
                CronSchedule::parse(bad).is_err(),
                "{bad:?} should be refused"
            );
        }
    }
}
