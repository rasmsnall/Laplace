//! the rules sftp and storage checks share. a file counts as arrived once it has stopped
//! changing: the same size and modified time on two polls in a row, so an upload still in
//! progress, or cut off part way, is not taken for a finished one. which files were seen is
//! remembered between polls, so a file uploaded with an older, preserved modified time
//! (`put -p`, `scp -p`, rsync) still counts, and ages are measured from when it appeared.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::{Report, Success};
use crate::config::FileRules;
use crate::human::duration;

/// a file on sftp or in object storage
pub struct RemoteFile {
    /// unique within the folder: the name on sftp, the full path in object storage
    pub key: String,
    /// what `pattern` is matched against
    pub name: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
}

/// what an earlier poll saw of one file
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub key: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
    /// when laplace first saw the file, which is when it arrived as far as schedules go
    pub first_seen: DateTime<Utc>,
    /// already reported, so it is not reported again
    pub counted: bool,
}

/// `previous` is the last poll's view of the folder. `baseline` is set on the first poll of a
/// flow that was already running before files were remembered: what is there then is old, and
/// is recorded without counting as new arrivals.
pub fn evaluate(
    rules: &FileRules,
    files: Vec<RemoteFile>,
    previous: &HashMap<String, Observation>,
    baseline: bool,
    now: DateTime<Utc>,
) -> Result<Report> {
    let pattern = glob::Pattern::new(&rules.pattern)?;
    let mut successes = Vec::new();
    let mut empty = None;
    let mut uncollected = None;
    let mut observations = Vec::new();

    for file in files.into_iter().filter(|file| pattern.matches(&file.name)) {
        let before = previous.get(&file.key);
        // at the baseline the files were already there before, so they have settled
        let inherited = baseline && before.is_none();
        let settled = inherited
            || before.is_some_and(|seen| seen.size == file.size && seen.modified == file.modified);
        let first_seen = before.map_or(now, |seen| seen.first_seen);
        // a file rewritten under the same name counts again once it settles
        let mut counted = inherited || (settled && before.is_some_and(|seen| seen.counted));

        if settled && file.size == 0 {
            empty.get_or_insert_with(|| format!("empty file: {}", file.name));
            counted = true;
        } else if settled && !counted {
            successes.push(Success {
                at: first_seen,
                detail: file.name.clone(),
                millis: None,
                metrics: [("bytes".to_owned(), file.size as f64)].into(),
            });
            counted = true;
        }
        if let Some(pickup) = rules.pickup
            && (now - first_seen).to_std().is_ok_and(|age| age > pickup)
        {
            uncollected.get_or_insert_with(|| {
                format!("not collected after {}: {}", duration(pickup), file.name)
            });
        }

        observations.push(Observation {
            key: file.key,
            size: file.size,
            modified: file.modified,
            first_seen,
            counted,
        });
    }

    Ok(Report {
        successes,
        problem: empty.or(uncollected),
        files: Some(observations),
        ..Report::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use std::time::Duration;

    fn rules(pickup: Option<Duration>) -> FileRules {
        FileRules {
            pattern: "*.xml".into(),
            pickup,
            poll: Duration::from_secs(300),
        }
    }

    fn at(minute: i64) -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + TimeDelta::days(20_000) + TimeDelta::minutes(minute)
    }

    fn file(name: &str, size: u64, modified: DateTime<Utc>) -> RemoteFile {
        RemoteFile {
            key: name.into(),
            name: name.into(),
            size,
            modified,
        }
    }

    /// runs polls in order, each seeing the given files, and returns every report
    fn polls(
        rules: &FileRules,
        baseline: bool,
        listings: Vec<(i64, Vec<RemoteFile>)>,
    ) -> Vec<Report> {
        let mut previous: HashMap<String, Observation> = HashMap::new();
        let mut first = baseline;
        listings
            .into_iter()
            .map(|(minute, files)| {
                let report = evaluate(rules, files, &previous, first, at(minute)).unwrap();
                first = false;
                previous = report
                    .files
                    .clone()
                    .unwrap()
                    .into_iter()
                    .map(|seen| (seen.key.clone(), seen))
                    .collect();
                report
            })
            .collect()
    }

    #[test]
    fn a_file_counts_once_it_has_stopped_changing() {
        let reports = polls(
            &rules(None),
            false,
            vec![
                (0, vec![file("a.xml", 100, at(0))]),
                (5, vec![file("a.xml", 100, at(0))]),
                (10, vec![file("a.xml", 100, at(0))]),
            ],
        );
        assert!(reports[0].successes.is_empty(), "first sight is not enough");
        assert_eq!(reports[1].successes.len(), 1);
        assert_eq!(
            reports[1].successes[0].at,
            at(0),
            "it arrived when first seen"
        );
        assert!(reports[2].successes.is_empty(), "counted once");
    }

    #[test]
    fn a_growing_upload_waits_until_it_is_complete() {
        let reports = polls(
            &rules(None),
            false,
            vec![
                (0, vec![file("a.xml", 0, at(0))]),
                (5, vec![file("a.xml", 4_000, at(4))]),
                (10, vec![file("a.xml", 9_000, at(9))]),
                (15, vec![file("a.xml", 9_000, at(9))]),
            ],
        );
        assert!(
            reports.iter().all(|r| r.problem.is_none()),
            "an upload starting empty is not an empty file"
        );
        assert!(reports[..3].iter().all(|r| r.successes.is_empty()));
        assert_eq!(reports[3].successes.len(), 1);
        assert_eq!(reports[3].successes[0].metrics["bytes"], 9_000.0);
    }

    #[test]
    fn a_preserved_old_modified_time_still_counts() {
        let old = at(-60 * 24 * 30);
        let reports = polls(
            &rules(None),
            false,
            vec![
                (0, vec![file("a.xml", 100, old)]),
                (5, vec![file("a.xml", 100, old)]),
            ],
        );
        assert_eq!(reports[1].successes.len(), 1);
        assert_eq!(reports[1].successes[0].at, at(0));
    }

    #[test]
    fn a_rewritten_file_counts_again() {
        let reports = polls(
            &rules(None),
            false,
            vec![
                (0, vec![file("a.xml", 100, at(0))]),
                (5, vec![file("a.xml", 100, at(0))]),
                (10, vec![file("a.xml", 250, at(9))]),
                (15, vec![file("a.xml", 250, at(9))]),
            ],
        );
        assert_eq!(reports[1].successes.len(), 1);
        assert!(reports[2].successes.is_empty());
        assert_eq!(reports[3].successes.len(), 1);
    }

    #[test]
    fn a_file_that_stays_empty_is_a_problem() {
        let reports = polls(
            &rules(None),
            false,
            vec![
                (0, vec![file("a.xml", 0, at(0))]),
                (5, vec![file("a.xml", 0, at(0))]),
            ],
        );
        assert!(reports[0].problem.is_none());
        assert_eq!(reports[1].problem.as_deref(), Some("empty file: a.xml"));
        assert!(reports[1].successes.is_empty());
    }

    #[test]
    fn pickup_is_measured_from_arrival_not_from_the_modified_time() {
        let old = at(-60 * 24);
        let reports = polls(
            &rules(Some(Duration::from_secs(3600))),
            false,
            vec![
                (0, vec![file("a.xml", 100, old)]),
                (30, vec![file("a.xml", 100, old)]),
                (61, vec![file("a.xml", 100, old)]),
            ],
        );
        assert!(reports[1].problem.is_none());
        assert_eq!(
            reports[2].problem.as_deref(),
            Some("not collected after 1h: a.xml")
        );
    }

    #[test]
    fn files_already_there_when_remembering_starts_are_not_new() {
        let reports = polls(
            &rules(None),
            true,
            vec![
                (0, vec![file("old.xml", 100, at(-100))]),
                (
                    5,
                    vec![file("old.xml", 100, at(-100)), file("new.xml", 50, at(4))],
                ),
                (
                    10,
                    vec![file("old.xml", 100, at(-100)), file("new.xml", 50, at(4))],
                ),
            ],
        );
        assert!(
            reports
                .iter()
                .flat_map(|r| &r.successes)
                .all(|s| s.detail == "new.xml")
        );
        assert_eq!(reports[2].successes.len(), 1);
    }

    #[test]
    fn an_empty_file_already_there_stays_a_problem_through_the_upgrade() {
        let reports = polls(
            &rules(None),
            true,
            vec![(0, vec![file("broken.xml", 0, at(-100))])],
        );
        assert_eq!(
            reports[0].problem.as_deref(),
            Some("empty file: broken.xml")
        );
        assert!(reports[0].successes.is_empty());
    }

    #[test]
    fn only_matching_files_count() {
        let reports = polls(
            &rules(None),
            false,
            vec![
                (
                    0,
                    vec![file("a.csv", 100, at(0)), file("b.xml", 100, at(0))],
                ),
                (
                    5,
                    vec![file("a.csv", 100, at(0)), file("b.xml", 100, at(0))],
                ),
            ],
        );
        assert_eq!(reports[1].successes.len(), 1);
        assert_eq!(reports[1].files.as_ref().unwrap().len(), 1);
    }
}
