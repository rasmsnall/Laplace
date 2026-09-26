//! the internal port for kubernetes probes and prometheus. it is never exposed through the
//! ingress, so flow names and states stay inside the cluster.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use chrono::Utc;

use crate::App;
use crate::status::State as FlowState;

/// the scheduler ticks every 2 seconds; this long without one means it is stuck
const SCHEDULER_STUCK_AFTER: i64 = 60;

pub fn routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/healthz", get(live))
        .route("/readyz", get(ready))
        .route("/metrics", get(metrics))
        .with_state(app)
}

fn scheduler_lag(app: &App) -> i64 {
    Utc::now().timestamp() - app.last_tick.load(Ordering::Relaxed)
}

/// liveness: kubernetes restarts the pod when the scheduler has stopped ticking
async fn live(State(app): State<Arc<App>>) -> impl IntoResponse {
    let lag = scheduler_lag(&app);
    if lag > SCHEDULER_STUCK_AFTER {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("scheduler has not ticked for {lag}s"),
        )
    } else {
        (StatusCode::OK, "ok".to_owned())
    }
}

/// readiness: traffic only goes to a pod that can reach postgres
async fn ready(State(app): State<Arc<App>>) -> impl IntoResponse {
    let ping = sqlx::query("select 1").execute(&app.pool);
    match tokio::time::timeout(Duration::from_secs(2), ping).await {
        Ok(Ok(_)) => (StatusCode::OK, "ready"),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "postgres unreachable"),
    }
}

async fn metrics(State(app): State<Arc<App>>) -> impl IntoResponse {
    let statuses = match app.statuses().await {
        Ok(statuses) => statuses,
        Err(error) => {
            return (StatusCode::SERVICE_UNAVAILABLE, format!("# {error:#}\n")).into_response();
        }
    };

    let mut out = String::new();
    out.push_str("# HELP laplace_flow_state 1 for the state each flow is in\n# TYPE laplace_flow_state gauge\n");
    for flow in &statuses {
        let owner = flow.owner.as_deref().unwrap_or("");
        let _ = writeln!(
            out,
            "laplace_flow_state{{flow=\"{}\",kind=\"{}\",owner=\"{}\",state=\"{}\"}} 1",
            label(&flow.id),
            flow.kind,
            label(owner),
            flow.state.as_str()
        );
    }

    out.push_str("# HELP laplace_flow_last_ok_age_seconds seconds since each flow last succeeded\n# TYPE laplace_flow_last_ok_age_seconds gauge\n");
    for flow in &statuses {
        if let Some(last_ok) = flow.last_ok {
            let _ = writeln!(
                out,
                "laplace_flow_last_ok_age_seconds{{flow=\"{}\"}} {}",
                label(&flow.id),
                (Utc::now() - last_ok).num_seconds().max(0)
            );
        }
    }

    out.push_str("# HELP laplace_flows flows per state\n# TYPE laplace_flows gauge\n");
    for state in FlowState::ALL {
        let count = statuses.iter().filter(|flow| flow.state == state).count();
        let _ = writeln!(out, "laplace_flows{{state=\"{}\"}} {count}", state.as_str());
    }

    out.push_str("# HELP laplace_scheduler_lag_seconds seconds since this replica's scheduler last ticked\n# TYPE laplace_scheduler_lag_seconds gauge\n");
    let _ = writeln!(out, "laplace_scheduler_lag_seconds {}", scheduler_lag(&app));

    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out).into_response()
}

/// prometheus label values escape backslash, quote and newline
fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// tells an outside service laplace is alive; when these stop, that service alerts.
/// a job, so one replica sends it, and only while the scheduler and postgres both work.
pub async fn heartbeat(app: &App) -> anyhow::Result<()> {
    let Some(url) = &app.heartbeat_url else {
        return Ok(());
    };
    app.client
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn escapes_label_values() {
        assert_eq!(label(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(label("x\ny"), "x\\ny");
    }
}
