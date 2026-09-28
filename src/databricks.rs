//! the databricks jobs api: listing a workspace's jobs so they can be monitored with one click

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const MAX_JOBS: usize = 2000;

pub struct Workspace {
    host: String,
    token: String,
}

impl Workspace {
    pub fn from_env() -> Result<Self> {
        let host = crate::optional_env("DATABRICKS_HOST").context("DATABRICKS_HOST is not set")?;
        let token =
            crate::optional_env("DATABRICKS_TOKEN").context("DATABRICKS_TOKEN is not set")?;
        Ok(Self {
            host: host.trim_end_matches('/').to_owned(),
            token,
        })
    }

    pub async fn get<T: serde::de::DeserializeOwned>(
        &self,
        client: &reqwest::Client,
        path: &str,
    ) -> Result<T> {
        let url = format!("{}{path}", self.host);
        Ok(client
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

impl Workspace {
    pub async fn post<T: serde::de::DeserializeOwned>(
        &self,
        client: &reqwest::Client,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let url = format!("{}{path}", self.host);
        Ok(client
            .post(url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

#[derive(Serialize)]
pub struct Job {
    pub job_id: u64,
    pub name: String,
    /// quartz cron as databricks stores it, e.g. `0 0 2 * * ?`
    pub cron: Option<String>,
    pub timezone: Option<String>,
    pub paused: bool,
}

#[derive(Deserialize)]
struct JobList {
    #[serde(default)]
    jobs: Vec<RawJob>,
    #[serde(default)]
    has_more: bool,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct RawJob {
    job_id: u64,
    settings: Settings,
}

#[derive(Deserialize)]
struct Settings {
    #[serde(default)]
    name: String,
    schedule: Option<Schedule>,
}

#[derive(Deserialize)]
struct Schedule {
    quartz_cron_expression: String,
    timezone_id: Option<String>,
    pause_status: Option<String>,
}

impl From<RawJob> for Job {
    fn from(raw: RawJob) -> Self {
        let schedule = raw.settings.schedule;
        Job {
            job_id: raw.job_id,
            name: if raw.settings.name.is_empty() {
                format!("job {}", raw.job_id)
            } else {
                raw.settings.name
            },
            paused: schedule
                .as_ref()
                .is_some_and(|s| s.pause_status.as_deref() == Some("PAUSED")),
            timezone: schedule.as_ref().and_then(|s| s.timezone_id.clone()),
            cron: schedule.map(|s| s.quartz_cron_expression),
        }
    }
}

pub async fn list_jobs(client: &reqwest::Client, workspace: &Workspace) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let path = match &page {
            Some(token) => format!("/api/2.1/jobs/list?limit=100&page_token={token}"),
            None => "/api/2.1/jobs/list?limit=100".to_owned(),
        };
        let list: JobList = workspace.get(client, &path).await?;
        jobs.extend(list.jobs.into_iter().map(Job::from));
        match list.next_page_token {
            Some(token) if list.has_more && jobs.len() < MAX_JOBS => page = Some(token),
            _ => break,
        }
    }
    jobs.sort_by_key(|job| job.name.to_lowercase());
    Ok(jobs)
}

pub async fn get_job(client: &reqwest::Client, workspace: &Workspace, job_id: u64) -> Result<Job> {
    let raw: RawJob = workspace
        .get(client, &format!("/api/2.1/jobs/get?job_id={job_id}"))
        .await?;
    Ok(raw.into())
}

/// a flow id from a job name: "Nightly ERP Load (v2)" becomes "nightly-erp-load-v2"
pub fn flow_id(name: &str) -> String {
    let mut id = String::new();
    for c in name.to_lowercase().chars().map(plain_letter) {
        if c.is_ascii_alphanumeric() {
            id.push(c);
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_matches('-').chars().take(64).collect()
}

/// swedish and finnish job names keep their meaning instead of losing letters
fn plain_letter(c: char) -> char {
    match c {
        'å' | 'ä' | 'æ' | 'á' | 'à' => 'a',
        'ö' | 'ø' | 'ó' | 'ò' => 'o',
        'é' | 'è' => 'e',
        'ü' | 'ú' => 'u',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::flow_id;

    #[test]
    fn makes_readable_ids() {
        assert_eq!(flow_id("Nightly ERP Load (v2)"), "nightly-erp-load-v2");
        assert_eq!(flow_id("  pgdelta // day  "), "pgdelta-day");
        assert_eq!(flow_id("Ölspår Päivittäinen"), "olspar-paivittainen");
    }
}
