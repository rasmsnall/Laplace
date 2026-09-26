use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::anomaly::Metrics;
use crate::config::{Flow, Kind};
use sqlx::types::Json;

/// more marks than this per flow cannot be told apart on screen
const MAX_MARKS: i64 = 600;

#[derive(Serialize)]
pub struct Timeline {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub flows: HashMap<String, Vec<Mark>>,
}

/// a run with a start and an end, or a single point in time when `end` equals `start`
#[derive(Serialize)]
pub struct Mark {
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub outcome: String,
    pub detail: String,
    pub millis: Option<i32>,
    pub metrics: Option<Json<Metrics>>,
}

#[derive(sqlx::FromRow)]
struct EventRow {
    flow: String,
    at: DateTime<Utc>,
    outcome: String,
    detail: String,
    millis: Option<i32>,
    metrics: Option<Json<Metrics>>,
}

pub async fn build(pool: &PgPool, flows: &[Flow], hours: i64) -> Result<Timeline> {
    let to = Utc::now();
    let from = to - TimeDelta::hours(hours);
    let bucket_seconds = (hours * 3600 / MAX_MARKS).max(1);

    let events: Vec<EventRow> = sqlx::query_as(
        "select flow, at, outcome, detail, millis, metrics from events where at >= $1 order by flow, at, id",
    )
    .bind(from)
    .fetch_all(pool)
    .await?;

    let mut by_flow: HashMap<String, Vec<EventRow>> = HashMap::new();
    for event in events {
        by_flow.entry(event.flow.clone()).or_default().push(event);
    }

    let mut marks_by_flow = HashMap::new();
    for flow in flows {
        let events = by_flow.remove(&flow.id).unwrap_or_default();
        let marks = match flow.kind {
            Kind::Heartbeat { .. } => runs(events),
            Kind::Inbound { max_error_rate } => {
                call_buckets(pool, &flow.id, from, bucket_seconds, max_error_rate).await?
            }
            _ => thin(points(events), bucket_seconds),
        };
        marks_by_flow.insert(flow.id.clone(), marks);
    }

    Ok(Timeline {
        from,
        to,
        flows: marks_by_flow,
    })
}

/// pairs each start with the ok or fail that follows it; a start followed by another start never reported back
fn runs(events: Vec<EventRow>) -> Vec<Mark> {
    let mut marks = Vec::new();
    let mut started: Option<EventRow> = None;

    for event in events {
        match event.outcome.as_str() {
            "start" => {
                if let Some(abandoned) = started.replace(event) {
                    marks.push(Mark {
                        start: abandoned.at,
                        end: None,
                        outcome: "unfinished".into(),
                        detail: "never reported an exit code".into(),
                        millis: None,
                        metrics: None,
                    });
                }
            }
            _ => {
                let start = started.take().map_or(event.at, |run| run.at);
                let millis = (event.at - start).num_milliseconds().try_into().ok();
                marks.push(Mark {
                    start,
                    end: Some(event.at),
                    outcome: event.outcome,
                    detail: event.detail,
                    millis,
                    metrics: event.metrics,
                });
            }
        }
    }

    if let Some(running) = started {
        marks.push(Mark {
            start: running.at,
            end: None,
            outcome: "running".into(),
            detail: running.detail,
            millis: None,
            metrics: None,
        });
    }
    marks
}

fn points(events: Vec<EventRow>) -> Vec<Mark> {
    events
        .into_iter()
        .map(|event| Mark {
            start: event.at,
            end: Some(event.at),
            outcome: event.outcome,
            detail: event.detail,
            millis: event.millis,
            metrics: event.metrics,
        })
        .collect()
}

/// keeps every failure and at most one success per bucket
fn thin(marks: Vec<Mark>, bucket_seconds: i64) -> Vec<Mark> {
    let mut last_bucket = None;
    marks
        .into_iter()
        .filter(|mark| {
            if mark.outcome != "ok" {
                return true;
            }
            let bucket = mark.start.timestamp() / bucket_seconds;
            last_bucket.replace(bucket) != Some(bucket)
        })
        .collect()
}

/// a bucket is a failure by the same rule as the flow status: server errors above the allowed rate
async fn call_buckets(
    pool: &PgPool,
    flow: &str,
    from: DateTime<Utc>,
    bucket_seconds: i64,
    max_error_rate: f64,
) -> Result<Vec<Mark>> {
    let rows: Vec<(DateTime<Utc>, i64, i64, i64, i64)> = sqlx::query_as(
        "select to_timestamp(floor(extract(epoch from minute) / $3) * $3) as bucket,
                sum(ok)::bigint, sum(client_errors)::bigint, sum(server_errors)::bigint, sum(total_millis)::bigint
         from calls where flow = $1 and minute >= $2
         group by bucket order by bucket",
    )
    .bind(flow)
    .bind(from)
    .bind(bucket_seconds.max(60) as f64)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(bucket, ok, client_errors, server_errors, total_millis)| {
            let calls = ok + client_errors + server_errors;
            let error_rate = server_errors as f64 / calls.max(1) as f64;
            Mark {
                start: bucket,
                end: Some(bucket),
                outcome: if error_rate > max_error_rate {
                    "fail"
                } else {
                    "ok"
                }
                .into(),
                detail: format!(
                    "{calls} calls, {client_errors} client errors, {server_errors} server errors"
                ),
                millis: (total_millis / calls.max(1)).try_into().ok(),
                metrics: None,
            }
        })
        .collect())
}
