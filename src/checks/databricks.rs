use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::{Report, Success};
use crate::databricks::Workspace;

#[derive(Deserialize)]
struct RunList {
    #[serde(default)]
    runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Run {
    run_id: u64,
    start_time: i64,
    end_time: i64,
    state: RunState,
}

#[derive(Deserialize)]
struct RunState {
    result_state: Option<String>,
    #[serde(default)]
    state_message: String,
}

impl Run {
    fn succeeded(&self) -> bool {
        self.state.result_state.as_deref() == Some("SUCCESS")
    }

    fn ended(&self) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp_millis(self.end_time)
    }
}

pub async fn check(
    client: &reqwest::Client,
    job_id: u64,
    since: Option<DateTime<Utc>>,
) -> Result<Report> {
    let workspace = Workspace::from_env()?;
    let list: RunList = workspace
        .get(
            client,
            &format!("/api/2.1/jobs/runs/list?job_id={job_id}&completed_only=true&limit=25"),
        )
        .await?;

    let problem = list.runs.first().filter(|run| !run.succeeded()).map(|run| {
        let result = run.state.result_state.as_deref().unwrap_or("unknown");
        format!(
            "run {} {}: {}",
            run.run_id,
            result.to_lowercase(),
            run.state.state_message
        )
    });

    let successes = list
        .runs
        .iter()
        .filter(|run| run.succeeded())
        .filter_map(|run| Some((run, run.ended()?)))
        .filter(|(_, ended)| since.is_none_or(|since| *ended > since))
        .map(|(run, ended)| Success {
            at: ended,
            detail: format!("run {}", run.run_id),
            millis: None,
            metrics: [(
                "seconds".to_owned(),
                ((run.end_time - run.start_time) / 1000) as f64,
            )]
            .into(),
        })
        .collect();

    Ok(Report {
        successes,
        problem,
        ..Report::default()
    })
}
