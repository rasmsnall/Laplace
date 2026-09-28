use std::{collections::HashMap, collections::HashSet, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;

use crate::schedule::{CronStyle, Schedule};

#[derive(Deserialize)]
pub struct Config {
    #[serde(rename = "flow")]
    pub flows: Vec<Flow>,
    /// who looks after flows, and where their alerts go
    #[serde(default)]
    pub owners: HashMap<String, Owner>,
    /// recurring windows when flows are expected to be down, such as a partner's weekly patching
    #[serde(default)]
    pub maintenance: HashMap<String, Maintenance>,
}

#[derive(Deserialize)]
pub struct Owner {
    /// environment variable holding this owner's slack or teams webhook
    pub webhook_env: Option<String>,
    /// addresses that get this owner's alerts by email
    #[serde(default)]
    pub email: Vec<String>,
}

#[derive(Deserialize)]
pub struct Maintenance {
    /// when the window opens, e.g. "0 2 * * 0" for sundays at 02:00
    pub cron: String,
    #[serde(default)]
    pub cron_style: CronStyle,
    /// the zone `cron` is read in; LAPLACE_TIMEZONE when left out
    pub timezone: Option<String>,
    #[serde(with = "humantime_serde")]
    pub lasts: Duration,
    /// flow ids or patterns such as "bank-*", which also match flows jobs registered
    pub flows: Vec<String>,
}

impl Maintenance {
    pub fn covers(&self, flow: &str) -> bool {
        self.flows
            .iter()
            .any(|pattern| glob::Pattern::new(pattern).is_ok_and(|pattern| pattern.matches(flow)))
    }

    /// when the window next opens after `now`
    pub fn next_opening(
        &self,
        default_zone: chrono_tz::Tz,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>> {
        let zone = Schedule::zone(self.timezone.as_deref(), default_zone)?;
        Schedule::parse(&self.cron, self.cron_style, zone, &[])?.next(now)
    }

    /// when the window that is open at `now` closes, if one is
    pub fn open_until(
        &self,
        default_zone: chrono_tz::Tz,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>> {
        let zone = Schedule::zone(self.timezone.as_deref(), default_zone)?;
        let schedule = Schedule::parse(&self.cron, self.cron_style, zone, &[])?;
        let Some(opened) = schedule.latest(now)? else {
            return Ok(None);
        };
        let closes = opened + TimeDelta::from_std(self.lasts)?;
        Ok((now < closes).then_some(closes))
    }
}

#[derive(Deserialize, Clone)]
pub struct Flow {
    pub id: String,
    /// how often the flow should succeed; ignored for lateness when `cron` is set
    #[serde(with = "humantime_serde", default = "default_every")]
    pub every: Duration,
    /// when the flow is due, e.g. "0 7 * * 1-5"
    pub cron: Option<String>,
    #[serde(default)]
    pub cron_style: CronStyle,
    /// only count business days in these countries: "se", "fi"
    #[serde(default)]
    pub calendar: Vec<String>,
    /// the zone `cron` is read in; LAPLACE_TIMEZONE when left out
    pub timezone: Option<String>,
    #[serde(with = "humantime_serde", default)]
    pub grace: Duration,
    /// shown as the source on the dashboard; derived from the kind when not set
    pub source: Option<String>,
    /// flows that must succeed before this one can, making them a pipeline
    #[serde(default)]
    pub after: Vec<String>,
    pub owner: Option<String>,
    /// uptime the flow is held to in reports, e.g. 0.995
    pub sla: Option<f64>,
    /// how far a run's numbers may stray from the usual: 0.5 accepts half to double
    #[serde(default = "default_anomaly_tolerance")]
    pub anomaly_tolerance: f64,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Kind {
    Heartbeat {
        #[serde(with = "humantime_serde", default)]
        max_runtime: Option<Duration>,
    },
    Inbound {
        #[serde(default = "default_max_error_rate")]
        max_error_rate: f64,
    },
    Http {
        url: String,
        #[serde(with = "humantime_serde", default = "default_timeout")]
        timeout: Duration,
        #[serde(with = "humantime_serde", default)]
        max_latency: Option<Duration>,
    },
    Sftp(Sftp),
    Storage(Storage),
    Delta(DeltaTables),
    Databricks {
        job_id: u64,
        #[serde(with = "humantime_serde", default = "default_poll")]
        poll: Duration,
    },
}

#[derive(Deserialize, Clone)]
pub struct Sftp {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    pub password_env: Option<String>,
    pub key_file: Option<PathBuf>,
    /// "SHA256:..." as printed by `ssh-keygen -lf`
    pub host_key: Option<String>,
    pub dir: String,
    #[serde(flatten)]
    pub files: FileRules,
}

#[derive(Deserialize, Clone)]
pub struct Storage {
    /// gs://bucket/prefix, abfss://container@account.dfs.core.windows.net/prefix or a local path
    pub url: String,
    #[serde(flatten)]
    pub files: FileRules,
}

#[derive(Deserialize, Clone)]
pub struct DeltaTables {
    /// one delta table, or a folder of them such as pgdelta's output
    pub url: String,
    #[serde(with = "humantime_serde", default = "default_delta_poll")]
    pub poll: Duration,
}

#[derive(Deserialize, Clone)]
pub struct FileRules {
    #[serde(default = "default_pattern")]
    pub pattern: String,
    /// a matching file older than this has not been collected
    #[serde(with = "humantime_serde", default)]
    pub pickup: Option<Duration>,
    #[serde(with = "humantime_serde", default = "default_poll")]
    pub poll: Duration,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Heartbeat { .. } => "heartbeat",
            Kind::Inbound { .. } => "inbound",
            Kind::Http { .. } => "http",
            Kind::Sftp(_) => "sftp",
            Kind::Storage(_) => "storage",
            Kind::Delta(_) => "delta",
            Kind::Databricks { .. } => "databricks",
        }
    }
}

impl Flow {
    pub fn schedule(&self, default_zone: chrono_tz::Tz) -> Result<Option<Schedule>> {
        let Some(cron) = &self.cron else {
            return Ok(None);
        };
        let zone = Schedule::zone(self.timezone.as_deref(), default_zone)?;
        Ok(Some(Schedule::parse(
            cron,
            self.cron_style,
            zone,
            &self.calendar,
        )?))
    }

    pub fn source_label(&self) -> String {
        if let Some(source) = &self.source {
            return source.clone();
        }
        match &self.kind {
            Kind::Heartbeat { .. } => format!("/ping/{}", self.id),
            Kind::Inbound { .. } => format!("/calls/{}", self.id),
            Kind::Http { url, .. } => url.clone(),
            Kind::Sftp(sftp) => format!("sftp://{}@{}{}", sftp.user, sftp.host, sftp.dir),
            Kind::Storage(storage) => storage.url.clone(),
            Kind::Delta(delta) => delta.url.clone(),
            Kind::Databricks { job_id, .. } => format!("databricks job {job_id}"),
        }
    }

    /// a page a browser can open for what the flow watches: the api, the databricks job, or the
    /// bucket in the google cloud console. sftp servers, azure storage and pushed jobs have none
    pub fn source_link(&self) -> Option<String> {
        match &self.kind {
            Kind::Http { url, .. } => web_link(url),
            Kind::Databricks { job_id, .. } => {
                databricks_job_link(crate::optional_env("DATABRICKS_HOST").as_deref(), *job_id)
            }
            Kind::Storage(Storage { url, .. }) | Kind::Delta(DeltaTables { url, .. }) => {
                let bucket_and_prefix = url.strip_prefix("gs://")?;
                web_link(&format!(
                    "https://console.cloud.google.com/storage/browser/{bucket_and_prefix}"
                ))
            }
            Kind::Heartbeat { .. } | Kind::Inbound { .. } | Kind::Sftp(_) => None,
        }
    }

    /// flows that watch polls itself; heartbeat and inbound flows are pushed to us
    pub fn poll_interval(&self) -> Option<Duration> {
        match &self.kind {
            Kind::Http { .. } => Some(self.every),
            Kind::Sftp(sftp) => Some(sftp.files.poll),
            Kind::Storage(storage) => Some(storage.files.poll),
            Kind::Delta(delta) => Some(delta.poll),
            Kind::Databricks { poll, .. } => Some(*poll),
            Kind::Heartbeat { .. } | Kind::Inbound { .. } => None,
        }
    }
}

fn databricks_job_link(host: Option<&str>, job_id: u64) -> Option<String> {
    web_link(&format!("{}/jobs/{job_id}", host?.trim_end_matches('/')))
}

/// only http and https become links, so nothing else can reach the dashboard's href
fn web_link(candidate: &str) -> Option<String> {
    let parsed = url::Url::parse(candidate).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.to_string())
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        Self::parse(&text).with_context(|| format!("in {path}"))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;

        let mut ids = HashSet::new();
        for flow in &config.flows {
            if !ids.insert(&flow.id) {
                bail!("duplicate flow id: {}", flow.id);
            }
            if let Kind::Sftp(sftp) = &flow.kind
                && sftp.key_file.is_none()
                && sftp.password_env.is_none()
            {
                bail!("flow {}: set key_file or password_env", flow.id);
            }
        }
        for flow in &config.flows {
            flow.schedule(chrono_tz::UTC)
                .with_context(|| format!("flow {}", flow.id))?;
            if !(flow.anomaly_tolerance > 0.0 && flow.anomaly_tolerance < 1.0) {
                bail!(
                    "flow {}: anomaly_tolerance must be above 0 and below 1",
                    flow.id
                );
            }
            if let Some(owner) = &flow.owner
                && !config.owners.contains_key(owner)
            {
                bail!("flow {}: owner {owner} is not in [owners]", flow.id);
            }
            if let Some(unknown) = flow
                .after
                .iter()
                .find(|upstream| config.flow(upstream).is_none())
            {
                bail!("flow {}: after refers to unknown flow {unknown}", flow.id);
            }
        }
        for (name, window) in &config.maintenance {
            window
                .open_until(chrono_tz::UTC, Utc::now())
                .with_context(|| format!("maintenance {name}"))?;
            if window.lasts.is_zero() {
                bail!("maintenance {name}: lasts must be longer than zero");
            }
            if let Some(bad) = window.flows.iter().find(|p| glob::Pattern::new(p).is_err()) {
                bail!("maintenance {name}: {bad:?} is not a valid pattern");
            }
        }
        for (name, owner) in &config.owners {
            if let Some(bad) = owner
                .email
                .iter()
                .find(|address| address.parse::<lettre::Address>().is_err())
            {
                bail!("owner {name}: {bad:?} is not an email address");
            }
        }
        if let Some(flow) = crate::pipeline::find_cycle(&config.flows) {
            bail!("flow {flow}: `after` goes round in a circle");
        }
        Ok(config)
    }

    pub fn flow(&self, id: &str) -> Option<&Flow> {
        self.flows.iter().find(|flow| flow.id == id)
    }
}

