//! what databricks jobs cost, from the billing system tables through a sql warehouse.
//!
//! the numbers are databricks' own charge at list price: before any discount the account has,
//! and without the cloud's charge for the machines of classic compute. usage reaches the
//! tables within about 12 hours, and late corrections arrive as extra records, so the last
//! 40 days are fetched again every hour and replace what was stored.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::config::Kind;
use crate::databricks::Workspace;
use crate::{App, optional_env};

const WINDOW_DAYS: i64 = 40;
/// days before yesterday that say what a job usually costs
const USUAL_DAYS: i64 = 14;
/// fewer days than this with a cost say too little about what is usual
const MIN_USUAL_DAYS: usize = 5;
/// yesterday is flagged above this many times the usual day, and at least this much more
const SPIKE_FACTOR: f64 = 2.0;
const SPIKE_MIN_EXTRA: f64 = 1.0;
const POLL: Duration = Duration::from_secs(5);
const MAX_POLLS: usize = 60;

/// usage records carry the job as a string; corrections are extra records with negative
/// quantities, so summing every record gives the corrected total
const QUERY: &str = "select cast(u.usage_metadata.job_id as bigint) as job_id,
       u.usage_date as day,
       sum(u.usage_quantity) as dbus,
       sum(u.usage_quantity * p.pricing.effective_list.default) as cost,
       max(p.currency_code) as currency
from system.billing.usage u
join system.billing.list_prices p
  on u.sku_name = p.sku_name and u.cloud = p.cloud and u.usage_unit = p.usage_unit
 and u.usage_end_time >= p.price_start_time
 and (p.price_end_time is null or u.usage_end_time < p.price_end_time)
where u.usage_metadata.job_id is not null
  and u.usage_date >= date_sub(current_date(), :days)
  and (:workspace is null or u.workspace_id = :workspace)
group by 1, 2";

/// DATABRICKS_WAREHOUSE_ID turns cost tracking on; DATABRICKS_WORKSPACE_ID keeps it to this
/// workspace, since the billing tables cover the whole account and job ids are per workspace
pub struct Settings {
    warehouse: String,
    workspace: Option<String>,
}

pub fn settings() -> Option<Settings> {
    Some(Settings {
        warehouse: optional_env("DATABRICKS_WAREHOUSE_ID")?,
        workspace: optional_env("DATABRICKS_WORKSPACE_ID"),
    })
}

pub struct DayCost {
    pub job_id: i64,
    pub day: NaiveDate,
    pub dbus: f64,
    pub cost: f64,
    pub currency: String,
}

pub async fn refresh(app: &App) -> Result<()> {
    let Some(settings) = settings() else {
        return Ok(());
    };
    let workspace = Workspace::from_env()?;
    let days = fetch(&app.client, &workspace, &settings).await?;
    store(&app.pool, &days).await?;
    flag_spikes(app).await
}

#[derive(Deserialize)]
struct Statement {
    statement_id: String,
    status: StatementStatus,
    result: Option<Chunk>,
}

#[derive(Deserialize)]
struct StatementStatus {
    state: String,
    error: Option<StatementError>,
}

#[derive(Deserialize)]
struct StatementError {
    message: Option<String>,
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    data_array: Vec<Vec<Option<String>>>,
    next_chunk_internal_link: Option<String>,
}

async fn fetch(
    client: &reqwest::Client,
    workspace: &Workspace,
    settings: &Settings,
) -> Result<Vec<DayCost>> {
    let body = json!({
        "warehouse_id": settings.warehouse,
        "statement": QUERY,
        "wait_timeout": "30s",
        "format": "JSON_ARRAY",
        "disposition": "INLINE",
        "parameters": [
            { "name": "days", "value": WINDOW_DAYS.to_string(), "type": "INT" },
            { "name": "workspace", "value": settings.workspace, "type": "STRING" },
        ],
    });
    let mut statement: Statement = workspace
        .post(client, "/api/2.0/sql/statements", &body)
        .await?;

    // a warehouse that was asleep takes a while to start
    for _ in 0..MAX_POLLS {
        if !matches!(statement.status.state.as_str(), "PENDING" | "RUNNING") {
            break;
        }
        tokio::time::sleep(POLL).await;
        statement = workspace
            .get(
                client,
                &format!("/api/2.0/sql/statements/{}", statement.statement_id),
            )
            .await?;
    }
    if statement.status.state != "SUCCEEDED" {
        let reason = statement
            .status
            .error
            .and_then(|error| error.message)
            .unwrap_or_default();
        bail!("the cost query ended {}: {reason}", statement.status.state);
    }

    let mut rows = Vec::new();
    let mut chunk = statement.result;
    while let Some(current) = chunk {
        rows.extend(current.data_array);
        chunk = match current.next_chunk_internal_link {
            Some(link) => Some(workspace.get(client, &link).await?),
            None => None,
        };
    }
    rows.into_iter().map(|row| parse(&row)).collect()
}

