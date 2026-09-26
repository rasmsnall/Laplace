//! delta tables read straight from storage: when they were last committed, what they hold, and
//! whether their schema changed. no databricks cluster involved.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use deltalake::DeltaTableBuilder;
use deltalake::table::builder::ensure_table_uri;
use futures::StreamExt;
use object_store::path::Path;

use super::{Report, Success, TableSchema, storage};
use crate::anomaly::Metrics;
use crate::config::DeltaTables;

/// a folder of this many tables is surely not what was meant
const MAX_TABLES: usize = 2000;
/// tables are looked for this many folders below `url`, e.g. `pg/public/users`
const MAX_DEPTH: usize = 3;
const READ_AT_ONCE: usize = 8;
/// a load writes its tables within this long of each other; a table older than that was left out
const LOAD_SPREAD: TimeDelta = TimeDelta::hours(6);

struct Table {
    name: String,
    version: u64,
    committed_at: DateTime<Utc>,
    rows: Option<u64>,
    files: u64,
    bytes: u64,
    schema: Vec<String>,
}

pub async fn check(config: &DeltaTables, since: Option<DateTime<Utc>>) -> Result<Report> {
    let names = discover(&config.url).await?;
    if names.is_empty() {
        return Ok(Report::problem(format!(
            "no delta tables under {}",
            config.url
        )));
    }

    let base = config.url.trim_end_matches('/');
    let results: Vec<(String, Result<Table>)> = futures::stream::iter(names)
        .map(|name| async move {
            let table = read(base, &name).await;
            (name, table)
        })
        .buffer_unordered(READ_AT_ONCE)
        .collect()
        .await;

    let mut tables = Vec::new();
    for (name, result) in results {
        match result {
            Ok(table) => tables.push(table),
            Err(error) => {
                return Ok(Report::problem(format!(
                    "cannot read table {}: {error:#}",
                    shown(&name)
                )));
            }
        }
    }
    tables.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summarize(&tables, since))
}

fn summarize(tables: &[Table], since: Option<DateTime<Utc>>) -> Report {
    let newest = tables
        .iter()
        .max_by_key(|t| t.committed_at)
        .expect("at least one table");
    let left_out: Vec<&str> = tables
        .iter()
        .filter(|t| newest.committed_at - t.committed_at > LOAD_SPREAD)
        .map(|t| shown(&t.name))
        .collect();

    let mut metrics: Metrics = BTreeMap::new();
    metrics.insert(
        "files".into(),
        tables.iter().map(|t| t.files).sum::<u64>() as f64,
    );
    metrics.insert(
        "bytes".into(),
        tables.iter().map(|t| t.bytes).sum::<u64>() as f64,
    );
    // row counts come from file statistics; a table written without them has no count
    if let Some(rows) = tables.iter().map(|t| t.rows).sum::<Option<u64>>() {
        metrics.insert("rows".into(), rows as f64);
    }
    if tables.len() > 1 {
        metrics.insert("tables".into(), tables.len() as f64);
    }

    let detail = if tables.len() == 1 {
        format!("version {}", newest.version)
    } else {
        format!(
            "{} tables, newest {} at version {}",
            tables.len(),
            shown(&newest.name),
            newest.version
        )
    };
    let successes = since
        .is_none_or(|since| newest.committed_at > since)
        .then_some(Success {
            at: newest.committed_at,
            detail,
            millis: None,
            metrics,
        })
        .into_iter()
        .collect();

    Report {
        successes,
        warning: (!left_out.is_empty())
            .then(|| format!("not in the latest load: {}", list(&left_out))),
        schemas: tables
            .iter()
            .map(|t| TableSchema {
                table: t.name.clone(),
                fields: t.schema.clone(),
            })
            .collect(),
        ..Report::default()
    }
}

