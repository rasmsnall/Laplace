use std::collections::BTreeMap;

use anyhow::Result;
use sqlx::PgPool;

/// numbers a run reported about itself, like rows loaded, bytes received or seconds taken
pub type Metrics = BTreeMap<String, f64>;

/// fewer earlier runs than this say too little about what is normal
const MIN_HISTORY: usize = 5;
const HISTORY: i64 = 10;

/// `tolerance` 0.5 accepts anything from half to double the usual value
pub fn unusual(value: f64, history: &[f64], tolerance: f64) -> Option<f64> {
    if history.len() < MIN_HISTORY {
        return None;
    }
    let mut sorted = history.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    let (low, high) = (median * (1.0 - tolerance), median / (1.0 - tolerance));
    (median > 0.0 && (value < low || value > high)).then_some(median)
}

/// compares the flow's latest run with the runs before it and stores what looked unusual, or clears it
pub async fn evaluate(pool: &PgPool, flow: &str, tolerance: f64) -> Result<()> {
    let latest: Option<sqlx::types::Json<Metrics>> = sqlx::query_scalar(
        "select metrics from events where flow = $1 and outcome = 'ok' and metrics is not null
         order by at desc, id desc limit 1",
    )
    .bind(flow)
    .fetch_optional(pool)
    .await?;
    let Some(sqlx::types::Json(latest)) = latest else {
        return Ok(());
    };

    let mut findings = Vec::new();
    for (name, value) in &latest {
        let history: Vec<f64> = sqlx::query_scalar(
            "select (metrics ->> $2)::float8 from events
             where flow = $1 and outcome = 'ok' and metrics ? $2
             order by at desc, id desc offset 1 limit $3",
        )
        .bind(flow)
        .bind(name)
        .bind(HISTORY)
        .fetch_all(pool)
        .await?;

        if let Some(usual) = unusual(*value, &history, tolerance) {
            findings.push(format!(
                "{name} {} is unusual, usually about {}",
                readable(*value),
                readable(usual)
            ));
        }
    }

    let anomaly = (!findings.is_empty()).then(|| findings.join("; "));
    sqlx::query("update flow_state set anomaly = $2 where flow = $1")
        .bind(flow)
        .bind(anomaly)
        .execute(pool)
        .await?;
    Ok(())
}

fn readable(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        let digits = (value as i64).to_string();
        let mut grouped = String::new();
        for (i, digit) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) && digit.is_ascii_digit() {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        grouped
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USUAL: [f64; 6] = [1000.0, 1100.0, 950.0, 1020.0, 990.0, 1050.0];

    #[test]
    fn flags_drops_and_spikes_only_outside_the_band() {
        assert_eq!(unusual(400.0, &USUAL, 0.5), Some(1020.0));
        assert_eq!(unusual(2500.0, &USUAL, 0.5), Some(1020.0));
        assert_eq!(unusual(700.0, &USUAL, 0.5), None);
        assert_eq!(unusual(1900.0, &USUAL, 0.5), None);
    }

    #[test]
    fn needs_enough_history() {
        assert_eq!(unusual(1.0, &USUAL[..4], 0.5), None);
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(readable(1204331.0), "1,204,331");
        assert_eq!(readable(-4200.0), "-4,200");
        assert_eq!(readable(2.5), "2.50");
    }
}