fn parse(row: &[Option<String>]) -> Result<DayCost> {
    let field = |i: usize, name: &str| {
        row.get(i)
            .cloned()
            .flatten()
            .with_context(|| format!("cost row without {name}"))
    };
    Ok(DayCost {
        job_id: field(0, "job_id")?.parse()?,
        day: field(1, "day")?.parse()?,
        dbus: field(2, "dbus")?.parse()?,
        cost: field(3, "cost")?.parse()?,
        currency: field(4, "currency")?,
    })
}

/// the window is replaced as a whole, so corrections and jobs that stopped costing are right
async fn store(pool: &PgPool, days: &[DayCost]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("delete from job_costs where day >= current_date - $1::int")
        .bind(WINDOW_DAYS as i32)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "insert into job_costs (job_id, day, dbus, cost, currency)
         select * from unnest($1::bigint[], $2::date[], $3::float8[], $4::float8[], $5::text[])
         on conflict (job_id, day) do update set dbus = excluded.dbus, cost = excluded.cost,
             currency = excluded.currency",
    )
    .bind(days.iter().map(|d| d.job_id).collect::<Vec<_>>())
    .bind(days.iter().map(|d| d.day).collect::<Vec<_>>())
    .bind(days.iter().map(|d| d.dbus).collect::<Vec<_>>())
    .bind(days.iter().map(|d| d.cost).collect::<Vec<_>>())
    .bind(days.iter().map(|d| d.currency.clone()).collect::<Vec<_>>())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// how much yesterday cost, when that was far above the usual day; costs below usual are
/// left alone, since yesterday may still be arriving
pub fn spike(yesterday: f64, usual_days: &[f64]) -> Option<f64> {
    if usual_days.len() < MIN_USUAL_DAYS {
        return None;
    }
    let mut sorted = usual_days.to_vec();
    sorted.sort_by(f64::total_cmp);
    let usual = sorted[sorted.len() / 2];
    (usual > 0.0 && yesterday > usual * SPIKE_FACTOR && yesterday - usual >= SPIKE_MIN_EXTRA)
        .then_some(usual)
}

async fn flag_spikes(app: &App) -> Result<()> {
    let recent: Vec<(i64, NaiveDate, f64, String)> = sqlx::query_as(
        "select job_id, day, cost, currency from job_costs
         where day >= current_date - $1::int and day < current_date",
    )
    .bind((USUAL_DAYS + 1) as i32)
    .fetch_all(&app.pool)
    .await?;
    let yesterday = chrono::Utc::now().date_naive() - chrono::Days::new(1);
    let mut by_job: HashMap<i64, Vec<(NaiveDate, f64, String)>> = HashMap::new();
    for (job_id, day, cost, currency) in recent {
        by_job
            .entry(job_id)
            .or_default()
            .push((day, cost, currency));
    }

    for flow in app.flows().await? {
        let Kind::Databricks { job_id, .. } = flow.kind else {
            continue;
        };
        let days = by_job.get(&(job_id as i64)).cloned().unwrap_or_default();
        let last = days.iter().find(|(day, ..)| *day == yesterday);
        let usual: Vec<f64> = days
            .iter()
            .filter(|(day, ..)| *day != yesterday)
            .map(|(_, cost, _)| *cost)
            .collect();
        let warning = last.and_then(|(_, cost, currency)| {
            spike(*cost, &usual).map(|usual| {
                format!("cost yesterday {cost:.2} {currency}, usually about {usual:.2}")
            })
        });
        sqlx::query("update flow_state set cost_warning = $2 where flow = $1")
            .bind(&flow.id)
            .bind(warning)
            .execute(&app.pool)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_a_day_far_above_the_usual_one() {
        let usual = [10.0, 12.0, 11.0, 9.0, 10.5, 11.5];
        assert_eq!(spike(40.0, &usual), Some(11.0));
        assert_eq!(
            spike(20.0, &usual),
            None,
            "under double is normal variation"
        );
        assert_eq!(spike(2.0, &usual), None, "a low day may still be arriving");
    }

    #[test]
    fn needs_enough_history_and_a_real_amount() {
        assert_eq!(spike(100.0, &[10.0, 10.0]), None);
        assert_eq!(
            spike(0.9, &[0.2, 0.2, 0.2, 0.2, 0.2]),
            None,
            "cents do not page anyone"
        );
    }

    #[test]
    fn reads_a_row_as_the_api_sends_it() {
        let row = ["42", "2026-09-27", "18.5", "12.95", "USD"].map(|v| Some(v.to_owned()));
        let day = parse(&row).unwrap();
        assert_eq!(
            (day.job_id, day.cost, day.currency.as_str()),
            (42, 12.95, "USD")
        );
        assert_eq!(day.day, NaiveDate::from_ymd_opt(2026, 9, 27).unwrap());
        assert!(parse(&[Some("42".into()), None]).is_err());
    }
}