/// the folders under `url` that hold a `_delta_log`, relative to it; "" when `url` is itself a table
async fn discover(url: &str) -> Result<Vec<String>> {
    let (store, root) = storage::open(url)?;
    let mut tables = Vec::new();
    let mut level = vec![root.clone()];

    for _ in 0..=MAX_DEPTH {
        let mut next = Vec::new();
        for folder in level {
            let listing = store.list_with_delimiter(Some(&folder)).await?;
            if listing
                .common_prefixes
                .iter()
                .any(|child| child.filename() == Some("_delta_log"))
            {
                tables.push(relative(&root, &folder));
                anyhow::ensure!(
                    tables.len() <= MAX_TABLES,
                    "more than {MAX_TABLES} delta tables under {url}"
                );
            } else {
                next.extend(listing.common_prefixes);
            }
        }
        level = next;
    }
    Ok(tables)
}

fn relative(root: &Path, folder: &Path) -> String {
    folder
        .as_ref()
        .strip_prefix(root.as_ref())
        .unwrap_or(folder.as_ref())
        .trim_matches('/')
        .to_owned()
}

async fn read(base: &str, name: &str) -> Result<Table> {
    let url = if name.is_empty() {
        base.to_owned()
    } else {
        format!("{base}/{name}")
    };
    let mut table = DeltaTableBuilder::from_url(ensure_table_uri(&url)?)?.build()?;
    table.load().await?;
    let version = table.version().context("the table has no commits")?;
    let snapshot = table.snapshot()?;
    let committed_at = snapshot
        .version_timestamp(version)
        .and_then(DateTime::from_timestamp_millis)
        .context("the latest commit has no time")?;

    let files: Vec<_> = snapshot.log_data().iter().collect();
    Ok(Table {
        name: name.to_owned(),
        version,
        committed_at,
        rows: files
            .iter()
            .map(|f| f.num_records().map(|n| n as u64))
            .sum(),
        files: files.len() as u64,
        bytes: files.iter().map(|f| f.size().max(0) as u64).sum(),
        schema: snapshot
            .schema()
            .fields()
            .map(|f| format!("{}:{}", f.name(), f.data_type()))
            .collect(),
    })
}

fn shown(name: &str) -> &str {
    if name.is_empty() { "the table" } else { name }
}

