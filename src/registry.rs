use std::time::Duration;

use anyhow::Result;
use sqlx::PgPool;

use crate::config::{Flow, Kind};
use crate::schedule::CronStyle;

pub const DEFAULT_EVERY: Duration = Duration::from_secs(86_400);
const MIN_GRACE: Duration = Duration::from_secs(300);
/// a scheduled job gets this long past its start time before it counts as late
const SCHEDULED_GRACE: Duration = Duration::from_secs(3600);
const DATABRICKS_POLL: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PushKind {
    Heartbeat,
    Inbound,
}

impl PushKind {
    fn as_str(self) -> &'static str {
        match self {
            PushKind::Heartbeat => "heartbeat",
            PushKind::Inbound => "inbound",
        }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    kind: String,
    every_seconds: i64,
    after: Vec<String>,
    owner: Option<String>,
    job_id: Option<i64>,
    cron: Option<String>,
    timezone: Option<String>,
    calendar: Vec<String>,
    grace_seconds: Option<i64>,
}

impl Row {
    fn into_flow(self) -> Flow {
        let every = Duration::from_secs(self.every_seconds.max(60) as u64);
        let kind = match (self.kind.as_str(), self.job_id) {
            ("inbound", _) => Kind::Inbound {
                max_error_rate: 0.05,
            },
            ("databricks", Some(job_id)) => Kind::Databricks {
                job_id: job_id as u64,
                poll: DATABRICKS_POLL,
            },
            _ => Kind::Heartbeat { max_runtime: None },
        };
        // databricks stores its schedules in quartz syntax
        let cron_style = if self.kind == "databricks" {
            CronStyle::Quartz
        } else {
            CronStyle::Unix
        };
        let grace = match (self.grace_seconds, &self.cron) {
            (Some(seconds), _) => Duration::from_secs(seconds.max(0) as u64),
            (None, Some(_)) => SCHEDULED_GRACE,
            (None, None) => (every / 10).max(MIN_GRACE),
        };
        Flow {
            id: self.id,
            every,
            cron: self.cron,
            cron_style,
            calendar: self.calendar,
            timezone: self.timezone,
            grace,
            source: None,
            after: self.after,
            owner: self.owner,
            sla: None,
            anomaly_tolerance: crate::config::default_anomaly_tolerance(),
            kind,
        }
    }
}

pub async fn all(pool: &PgPool) -> Result<Vec<Flow>> {
    let rows: Vec<Row> = sqlx::query_as(
        "select id, kind, every_seconds, after, owner, job_id, cron, timezone, calendar, grace_seconds from registered_flows order by id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Row::into_flow).collect())
}

pub async fn find(pool: &PgPool, id: &str) -> Result<Option<Flow>> {
    let row: Option<Row> = sqlx::query_as(
        "select id, kind, every_seconds, after, owner, job_id, cron, timezone, calendar, grace_seconds from registered_flows where id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Row::into_flow))
}

/// what a job may say about itself when it reports; anything left out keeps its current value
#[derive(Default, Clone)]
pub struct Settings {
    pub every: Option<Duration>,
    pub after: Option<Vec<String>>,
    pub owner: Option<String>,
    pub cron: Option<String>,
    pub calendar: Option<Vec<String>>,
    pub timezone: Option<String>,
    pub grace: Option<Duration>,
}

/// creates the flow, or updates the settings the job sent
pub async fn register(
    pool: &PgPool,
    id: &str,
    kind: PushKind,
    settings: &Settings,
) -> Result<Flow> {
    let mut tx = pool.begin().await?;
    let row: Row = sqlx::query_as(
        "insert into registered_flows (id, kind, every_seconds, after, owner, cron, calendar, timezone, grace_seconds)
         values ($1, $2, coalesce($3, $4), coalesce($5, '{}'), $6, $7, coalesce($8, '{}'), $9, $10)
         on conflict (id) do update set
             every_seconds = coalesce($3, registered_flows.every_seconds),
             after = coalesce($5, registered_flows.after),
             owner = coalesce($6, registered_flows.owner),
             cron = coalesce($7, registered_flows.cron),
             calendar = coalesce($8, registered_flows.calendar),
             timezone = coalesce($9, registered_flows.timezone),
             grace_seconds = coalesce($10, registered_flows.grace_seconds)
         returning id, kind, every_seconds, after, owner, job_id, cron, timezone, calendar, grace_seconds",
    )
    .bind(id)
    .bind(kind.as_str())
    .bind(settings.every.map(|every| every.as_secs() as i64))
    .bind(DEFAULT_EVERY.as_secs() as i64)
    .bind(&settings.after)
    .bind(&settings.owner)
    .bind(&settings.cron)
    .bind(&settings.calendar)
    .bind(&settings.timezone)
    .bind(settings.grace.map(|grace| grace.as_secs() as i64))
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("insert into flow_state (flow) values ($1) on conflict do nothing")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(row.into_flow())
}

/// a workspace job picked in the dashboard; polled like a configured databricks flow
pub async fn register_databricks(
    pool: &PgPool,
    id: &str,
    job: &crate::databricks::Job,
    owner: Option<&str>,
) -> Result<Flow> {
    let mut tx = pool.begin().await?;
    let row: Row = sqlx::query_as(
        "insert into registered_flows (id, kind, every_seconds, owner, job_id, cron, timezone)
         values ($1, 'databricks', $2, $3, $4, $5, $6)
         returning id, kind, every_seconds, after, owner, job_id, cron, timezone, calendar, grace_seconds",
    )
    .bind(id)
    .bind(DEFAULT_EVERY.as_secs() as i64)
    .bind(owner)
    .bind(job.job_id as i64)
    .bind(&job.cron)
    .bind(&job.timezone)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("insert into flow_state (flow) values ($1) on conflict do nothing")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(row.into_flow())
}

/// a flow of the given kind with default settings, for checks before it exists
pub fn placeholder(kind: PushKind) -> Flow {
    Row {
        id: String::new(),
        kind: kind.as_str().to_owned(),
        every_seconds: DEFAULT_EVERY.as_secs() as i64,
        after: Vec::new(),
        owner: None,
        job_id: None,
        cron: None,
        timezone: None,
        calendar: Vec::new(),
        grace_seconds: None,
    }
    .into_flow()
}

/// ids end up in urls, file names and alert text, so keep them boring
pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_' || b == b'.'
        })
}
