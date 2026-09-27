use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Days, TimeDelta, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use sqlx::PgPool;

use crate::config::Flow;

#[derive(Serialize)]
pub struct Report {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub flows: Vec<FlowReport>,
}

#[derive(Serialize)]
pub struct FlowReport {
    pub id: String,
    pub kind: &'static str,
    pub owner: Option<String>,
    /// share of the measured time the flow was ok or only warned, none before it first reported
    pub uptime: Option<f64>,
    pub target: Option<f64>,
    pub met: Option<bool>,
    pub incidents: usize,
    pub longest_outage_seconds: i64,
    pub mean_recovery_seconds: Option<i64>,
    pub runs_ok: i64,
    pub runs_failed: i64,
    /// uptime of each day of the period, none for a day with nothing measured or still to come
    pub days: Vec<Option<f64>>,
}

#[derive(sqlx::FromRow, Clone)]
struct Change {
    flow: String,
    at: DateTime<Utc>,
    state: String,
}

/// time in a state counts as up when data was flowing
fn is_up(state: &str) -> bool {
    matches!(state, "ok" | "warning")
}

/// before a flow first reports there is nothing to hold it to, and planned maintenance is not held against it
fn is_measured(state: &str) -> bool {
    !matches!(state, "pending" | "maintenance")
}

pub async fn build(
    pool: &PgPool,
    flows: &[Flow],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    zone: Tz,
) -> Result<Report> {
    let until = to.min(Utc::now());

    // the state each flow was in when the period began, then every change inside it
    let changes: Vec<Change> = sqlx::query_as(
        "(select distinct on (flow) flow, $1::timestamptz as at, state from state_changes
          where at < $1 order by flow, at desc)
         union all
         (select flow, at, state from state_changes where at >= $1 and at < $2)
         order by flow, at",
    )
    .bind(from)
    .bind(until)
    .fetch_all(pool)
    .await?;
    let mut by_flow: HashMap<String, Vec<Change>> = HashMap::new();
    for change in changes {
        by_flow.entry(change.flow.clone()).or_default().push(change);
    }

    let runs: Vec<(String, i64, i64)> = sqlx::query_as(
        "select flow, count(*) filter (where outcome = 'ok'), count(*) filter (where outcome = 'fail')
         from events where at >= $1 and at < $2 group by flow",
    )
    .bind(from)
    .bind(until)
    .fetch_all(pool)
    .await?;
    let runs: HashMap<String, (i64, i64)> = runs
        .into_iter()
        .map(|(flow, ok, failed)| (flow, (ok, failed)))
        .collect();

    let flows = flows
        .iter()
        .map(|flow| {
            let changes = by_flow.get(&flow.id).map(Vec::as_slice).unwrap_or_default();
            let summary = summarize(changes, until);
            let (runs_ok, runs_failed) = runs.get(&flow.id).copied().unwrap_or_default();
            FlowReport {
                id: flow.id.clone(),
                kind: flow.kind.name(),
                owner: flow.owner.clone(),
                uptime: summary.uptime,
                target: flow.sla,
                met: flow
                    .sla
                    .zip(summary.uptime)
                    .map(|(target, uptime)| uptime >= target),
                incidents: summary.incidents,
                longest_outage_seconds: summary.longest_outage.num_seconds(),
                mean_recovery_seconds: summary.mean_recovery.map(|d| d.num_seconds()),
                runs_ok,
                runs_failed,
                days: daily(changes, from, to, until, zone),
            }
        })
        .collect();

    Ok(Report { from, to, flows })
}

/// the period cut into local days, each summarized on its own
fn daily(
    changes: &[Change],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    until: DateTime<Utc>,
    zone: Tz,
) -> Vec<Option<f64>> {
    let mut days = Vec::new();
    let mut start = from;
    while start < to {
        let end = start
            .with_timezone(&zone)
            .date_naive()
            .checked_add_days(Days::new(1))
            .and_then(|next| next.and_hms_opt(0, 0, 0))
            .and_then(|midnight| midnight.and_local_timezone(zone).earliest())
            .map_or(to, |end| end.with_timezone(&Utc))
            .min(to);
        days.push(
            (start < until)
                .then(|| day(changes, start, end.min(until)))
                .flatten(),
        );
        start = end;
    }
    days
}