fn list(names: &[&str]) -> String {
    const SHOWN: usize = 5;
    let more = names.len().saturating_sub(SHOWN);
    let head = names
        .iter()
        .take(SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if more > 0 {
        format!("{head} and {more} more")
    } else {
        head
    }
}

/// what changed between two schemas, as `name:type` fields; none when nothing did
pub fn schema_change(before: &[String], after: &[String]) -> Option<String> {
    let split = |fields: &[String]| -> BTreeMap<String, String> {
        fields
            .iter()
            .map(|f| {
                f.split_once(':')
                    .map_or((f.clone(), String::new()), |(n, t)| {
                        (n.to_owned(), t.to_owned())
                    })
            })
            .collect()
    };
    let (old, new) = (split(before), split(after));

    let mut changes = Vec::new();
    changes.extend(
        new.keys()
            .filter(|n| !old.contains_key(*n))
            .map(|n| format!("+{n}")),
    );
    changes.extend(
        old.keys()
            .filter(|n| !new.contains_key(*n))
            .map(|n| format!("-{n}")),
    );
    changes.extend(new.iter().filter_map(|(n, t)| {
        old.get(n)
            .filter(|was| *was != t)
            .map(|was| format!("{n} {was} to {t}"))
    }));
    (!changes.is_empty()).then(|| changes.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(list: &[&str]) -> Vec<String> {
        list.iter().map(|f| f.to_string()).collect()
    }

    #[test]
    fn describes_schema_changes() {
        let before = fields(&["id:long", "amount:decimal(10,2)", "legacy:string"]);
        let after = fields(&["id:long", "amount:decimal(12,2)", "email_verified:boolean"]);
        assert_eq!(
            schema_change(&before, &after).unwrap(),
            "+email_verified, -legacy, amount decimal(10,2) to decimal(12,2)"
        );
        assert_eq!(schema_change(&before, &before), None);
    }

    fn table(name: &str, hours_ago: i64, rows: Option<u64>) -> Table {
        Table {
            name: name.into(),
            version: 3,
            committed_at: Utc::now() - TimeDelta::hours(hours_ago),
            rows,
            files: 2,
            bytes: 100,
            schema: Vec::new(),
        }
    }

    #[test]
    fn flags_tables_left_out_of_the_latest_load() {
        let tables = [
            table("public/orders", 1, Some(10)),
            table("public/users", 1, Some(5)),
            table("public/old", 30, Some(1)),
        ];
        let report = summarize(&tables, None);
        assert_eq!(
            report.warning.as_deref(),
            Some("not in the latest load: public/old")
        );
        let metrics = &report.successes[0].metrics;
        assert_eq!(
            (metrics["rows"], metrics["tables"], metrics["files"]),
            (16.0, 3.0, 6.0)
        );
    }

    #[test]
    fn a_commit_already_seen_is_not_a_new_success() {
        let tables = [table("t", 2, None)];
        assert!(
            summarize(&tables, Some(Utc::now() - TimeDelta::hours(1)))
                .successes
                .is_empty()
        );
        assert!(
            !summarize(&tables, None).successes[0]
                .metrics
                .contains_key("rows"),
            "unknown rows are left out"
        );
    }
}

#[cfg(test)]
mod real_tables {
    use std::sync::Arc;

    use deltalake::arrow::array::{Int64Array, RecordBatch, StringArray};
    use deltalake::arrow::datatypes::{DataType, Field, Schema};
    use deltalake::kernel::StructType;
    use deltalake::kernel::engine::arrow_conversion::TryIntoKernel;
    use deltalake::writer::{DeltaWriter, RecordBatchWriter};

    use super::*;

    async fn write_table(path: &std::path::Path, rows: i64) {
        std::fs::create_dir_all(path).unwrap();
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let columns: StructType = schema.as_ref().try_into_kernel().unwrap();
        let mut table =
            DeltaTableBuilder::from_url(ensure_table_uri(path.to_str().unwrap()).unwrap())
                .unwrap()
                .build()
                .unwrap()
                .create()
                .with_columns(columns.fields().cloned())
                .await
                .unwrap();
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from_iter_values(0..rows)),
                Arc::new(StringArray::from_iter_values(
                    (0..rows).map(|i| format!("row {i}")),
                )),
            ],
        )
        .unwrap();
        let mut writer = RecordBatchWriter::for_table(&table).unwrap();
        writer.write(batch).await.unwrap();
        writer.flush_and_commit(&mut table).await.unwrap();
    }

    #[tokio::test]
    async fn finds_and_reads_every_table_in_a_folder() {
        let root = std::env::temp_dir().join(format!("laplace-delta-{}", std::process::id()));
        write_table(&root.join("pg/public/users"), 3).await;
        write_table(&root.join("pg/public/orders"), 2).await;

        let config = DeltaTables {
            url: root.join("pg").to_str().unwrap().to_owned(),
            poll: std::time::Duration::from_secs(60),
        };
        let report = check(&config, None).await.unwrap();
        std::fs::remove_dir_all(&root).ok();

        assert!(report.problem.is_none(), "{:?}", report.problem);
        let metrics = &report.successes[0].metrics;
        assert_eq!((metrics["rows"], metrics["tables"]), (5.0, 2.0));
        let tables: Vec<&str> = report.schemas.iter().map(|s| s.table.as_str()).collect();
        assert_eq!(tables, ["public/orders", "public/users"]);
        assert_eq!(report.schemas[0].fields, ["id:long", "name:string"]);
    }

    #[tokio::test]
    async fn a_folder_without_tables_is_a_problem() {
        let root = std::env::temp_dir().join(format!("laplace-empty-{}", std::process::id()));
        std::fs::create_dir_all(root.join("nothing/here")).unwrap();
        let config = DeltaTables {
            url: root.to_str().unwrap().to_owned(),
            poll: std::time::Duration::from_secs(60),
        };
        let report = check(&config, None).await.unwrap();
        std::fs::remove_dir_all(&root).ok();
        assert!(report.problem.unwrap().starts_with("no delta tables under"));
    }
}
