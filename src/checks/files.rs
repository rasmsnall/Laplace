use anyhow::Result;
use chrono::{DateTime, Utc};

use super::{Report, Success};
use crate::config::FileRules;
use crate::human::duration;

/// a file on sftp or in object storage; sftp and storage checks share the same rules
pub struct RemoteFile {
    pub name: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
}

pub fn evaluate(
    rules: &FileRules,
    files: Vec<RemoteFile>,
    since: Option<DateTime<Utc>>,
) -> Result<Report> {
    let pattern = glob::Pattern::new(&rules.pattern)?;
    let matching: Vec<RemoteFile> = files
        .into_iter()
        .filter(|file| pattern.matches(&file.name))
        .collect();
    let now = Utc::now();

    let problem = if let Some(empty) = matching.iter().find(|file| file.size == 0) {
        Some(format!("empty file: {}", empty.name))
    } else {
        rules.pickup.and_then(|pickup| {
            matching
                .iter()
                .find(|file| (now - file.modified).to_std().is_ok_and(|age| age > pickup))
                .map(|file| format!("not collected after {}: {}", duration(pickup), file.name))
        })
    };

    let successes = matching
        .into_iter()
        .filter(|file| file.size > 0 && since.is_none_or(|since| file.modified > since))
        .map(|file| Success {
            at: file.modified,
            detail: file.name,
            millis: None,
            metrics: [("bytes".to_owned(), file.size as f64)].into(),
        })
        .collect();

    Ok(Report {
        successes,
        problem,
        ..Report::default()
    })
}
