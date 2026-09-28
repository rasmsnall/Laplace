use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{TimeDelta, Utc};

use crate::{App, alerts, anomaly, checks, costs, db, export, ops};

const ALERTS_EVERY: Duration = Duration::from_secs(30);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);
const EXPORT_EVERY: Duration = Duration::from_secs(300);
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// billing usage arrives within about 12 hours, so hourly is plenty
const COSTS_EVERY: Duration = Duration::from_secs(3600);
const TICK: Duration = Duration::from_secs(2);

enum Job {
    Check(String),
    Alerts,
    Heartbeat,
    Export,
    Prune,
    Costs,
}

impl Job {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "alerts" => Some(Job::Alerts),
            "heartbeat" => Some(Job::Heartbeat),
            "export" => Some(Job::Export),
            "prune" => Some(Job::Prune),
            "costs" => Some(Job::Costs),
            _ => name
                .strip_prefix("check:")
                .map(|flow| Job::Check(flow.to_owned())),
        }
    }
}

/// read on every tick, so flows registered while running are polled without a restart
pub async fn intervals(app: &App) -> Result<HashMap<String, Duration>> {
    let mut intervals: HashMap<String, Duration> = app
        .flows()
        .await?
        .iter()
        .filter_map(|flow| Some((format!("check:{}", flow.id), flow.poll_interval()?)))
        .collect();
    intervals.insert("alerts".into(), ALERTS_EVERY);
    intervals.insert("prune".into(), PRUNE_EVERY);
    if app.delta_uri.is_some() {
        intervals.insert("export".into(), EXPORT_EVERY);
    }
    if costs::settings().is_some() {
        intervals.insert("costs".into(), COSTS_EVERY);
    }
    if app.heartbeat_url.is_some() {
        intervals.insert("heartbeat".into(), HEARTBEAT_EVERY);
    }
    Ok(intervals)
}

/// every replica runs this loop; the database decides which replica gets which job
pub async fn run_forever(app: Arc<App>) {
    let mut ticker = tokio::time::interval(TICK);
    loop {
        ticker.tick().await;
        app.last_tick
            .store(Utc::now().timestamp(), std::sync::atomic::Ordering::Relaxed);

        if let Err(error) = app.calls.flush(&app.pool).await {
            eprintln!("flushing inbound calls failed: {error:#}");
        }
        if let Err(error) = app.reload_settings().await {
            eprintln!("reading settings failed: {error:#}");
        }
        if let Err(error) = app.reload_config().await {
            eprintln!("reloading flows failed: {error:#}");
        }

        let due = match claim(&app).await {
            Ok(due) => due,
            Err(error) => {
                eprintln!("claiming jobs failed: {error:#}");
                continue;
            }
        };
        for name in due {
            let app = app.clone();
            tokio::spawn(async move {
                if let Err(error) = run_job(&app, &name).await {
                    eprintln!("job {name} failed: {error:#}");
                }
            });
        }
    }
}

async fn claim(app: &App) -> Result<Vec<String>> {
    let intervals = intervals(app).await?;
    db::add_jobs(&app.pool, &intervals.keys().collect::<Vec<_>>()).await?;
    db::claim_due_jobs(&app.pool, &intervals).await
}

async fn run_job(app: &App, name: &str) -> Result<()> {
    match Job::parse(name) {
        Some(Job::Check(flow_id)) => {
            let Some(flow) = app.flow(&flow_id).await? else {
                return Ok(());
            };
            let (files, files_observed) = db::file_observations(&app.pool, &flow.id).await?;
            let known = checks::Known {
                last_ok: db::last_ok(&app.pool, &flow.id).await?,
                host_key: db::trusted_host_key(&app.pool, &flow.id).await?,
                files,
                files_observed,
            };
            let report = checks::run(&flow, known, &app.client).await;
            db::apply_report(&app.pool, &flow.id, &report).await?;
            if report
                .successes
                .iter()
                .any(|success| !success.metrics.is_empty())
            {
                anomaly::evaluate(&app.pool, &flow.id, flow.anomaly_tolerance).await?;
            }
            Ok(())
        }
        Some(Job::Alerts) => alerts::run(app).await,
        Some(Job::Heartbeat) => ops::heartbeat(app).await,
        Some(Job::Export) => match &app.delta_uri {
            Some(uri) => export::run(&app.pool, uri).await,
            None => Ok(()),
        },
        Some(Job::Prune) => {
            db::prune(
                &app.pool,
                Utc::now() - TimeDelta::days(app.settings().retention_days),
            )
            .await
        }
        Some(Job::Costs) => costs::refresh(app).await,
        None => Ok(()),
    }
}
