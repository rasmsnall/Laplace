//! appends history to delta so databricks can report on it without touching postgres.
//! a crash between the delta commit and the watermark update appends a batch twice;
//! events carry their id and calls their (flow, minute) key, so readers can deduplicate.

use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use deltalake::arrow::array::{
    ArrayRef, Int32Array, Int64Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use deltalake::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use deltalake::kernel::StructType;
use deltalake::kernel::engine::arrow_conversion::TryIntoKernel;
use deltalake::table::builder::ensure_table_uri;
use deltalake::writer::{DeltaWriter, RecordBatchWriter};
use deltalake::{DeltaTable, DeltaTableBuilder};
use sqlx::PgPool;

const BATCH_ROWS: i64 = 100_000;

#[derive(sqlx::FromRow)]
struct EventRow {
    id: i64,
    flow: String,
    at: DateTime<Utc>,
    outcome: String,
    detail: String,
    millis: Option<i32>,
}

#[derive(sqlx::FromRow)]
struct CallRow {
    flow: String,
    minute: DateTime<Utc>,
    ok: i32,
    client_errors: i32,
    server_errors: i32,
    total_millis: i64,
}

pub async fn run(pool: &PgPool, root: &str) -> Result<()> {
    export_events(pool, &format!("{root}/events")).await?;
    export_calls(pool, &format!("{root}/calls")).await?;
    export_state_changes(pool, &format!("{root}/state_changes")).await
}

#[derive(sqlx::FromRow)]
struct StateChangeRow {
    id: i64,
    flow: String,
    at: DateTime<Utc>,
    state: String,
    detail: String,
}

async fn export_state_changes(pool: &PgPool, uri: &str) -> Result<()> {
    let after = watermark(pool, "state_changes").await?;
    let rows: Vec<StateChangeRow> = sqlx::query_as(
        "select id, flow, at, state, detail from state_changes where id > $1 order by id limit $2",
    )
    .bind(after)
    .bind(BATCH_ROWS)
    .fetch_all(pool)
    .await?;
    let Some(last) = rows.last().map(|row| row.id) else {
        return Ok(());
    };

    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("flow", DataType::Utf8, false),
            Field::new("at", utc_timestamp(), false),
            Field::new("state", DataType::Utf8, false),
            Field::new("detail", DataType::Utf8, false),
        ])),
        vec![
            column(Int64Array::from_iter_values(rows.iter().map(|r| r.id))),
            column(StringArray::from_iter_values(rows.iter().map(|r| &r.flow))),
            column(timestamps(rows.iter().map(|r| r.at))),
            column(StringArray::from_iter_values(rows.iter().map(|r| &r.state))),
            column(StringArray::from_iter_values(
                rows.iter().map(|r| &r.detail),
            )),
        ],
    )?;
    append(uri, batch).await?;
    set_watermark(pool, "state_changes", last).await
}

async fn export_events(pool: &PgPool, uri: &str) -> Result<()> {
    loop {
        let after = watermark(pool, "events").await?;
        let rows: Vec<EventRow> = sqlx::query_as(
            "select id, flow, at, outcome, detail, millis from events where id > $1 order by id limit $2",
        )
        .bind(after)
        .bind(BATCH_ROWS)
        .fetch_all(pool)
        .await?;
        let Some(last) = rows.last().map(|row| row.id) else {
            return Ok(());
        };

        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("flow", DataType::Utf8, false),
                Field::new("at", utc_timestamp(), false),
                Field::new("outcome", DataType::Utf8, false),
                Field::new("detail", DataType::Utf8, false),
                Field::new("millis", DataType::Int32, true),
            ])),
            vec![
                column(Int64Array::from_iter_values(rows.iter().map(|r| r.id))),
                column(StringArray::from_iter_values(rows.iter().map(|r| &r.flow))),
                column(timestamps(rows.iter().map(|r| r.at))),
                column(StringArray::from_iter_values(
                    rows.iter().map(|r| &r.outcome),
                )),
                column(StringArray::from_iter_values(
                    rows.iter().map(|r| &r.detail),
                )),
                column(Int32Array::from_iter(rows.iter().map(|r| r.millis))),
            ],
        )?;
        append(uri, batch).await?;
        set_watermark(pool, "events", last).await?;
    }
}

/// only closed minutes are exported; the current one is still being counted
async fn export_calls(pool: &PgPool, uri: &str) -> Result<()> {
    let after = DateTime::from_timestamp(watermark(pool, "calls").await?, 0).unwrap_or_default();
    let rows: Vec<CallRow> = sqlx::query_as(
        "select flow, minute, ok, client_errors, server_errors, total_millis from calls
         where minute > $1 and minute < date_trunc('minute', now()) - interval '1 minute'
         order by minute",
    )
    .bind(after)
    .fetch_all(pool)
    .await?;
    let Some(last) = rows.last().map(|row| row.minute) else {
        return Ok(());
    };

    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("flow", DataType::Utf8, false),
            Field::new("minute", utc_timestamp(), false),
            Field::new("ok", DataType::Int32, false),
            Field::new("client_errors", DataType::Int32, false),
            Field::new("server_errors", DataType::Int32, false),
            Field::new("total_millis", DataType::Int64, false),
        ])),
        vec![
            column(StringArray::from_iter_values(rows.iter().map(|r| &r.flow))),
            column(timestamps(rows.iter().map(|r| r.minute))),
            column(Int32Array::from_iter_values(rows.iter().map(|r| r.ok))),
            column(Int32Array::from_iter_values(
                rows.iter().map(|r| r.client_errors),
            )),
            column(Int32Array::from_iter_values(
                rows.iter().map(|r| r.server_errors),
            )),
            column(Int64Array::from_iter_values(
                rows.iter().map(|r| r.total_millis),
            )),
        ],
    )?;
    append(uri, batch).await?;
    set_watermark(pool, "calls", last.timestamp()).await
}

async fn append(uri: &str, batch: RecordBatch) -> Result<()> {
    let mut table = open_or_create(uri, &batch.schema()).await?;
    let mut writer = RecordBatchWriter::for_table(&table)?;
    writer.write(batch).await?;
    writer.flush_and_commit(&mut table).await?;
    Ok(())
}

async fn open_or_create(uri: &str, schema: &Schema) -> Result<DeltaTable> {
    let mut table = DeltaTableBuilder::from_url(ensure_table_uri(uri)?)?.build()?;
    if table.load().await.is_ok() {
        return Ok(table);
    }
    let columns: StructType = schema.try_into_kernel()?;
    Ok(table
        .create()
        .with_columns(columns.fields().cloned())
        .await?)
}

async fn watermark(pool: &PgPool, name: &str) -> Result<i64> {
    let value: Option<i64> =
        sqlx::query_scalar("select watermark from export_watermarks where name = $1")
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(value.unwrap_or(0))
}

async fn set_watermark(pool: &PgPool, name: &str, value: i64) -> Result<()> {
    sqlx::query(
        "insert into export_watermarks (name, watermark) values ($1, $2)
         on conflict (name) do update set watermark = excluded.watermark",
    )
    .bind(name)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

fn utc_timestamp() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

fn timestamps(values: impl Iterator<Item = DateTime<Utc>>) -> TimestampMicrosecondArray {
    TimestampMicrosecondArray::from_iter_values(values.map(|at| at.timestamp_micros()))
        .with_timezone("UTC")
}

fn column(array: impl deltalake::arrow::array::Array + 'static) -> ArrayRef {
    Arc::new(array)
}
