use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use serde::Deserialize;

use crate::db::{self, CallCounts};

#[derive(Deserialize)]
pub struct Call {
    pub status: u16,
    #[serde(default)]
    pub millis: i64,
}

/// a call longer than a day is a gateway reporting nonsense, not a slow call
const MAX_MILLIS: i64 = 86_400_000;

/// counts calls in memory and writes them per minute, so a busy gateway costs one upsert
/// per flow per flush instead of one write per request
#[derive(Default)]
pub struct CallBuffer(Mutex<HashMap<(String, DateTime<Utc>), CallCounts>>);

impl CallBuffer {
    pub fn add(&self, flow: &str, calls: &[Call]) {
        let minute = Utc::now().duration_trunc(TimeDelta::minutes(1)).unwrap();
        let mut buffer = self.0.lock().unwrap();
        let counts = buffer.entry((flow.to_owned(), minute)).or_default();
        for call in calls {
            match call.status {
                500.. => counts.server_errors += 1,
                400..500 => counts.client_errors += 1,
                _ => counts.ok += 1,
            }
            counts.total_millis = counts
                .total_millis
                .saturating_add(call.millis.clamp(0, MAX_MILLIS));
        }
    }

    pub async fn flush(&self, pool: &sqlx::PgPool) -> anyhow::Result<()> {
        let pending = std::mem::take(&mut *self.0.lock().unwrap());
        if pending.is_empty() {
            return Ok(());
        }
        if let Err(error) = db::add_calls(pool, &pending).await {
            self.restore(pending);
            return Err(error);
        }
        Ok(())
    }

    fn restore(&self, pending: HashMap<(String, DateTime<Utc>), CallCounts>) {
        let mut buffer = self.0.lock().unwrap();
        for (key, counts) in pending {
            let entry = buffer.entry(key).or_default();
            entry.ok += counts.ok;
            entry.client_errors += counts.client_errors;
            entry.server_errors += counts.server_errors;
            entry.total_millis = entry.total_millis.saturating_add(counts.total_millis);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonsense_durations_cannot_overflow_the_minute() {
        let buffer = CallBuffer::default();
        let calls = [
            Call {
                status: 200,
                millis: i64::MAX,
            },
            Call {
                status: 200,
                millis: i64::MAX,
            },
            Call {
                status: 503,
                millis: -5,
            },
        ];
        buffer.add("gateway", &calls);
        let counts = buffer.0.lock().unwrap().values().next().copied().unwrap();
        assert_eq!(counts.total_millis, 2 * MAX_MILLIS);
        assert_eq!((counts.ok, counts.server_errors), (2, 1));
    }
}