/// the state a flow was in when the day began, then its changes during the day
fn day(changes: &[Change], start: DateTime<Utc>, end: DateTime<Utc>) -> Option<f64> {
    let mut clipped: Vec<Change> = changes
        .iter()
        .rev()
        .find(|change| change.at <= start)
        .map(|change| Change {
            at: start,
            ..change.clone()
        })
        .into_iter()
        .collect();
    clipped.extend(
        changes
            .iter()
            .filter(|change| change.at > start && change.at < end)
            .cloned(),
    );
    summarize(&clipped, end).uptime
}

struct Summary {
    uptime: Option<f64>,
    incidents: usize,
    longest_outage: TimeDelta,
    mean_recovery: Option<TimeDelta>,
}

/// walks the changes in order; each state lasts until the next change, the last one until `until`
fn summarize(changes: &[Change], until: DateTime<Utc>) -> Summary {
    let mut measured = TimeDelta::zero();
    let mut up = TimeDelta::zero();
    let mut outages = Vec::new();
    let mut outage_started: Option<DateTime<Utc>> = None;

    for (i, change) in changes.iter().enumerate() {
        let end = changes.get(i + 1).map_or(until, |next| next.at);
        let lasted = (end - change.at).max(TimeDelta::zero());
        if is_measured(&change.state) {
            measured += lasted;
        }
        if is_up(&change.state) {
            up += lasted;
            if let Some(started) = outage_started.take() {
                outages.push(change.at - started);
            }
        } else if is_measured(&change.state) && outage_started.is_none() {
            outage_started = Some(change.at);
        }
    }
    let ongoing = outage_started.map(|started| until - started);

    let longest_outage = outages
        .iter()
        .chain(&ongoing)
        .copied()
        .max()
        .unwrap_or_default();
    let mean_recovery = (!outages.is_empty())
        .then(|| outages.iter().copied().sum::<TimeDelta>() / outages.len() as i32);
    Summary {
        uptime: (measured > TimeDelta::zero())
            .then(|| up.num_seconds() as f64 / measured.num_seconds() as f64),
        incidents: outages.len() + usize::from(ongoing.is_some()),
        longest_outage,
        mean_recovery,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(hour: i64, state: &str) -> Change {
        Change {
            flow: "f".into(),
            at: DateTime::UNIX_EPOCH + TimeDelta::hours(hour),
            state: state.into(),
        }
    }

    #[test]
    fn measures_uptime_outages_and_recovery() {
        // pending 2h (not measured), ok 10h, late 2h, ok 6h, failed 4h still going at the end
        let changes = [
            change(0, "pending"),
            change(2, "ok"),
            change(12, "late"),
            change(14, "ok"),
            change(20, "failed"),
        ];
        let summary = summarize(&changes, DateTime::UNIX_EPOCH + TimeDelta::hours(24));

        assert_eq!(summary.uptime, Some(16.0 / 22.0));
        assert_eq!(summary.incidents, 2);
        assert_eq!(summary.longest_outage, TimeDelta::hours(4));
        assert_eq!(summary.mean_recovery, Some(TimeDelta::hours(2)));
    }

    #[test]
    fn cuts_the_period_into_days() {
        // ok, then late from 06:00 to 18:00 on the second day, ok again; the fourth day is to come
        let changes = [change(0, "ok"), change(30, "late"), change(42, "ok")];
        let from = DateTime::UNIX_EPOCH;
        let days = daily(
            &changes,
            from,
            from + TimeDelta::days(4),
            from + TimeDelta::days(3),
            chrono_tz::UTC,
        );
        assert_eq!(days, vec![Some(1.0), Some(0.5), Some(1.0), None]);
    }

    #[test]
    fn maintenance_is_not_held_against_uptime() {
        // ok 20h, then 4h down inside a maintenance window
        let changes = [change(0, "ok"), change(20, "maintenance")];
        let summary = summarize(&changes, DateTime::UNIX_EPOCH + TimeDelta::hours(24));
        assert_eq!(summary.uptime, Some(1.0));
    }

    #[test]
    fn nothing_measured_before_the_first_report() {
        let summary = summarize(
            &[change(0, "pending")],
            DateTime::UNIX_EPOCH + TimeDelta::hours(5),
        );
        assert_eq!(summary.uptime, None);
        assert_eq!(summary.incidents, 0);
    }
}
