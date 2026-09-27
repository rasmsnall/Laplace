//! when a flow is due: a cron schedule, optionally limited to swedish or finnish business days

use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use croner::Cron;
use croner::parser::CronParser;
use serde::Deserialize;

use crate::holidays::{self, Country};

/// how many non-business days in a row a search may skip, e.g. a long christmas
const MAX_SKIPPED_DAYS: usize = 400;

#[derive(Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum CronStyle {
    /// five fields, sunday is 0, as in crontab: `0 7 * * 1-5`
    #[default]
    Unix,
    /// seconds first, sunday is 1, as databricks stores it: `0 0 7 ? * MON-FRI`
    Quartz,
}

pub struct Schedule {
    cron: Cron,
    zone: Tz,
    calendar: Vec<Country>,
}

pub enum Due {
    /// no deadline has passed yet
    NotYet,
    Met {
        next_deadline: Option<DateTime<Utc>>,
    },
    Missed {
        deadline: DateTime<Utc>,
    },
}

impl Schedule {
    pub fn parse(cron: &str, style: CronStyle, zone: Tz, calendar: &[String]) -> Result<Self> {
        let cron = match style {
            CronStyle::Unix => Cron::from_str(cron),
            CronStyle::Quartz => CronParser::builder()
                .alternative_weekdays(true)
                .build()
                .parse(cron),
        }
        .with_context(|| format!("cron {cron:?}"))?;
        let calendar = calendar
            .iter()
            .map(|code| {
                Country::parse(code).with_context(|| format!("calendar {code:?}: use se or fi"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            cron,
            zone,
            calendar,
        })
    }

    pub fn zone(name: Option<&str>, default: Tz) -> Result<Tz> {
        match name {
            Some(name) => name
                .parse()
                .map_err(|_| anyhow::anyhow!("timezone {name:?} is not known")),
            None => Ok(default),
        }
    }

    /// the latest deadline that has passed is met when a success came after the one before it
    pub fn check(
        &self,
        last_ok: DateTime<Utc>,
        grace: Duration,
        now: DateTime<Utc>,
    ) -> Result<Due> {
        let grace = TimeDelta::from_std(grace)?;
        let Some(due) = self.at_or_before((now - grace).with_timezone(&self.zone))? else {
            return Ok(Due::NotYet);
        };
        let window_opened = self.before(due)?.map(|previous| previous + grace);

        if window_opened.is_none_or(|opened| last_ok > opened) {
            let next = self
                .after(now.with_timezone(&self.zone))?
                .map(|next| next + grace);
            Ok(Due::Met {
                next_deadline: next,
            })
        } else {
            Ok(Due::Missed {
                deadline: due + grace,
            })
        }
    }

    /// the first occurrence after `now`
    pub fn next(&self, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
        self.after(now.with_timezone(&self.zone))
    }

    /// the last occurrence at or before `now`
    pub fn latest(&self, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
        self.at_or_before(now.with_timezone(&self.zone))
    }

    pub fn label(&self) -> String {
        let when = self
            .cron
            .describe()
            .to_lowercase()
            .trim_end_matches('.')
            .replace(
                ", on monday, tuesday, wednesday, thursday, and friday",
                " on weekdays",
            )
            .replace(", on saturday and sunday", " on weekends");
        let calendar: Vec<&str> = self.calendar.iter().map(|country| country.code()).collect();
        if calendar.is_empty() {
            when
        } else {
            format!("{when}, {} business days", calendar.join("+"))
        }
    }

    pub fn local(&self, time: DateTime<Utc>) -> String {
        time.with_timezone(&self.zone)
            .format("%a %d %b %H:%M %Z")
            .to_string()
            .to_lowercase()
    }

    fn counts(&self, time: &DateTime<Tz>) -> bool {
        self.calendar.is_empty() || holidays::is_business_day(time.date_naive(), &self.calendar)
    }

    fn at_or_before(&self, time: DateTime<Tz>) -> Result<Option<DateTime<Utc>>> {
        let first = self.cron.find_previous_occurrence(&time, true)?;
        self.walk(first, |t| self.cron.find_previous_occurrence(t, false))
    }

    fn before(&self, time: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
        let first = self
            .cron
            .find_previous_occurrence(&time.with_timezone(&self.zone), false)?;
        self.walk(first, |t| self.cron.find_previous_occurrence(t, false))
    }

    fn after(&self, time: DateTime<Tz>) -> Result<Option<DateTime<Utc>>> {
        let first = self.cron.find_next_occurrence(&time, false)?;
        self.walk(first, |t| self.cron.find_next_occurrence(t, false))
    }

    /// steps from occurrence to occurrence until one falls on a day that counts
    fn walk(
        &self,
        mut time: DateTime<Tz>,
        step: impl Fn(&DateTime<Tz>) -> Result<DateTime<Tz>, croner::errors::CronError>,
    ) -> Result<Option<DateTime<Utc>>> {
        for _ in 0..MAX_SKIPPED_DAYS {
            if self.counts(&time) {
                return Ok(Some(time.with_timezone(&Utc)));
            }
            time = step(&time)?;
        }
        bail!("no business day found within {MAX_SKIPPED_DAYS} occurrences")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn stockholm(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        chrono_tz::Europe::Stockholm
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn weekday_file() -> Schedule {
        // a partner file due by 07:00 on swedish business days
        Schedule::parse(
            "0 7 * * 1-5",
            CronStyle::Unix,
            chrono_tz::Europe::Stockholm,
            &["se".into()],
        )
        .unwrap()
    }

    const GRACE: Duration = Duration::from_secs(30 * 60);

    #[test]
    fn met_when_the_file_came_after_the_previous_deadline() {
        let due = weekday_file()
            .check(
                stockholm(2026, 9, 25, 6, 40),
                GRACE,
                stockholm(2026, 9, 25, 9, 0),
            )
            .unwrap();
        assert!(matches!(due, Due::Met { .. }));
    }

    #[test]
    fn missed_when_the_last_file_is_from_the_day_before() {
        let due = weekday_file()
            .check(
                stockholm(2026, 9, 24, 6, 40),
                GRACE,
                stockholm(2026, 9, 25, 9, 0),
            )
            .unwrap();
        let Due::Missed { deadline } = due else {
            panic!("should be missed")
        };
        assert_eq!(deadline, stockholm(2026, 9, 25, 7, 30));
    }

    #[test]
    fn a_weekend_is_not_late() {
        // friday's file arrived; on sunday nothing new is expected
        let due = weekday_file()
            .check(
                stockholm(2026, 9, 25, 6, 40),
                GRACE,
                stockholm(2026, 9, 27, 12, 0),
            )
            .unwrap();
        assert!(matches!(due, Due::Met { .. }));
    }

    #[test]
    fn a_holiday_is_not_late_but_the_next_business_day_is() {
        // midsummer eve 2026 is friday june 19; thursday's file is enough until monday
        let schedule = weekday_file();
        let thursday = stockholm(2026, 6, 18, 6, 50);
        assert!(matches!(
            schedule
                .check(thursday, GRACE, stockholm(2026, 6, 19, 12, 0))
                .unwrap(),
            Due::Met { .. }
        ));
        assert!(matches!(
            schedule
                .check(thursday, GRACE, stockholm(2026, 6, 22, 8, 0))
                .unwrap(),
            Due::Missed { .. }
        ));
    }

    #[test]
    fn databricks_quartz_schedules_parse() {
        let nightly = Schedule::parse(
            "0 0 2 * * ?",
            CronStyle::Quartz,
            chrono_tz::Europe::Stockholm,
            &[],
        )
        .unwrap();
        let ran = stockholm(2026, 9, 25, 2, 20);
        assert!(matches!(
            nightly
                .check(ran, GRACE, stockholm(2026, 9, 25, 12, 0))
                .unwrap(),
            Due::Met { .. }
        ));
        assert!(matches!(
            nightly
                .check(ran, GRACE, stockholm(2026, 9, 26, 3, 0))
                .unwrap(),
            Due::Missed { .. }
        ));

        // quartz counts sunday as 1, so 2-6 is monday to friday
        let weekdays = Schedule::parse(
            "0 30 6 ? * 2-6",
            CronStyle::Quartz,
            chrono_tz::Europe::Helsinki,
            &[],
        )
        .unwrap();
        let friday = chrono_tz::Europe::Helsinki
            .with_ymd_and_hms(2026, 9, 25, 6, 45, 0)
            .unwrap()
            .with_timezone(&Utc);
        let saturday_noon = chrono_tz::Europe::Helsinki
            .with_ymd_and_hms(2026, 9, 26, 12, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        assert!(matches!(
            weekdays.check(friday, GRACE, saturday_noon).unwrap(),
            Due::Met { .. }
        ));
    }

    #[test]
    fn labels_read_like_a_sentence() {
        assert_eq!(
            weekday_file().label(),
            "at 07:00 on weekdays, se business days"
        );
    }

    #[test]
    fn rejects_unknown_calendars() {
        assert!(
            Schedule::parse("0 7 * * *", CronStyle::Unix, chrono_tz::UTC, &["no".into()]).is_err()
        );
    }
}
