use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::anomaly::Metrics;
use crate::checks::Report;
use sqlx::types::Json;

#[derive(Clone, Copy)]
pub enum Outcome {
    Start,
    Ok,
    Fail,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Start => "start",
            Outcome::Ok => "ok",
            Outcome::Fail => "fail",
        }
    }
}

#[derive(sqlx::FromRow, Default, Clone)]
pub struct FlowState {
    pub flow: String,
    pub last_ok: Option<DateTime<Utc>>,
    pub running_since: Option<DateTime<Utc>>,
    pub problem: Option<String>,
    pub check_warning: Option<String>,
    pub anomaly: Option<String>,
    pub ack_note: Option<String>,
    pub ack_by: Option<String>,
    pub ack_at: Option<DateTime<Utc>>,
    pub silenced_until: Option<DateTime<Utc>>,
    pub silence_note: Option<String>,
    pub alerted_state: String,
    pub recorded_state: Option<String>,
}

#[derive(sqlx::FromRow, Default, Clone, Copy)]
pub struct CallTotals {
    pub ok: i64,
    pub client_errors: i64,
    pub server_errors: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct Event {
    pub at: DateTime<Utc>,
    pub outcome: String,
    pub detail: String,
    pub millis: Option<i32>,
    pub metrics: Option<Json<Metrics>>,
}

#[derive(Default, Clone, Copy)]
pub struct CallCounts {
    pub ok: i32,
    pub client_errors: i32,
    pub server_errors: i32,
    pub total_millis: i64,
}

pub async fn ensure_flows(pool: &PgPool, flow_ids: &[String]) -> Result<()> {
    sqlx::query("insert into flow_state (flow) select unnest($1::text[]) on conflict do nothing")
        .bind(flow_ids)
        .execute(pool)
        .await?;
    Ok(())
}

/// a finished run also gets `seconds`, measured from its start ping
pub async fn record(
    pool: &PgPool,
    flow: &str,
    outcome: Outcome,
    detail: &str,
    metrics: Option<&Metrics>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "insert into events (flow, at, outcome, detail, metrics)
         select $1, now(), $2, $3,
                case when $2 <> 'start' and running_since is not null
                     then coalesce($4, '{}'::jsonb) || jsonb_build_object('seconds', round(extract(epoch from now() - running_since)))
                     else $4 end
         from flow_state where flow = $1",
    )
    .bind(flow)
    .bind(outcome.as_str())
    .bind(detail)
    .bind(metrics.map(Json))
    .execute(&mut *tx)
    .await?;

