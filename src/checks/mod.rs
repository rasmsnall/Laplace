mod databricks;
pub mod delta;
pub mod files;
mod http;
mod sftp;
mod storage;

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::anomaly::Metrics;
use crate::config::{Flow, Kind};

/// what one poll of an external system found
#[derive(Default)]
pub struct Report {
    pub successes: Vec<Success>,
    pub problem: Option<String>,
    /// works now but needs attention soon; replaced on every poll
    pub warning: Option<String>,
    /// the sftp server's key as seen on this poll
    pub host_key: Option<String>,
    /// the schema of each delta table seen on this poll
    pub schemas: Vec<TableSchema>,
    /// every matching file an sftp or storage poll saw, replacing what the last poll saw
    pub files: Option<Vec<files::Observation>>,
}

pub struct TableSchema {
    pub table: String,
    /// `name:type`, in table order
    pub fields: Vec<String>,
}

pub struct Success {
    pub at: DateTime<Utc>,
    pub detail: String,
    pub millis: Option<i32>,
    pub metrics: Metrics,
}

impl Report {
    fn problem(problem: String) -> Self {
        Self {
            problem: Some(problem),
            ..Self::default()
        }
    }
}

/// what earlier polls already established
pub struct Known {
    /// the last success already recorded, so each job run or commit is reported once
    pub last_ok: Option<DateTime<Utc>>,
    pub host_key: Option<String>,
    /// the files the last sftp or storage poll saw
    pub files: HashMap<String, files::Observation>,
    /// whether files have been remembered for this flow before
    pub files_observed: bool,
}

impl Known {
    /// a flow that already ran before files were remembered starts from what is there now
    fn baseline(&self) -> bool {
        !self.files_observed && self.last_ok.is_some()
    }
}

pub async fn run(flow: &Flow, known: Known, client: &reqwest::Client) -> Report {
    let result = match &flow.kind {
        Kind::Http {
            url,
            timeout,
            max_latency,
        } => http::probe(client, url, *timeout, *max_latency).await,
        Kind::Sftp(sftp) => {
            let sftp = sftp.clone();
            let baseline = known.baseline();
            tokio::task::spawn_blocking(move || {
                sftp::check(&sftp, known.host_key.as_deref(), &known.files, baseline)
            })
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
        }
        Kind::Storage(storage) => storage::check(storage, &known.files, known.baseline()).await,
        Kind::Delta(delta) => delta::check(delta, known.last_ok).await,
        Kind::Databricks { job_id, .. } => databricks::check(client, *job_id, known.last_ok).await,
        Kind::Heartbeat { .. } | Kind::Inbound { .. } => {
            unreachable!("{} flows are pushed, not polled", flow.kind.name())
        }
    };
    result.unwrap_or_else(|error| Report::problem(format!("{error:#}")))
}