fn default_every() -> Duration {
    Duration::from_secs(86_400)
}

fn default_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_poll() -> Duration {
    Duration::from_secs(300)
}

/// reading hundreds of transaction logs is heavier than listing a folder
fn default_delta_poll() -> Duration {
    Duration::from_secs(900)
}

fn default_ssh_port() -> u16 {
    22
}

fn default_pattern() -> String {
    "*".into()
}

pub fn default_anomaly_tolerance() -> f64 {
    0.5
}

fn default_max_error_rate() -> f64 {
    0.05
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const FILE: &str = r#"
        [maintenance.bank-sunday]
        cron = "0 2 * * 0"
        lasts = "2h"
        flows = ["bank-*"]

        [owners.finance]
        email = ["ops@example.com"]

        [[flow]]
        id = "bank-statements"
        kind = "heartbeat"
        owner = "finance"
    "#;

    fn stockholm(d: u32, h: u32, min: u32) -> DateTime<Utc> {
        chrono_tz::Europe::Stockholm
            .with_ymd_and_hms(2026, 9, d, h, min, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_window_is_open_from_its_cron_for_as_long_as_it_lasts() {
        let config = Config::parse(FILE).unwrap();
        let window = &config.maintenance["bank-sunday"];
        let zone = chrono_tz::Europe::Stockholm;
        // sunday 27 september 2026
        assert_eq!(window.open_until(zone, stockholm(27, 1, 59)).unwrap(), None);
        assert_eq!(
            window.open_until(zone, stockholm(27, 3, 30)).unwrap(),
            Some(stockholm(27, 4, 0))
        );
        assert_eq!(window.open_until(zone, stockholm(27, 4, 0)).unwrap(), None);
        assert_eq!(
            window.next_opening(zone, stockholm(27, 3, 0)).unwrap(),
            Some(
                chrono_tz::Europe::Stockholm
                    .with_ymd_and_hms(2026, 10, 4, 2, 0, 0)
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        assert!(window.covers("bank-statements"));
        assert!(!window.covers("erp-api"));
    }

    #[test]
    fn links_only_to_pages_a_browser_can_open() {
        let flow = |kind: &str| {
            Config::parse(&FILE.replace(r#"kind = "heartbeat""#, kind))
                .unwrap()
                .flows[0]
                .source_link()
        };
        assert_eq!(
            flow(
                r#"kind = "http"
        url = "https://api.partner.example/health""#
            )
            .as_deref(),
            Some("https://api.partner.example/health")
        );
        assert_eq!(
            flow(
                r#"kind = "storage"
        url = "gs://landing/erp/""#
            )
            .as_deref(),
            Some("https://console.cloud.google.com/storage/browser/landing/erp/")
        );
        assert_eq!(
            flow(
                r#"kind = "storage"
        url = "abfss://landing@account.dfs.core.windows.net/erp/""#
            ),
            None
        );
        assert_eq!(flow(r#"kind = "heartbeat""#), None);
        assert_eq!(
            flow(
                r#"kind = "http"
        url = "javascript:alert(1)""#
            ),
            None
        );
        assert_eq!(
            databricks_job_link(Some("https://adb-1.azuredatabricks.net/"), 42).as_deref(),
            Some("https://adb-1.azuredatabricks.net/jobs/42")
        );
        assert_eq!(databricks_job_link(None, 42), None);
    }

    #[test]
    fn rejects_a_bad_window_or_address() {
        let bad_cron = FILE.replace("0 2 * * 0", "whenever");
        assert!(Config::parse(&bad_cron).is_err());
        let bad_email = FILE.replace("ops@example.com", "ops at example");
        assert!(Config::parse(&bad_email).is_err());
    }
}