    let update = match outcome {
        Outcome::Start => sqlx::query("update flow_state set running_since = now() where flow = $1").bind(flow),
        Outcome::Ok => sqlx::query(
            "update flow_state set last_ok = now(), running_since = null, problem = null where flow = $1",
        )
        .bind(flow),
        Outcome::Fail => sqlx::query("update flow_state set running_since = null, problem = $2 where flow = $1")
            .bind(flow)
            .bind(detail),
    };
    update.execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// a schema change stays a warning for a day, long enough for someone to notice it
const SCHEMA_CHANGE_SHOWN: &str = "24 hours";

async fn remember_schemas(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    flow: &str,
    report: &Report,
) -> Result<Vec<String>> {
    for schema in &report.schemas {
        let stored: Option<Vec<String>> = sqlx::query_scalar(
            "select fields from delta_schemas where flow = $1 and table_name = $2",
        )
        .bind(flow)
        .bind(&schema.table)
        .fetch_optional(&mut **tx)
        .await?;
        match stored {
            None => {
                sqlx::query(
                    "insert into delta_schemas (flow, table_name, fields) values ($1, $2, $3)",
                )
                .bind(flow)
                .bind(&schema.table)
                .bind(&schema.fields)
                .execute(&mut **tx)
                .await?;
            }
            Some(before) if before != schema.fields => {
                let change = crate::checks::delta::schema_change(&before, &schema.fields);
                sqlx::query(
                    "update delta_schemas set fields = $3,
                         change = coalesce($4, change), changed_at = case when $4 is null then changed_at else now() end
                     where flow = $1 and table_name = $2",
                )
                .bind(flow)
                .bind(&schema.table)
                .bind(&schema.fields)
                .bind(&change)
                .execute(&mut **tx)
                .await?;
            }
            Some(_) => {}
        }
    }

    let recent: Vec<(String, String)> = sqlx::query_as(
        "select table_name, change from delta_schemas
         where flow = $1 and change is not null and changed_at > now() - $2::interval
         order by table_name",
    )
    .bind(flow)
    .bind(SCHEMA_CHANGE_SHOWN)
    .fetch_all(&mut **tx)
    .await?;
    Ok(recent
        .into_iter()
        .map(|(table, change)| {
            let table = if table.is_empty() {
                "the table".to_owned()
            } else {
                table
            };
            format!("schema of {table} changed: {change}")
        })
        .collect())
}

pub async fn apply_report(pool: &PgPool, flow: &str, report: &Report) -> Result<()> {
    let mut tx = pool.begin().await?;

    for success in &report.successes {
        let metrics = (!success.metrics.is_empty()).then_some(Json(&success.metrics));
        sqlx::query("insert into events (flow, at, outcome, detail, millis, metrics) values ($1, $2, 'ok', $3, $4, $5)")
            .bind(flow)
            .bind(success.at)
            .bind(&success.detail)
            .bind(success.millis)
            .bind(metrics)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(latest) = report.successes.iter().map(|s| s.at).max() {
        sqlx::query("update flow_state set last_ok = greatest(last_ok, $2) where flow = $1")
            .bind(flow)
            .bind(latest)
            .execute(&mut *tx)
            .await?;
    }

    let schema_changes = remember_schemas(&mut tx, flow, report).await?;
    let warnings: Vec<&str> = report
        .warning
        .iter()
        .map(String::as_str)
        .chain(schema_changes.iter().map(String::as_str))
        .collect();
    let warning = (!warnings.is_empty()).then(|| warnings.join("; "));
    sqlx::query("update flow_state set check_warning = $2 where flow = $1")
        .bind(flow)
        .bind(&warning)
        .execute(&mut *tx)
        .await?;

    if let Some(seen) = &report.host_key {
        sqlx::query(
            "insert into host_keys (flow, fingerprint) values ($1, $2)
             on conflict (flow) do update set
                 pending = case when host_keys.fingerprint = excluded.fingerprint then null else excluded.fingerprint end",
        )
        .bind(flow)
        .bind(seen)
        .execute(&mut *tx)
        .await?;
    }

    let previous: Option<String> =
        sqlx::query_scalar("select problem from flow_state where flow = $1 for update")
            .bind(flow)
            .fetch_one(&mut *tx)
            .await?;
    if previous != report.problem {
        sqlx::query("update flow_state set problem = $2 where flow = $1")
            .bind(flow)
            .bind(&report.problem)
            .execute(&mut *tx)
            .await?;
        if let Some(problem) = &report.problem {
            sqlx::query(
                "insert into events (flow, at, outcome, detail) values ($1, now(), 'fail', $2)",
            )
            .bind(flow)
            .bind(problem)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

pub async fn add_calls(
    pool: &PgPool,
    minutes: &HashMap<(String, DateTime<Utc>), CallCounts>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    for ((flow, minute), counts) in minutes {
        sqlx::query(
            "insert into calls (flow, minute, ok, client_errors, server_errors, total_millis)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (flow, minute) do update set
                 ok = calls.ok + excluded.ok,
                 client_errors = calls.client_errors + excluded.client_errors,
                 server_errors = calls.server_errors + excluded.server_errors,
                 total_millis = calls.total_millis + excluded.total_millis",
        )
        .bind(flow)
        .bind(minute)
        .bind(counts.ok)
        .bind(counts.client_errors)
        .bind(counts.server_errors)
        .bind(counts.total_millis)
        .execute(&mut *tx)
        .await?;

        sqlx::query("update flow_state set last_ok = greatest(last_ok, $2) where flow = $1")
            .bind(flow)
            .bind(minute)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn last_ok(pool: &PgPool, flow: &str) -> Result<Option<DateTime<Utc>>> {
    Ok(
        sqlx::query_scalar("select last_ok from flow_state where flow = $1")
            .bind(flow)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn trusted_host_key(pool: &PgPool, flow: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("select fingerprint from host_keys where flow = $1")
            .bind(flow)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn flows_with_pending_host_key(
    pool: &PgPool,
) -> Result<std::collections::HashSet<String>> {
    let flows: Vec<String> =
        sqlx::query_scalar("select flow from host_keys where pending is not null")
            .fetch_all(pool)
            .await?;
    Ok(flows.into_iter().collect())
}

/// accepts the changed key and checks again right away; returns false when nothing was pending
pub async fn trust_pending_host_key(pool: &PgPool, flow: &str) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let trusted = sqlx::query(
        "update host_keys set fingerprint = pending, pending = null, seen_at = now()
         where flow = $1 and pending is not null",
    )
    .bind(flow)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if trusted {
        sqlx::query("update jobs set next_run_at = now() where name = $1")
            .bind(format!("check:{flow}"))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(trusted)
}

pub async fn flow_states(pool: &PgPool) -> Result<HashMap<String, FlowState>> {
    let rows: Vec<FlowState> = sqlx::query_as("select * from flow_state")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.flow.clone(), row))
        .collect())
}

pub async fn call_totals_since(
    pool: &PgPool,
    since: DateTime<Utc>,
) -> Result<HashMap<String, CallTotals>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "select flow, sum(ok)::bigint, sum(client_errors)::bigint, sum(server_errors)::bigint
         from calls where minute >= $1 group by flow",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(flow, ok, client_errors, server_errors)| {
            (
                flow,
                CallTotals {
                    ok,
                    client_errors,
                    server_errors,
                },
            )
        })
        .collect())
}

pub async fn acknowledge(pool: &PgPool, flow: &str, note: &str, by: &str) -> Result<()> {
    sqlx::query("update flow_state set ack_note = $2, ack_by = $3, ack_at = now() where flow = $1")
        .bind(flow)
        .bind(note)
        .bind(by)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn clear_acknowledgement(pool: &PgPool, flow: &str) -> Result<()> {
    sqlx::query(
        "update flow_state set ack_note = null, ack_by = null, ack_at = null where flow = $1",
    )
    .bind(flow)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn silence(
    pool: &PgPool,
    flow: &str,
    until: DateTime<Utc>,
    note: Option<&str>,
) -> Result<()> {
    sqlx::query("update flow_state set silenced_until = $2, silence_note = $3 where flow = $1")
        .bind(flow)
        .bind(until)
        .bind(note)
        .execute(pool)
        .await?;
    Ok(())
}

/// ending a silence forgets what was alerted, so a flow still broken then alerts right away
pub async fn lift_silence(pool: &PgPool, flow: &str) -> Result<()> {
    sqlx::query(
        "update flow_state set silenced_until = null, silence_note = null, alerted_state = 'pending' where flow = $1",
    )
    .bind(flow)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn end_expired_silences(pool: &PgPool) -> Result<()> {
    sqlx::query(
        "update flow_state set silenced_until = null, silence_note = null, alerted_state = 'pending'
         where silenced_until <= now()",
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_state(pool: &PgPool, flow: &str, state: &str, detail: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("insert into state_changes (flow, state, detail) values ($1, $2, $3)")
        .bind(flow)
        .bind(state)
        .bind(detail)
        .execute(&mut *tx)
        .await?;
    sqlx::query("update flow_state set recorded_state = $2 where flow = $1")
        .bind(flow)
        .bind(state)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// who changed what, for the settings page
pub async fn audit(pool: &PgPool, by: &str, action: &str, detail: &str) -> Result<()> {
    sqlx::query("insert into audit (by, action, detail) values ($1, $2, $3)")
        .bind(by)
        .bind(action)
        .bind(detail)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_alerted(pool: &PgPool, flow: &str, state: &str) -> Result<()> {
    sqlx::query("update flow_state set alerted_state = $2 where flow = $1")
        .bind(flow)
        .bind(state)
        .execute(pool)
        .await?;
    Ok(())
}

/// when the flow last failed, so an alert can point at that run
pub async fn last_failure(pool: &PgPool, flow: &str) -> Result<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar(
        "select at from events where flow = $1 and outcome = 'fail' order by at desc limit 1",
    )
    .bind(flow)
    .fetch_optional(pool)
    .await?)
}

pub async fn recent_events(pool: &PgPool, flow: &str, limit: i64) -> Result<Vec<Event>> {
    Ok(sqlx::query_as(
        "select at, outcome, detail, millis, metrics from events where flow = $1 order by at desc, id desc limit $2",
    )
    .bind(flow)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// inbound traffic shown in the same shape as events, one row per minute
pub async fn recent_calls(pool: &PgPool, flow: &str, limit: i64) -> Result<Vec<Event>> {
    Ok(sqlx::query_as(
        "select minute as at,
                case when server_errors > 0 then 'fail' else 'ok' end as outcome,
                format('%s calls, %s client errors, %s server errors',
                       ok + client_errors + server_errors, client_errors, server_errors) as detail,
                (total_millis / nullif(ok + client_errors + server_errors, 0))::int as millis,
                null::jsonb as metrics
         from calls where flow = $1 order by minute desc limit $2",
    )
    .bind(flow)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn sync_jobs(pool: &PgPool, names: &[String]) -> Result<()> {
    sqlx::query("insert into jobs (name) select unnest($1::text[]) on conflict do nothing")
        .bind(names)
        .execute(pool)
        .await?;
    sqlx::query("delete from jobs where name <> all($1)")
        .bind(names)
        .execute(pool)
        .await?;
    Ok(())
}

/// adds jobs for flows registered since startup; they start due right away
pub async fn add_jobs(pool: &PgPool, names: &[&String]) -> Result<()> {
    sqlx::query("insert into jobs (name) select unnest($1::text[]) on conflict do nothing")
        .bind(names)
        .execute(pool)
        .await?;
    Ok(())
}

/// claims due jobs and pushes them forward in one transaction, so no other replica sees them as due
pub async fn claim_due_jobs(
    pool: &PgPool,
    intervals: &HashMap<String, Duration>,
) -> Result<Vec<String>> {
    let mut tx = pool.begin().await?;
    let known: Vec<&String> = intervals.keys().collect();
    let due: Vec<String> = sqlx::query_scalar(
        "select name from jobs where next_run_at <= now() and name = any($1)
         order by next_run_at limit 50 for update skip locked",
    )
    .bind(&known)
    .fetch_all(&mut *tx)
    .await?;

    let seconds: Vec<f64> = due
        .iter()
        .map(|name| intervals[name].as_secs_f64())
        .collect();
    sqlx::query(
        "update jobs set next_run_at = now() + make_interval(secs => due.seconds)
         from unnest($1::text[], $2::float8[]) as due(name, seconds)
         where jobs.name = due.name",
    )
    .bind(&due)
    .bind(&seconds)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(due)
}

pub async fn prune(pool: &PgPool, before: DateTime<Utc>) -> Result<()> {
    sqlx::query("delete from events where at < $1")
        .bind(before)
        .execute(pool)
        .await?;
    sqlx::query("delete from calls where minute < $1")
        .bind(before)
        .execute(pool)
        .await?;
    // state history stays a year so reports can compare months
    sqlx::query("delete from state_changes where at < now() - interval '400 days'")
        .execute(pool)
        .await?;
    Ok(())
}
