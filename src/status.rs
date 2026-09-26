use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use sqlx::PgPool;

use chrono_tz::Tz;

use crate::config::{Config, Flow, Kind};
use crate::db::{self, CallTotals, FlowState};
use crate::schedule::{Due, Schedule};
use crate::{human, pipeline};

/// below this many calls in the window one error would swing the rate too much to alert on
const MIN_CALLS_FOR_ERROR_RATE: i64 = 10;

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    /// works now, but needs attention soon or looked unusual
    Warning,
    Late,
    Failed,
    Pending,
    /// late or pending because an upstream step in its pipeline is broken
    Blocked,
    /// broken inside a planned maintenance window, so nobody is alerted
    Maintenance,
}

impl State {
    pub const ALL: [State; 7] = [
        State::Ok,
        State::Warning,
        State::Late,
        State::Failed,
        State::Pending,
        State::Blocked,
        State::Maintenance,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::Ok => "ok",
            State::Warning => "warning",
            State::Late => "late",
            State::Failed => "failed",
            State::Pending => "pending",
            State::Blocked => "blocked",
            State::Maintenance => "maintenance",
        }
    }

    pub fn needs_attention(self) -> bool {
        matches!(self, State::Warning | State::Late | State::Failed)
    }

    /// steps after a broken one cannot succeed either
    pub fn is_broken(self) -> bool {
        matches!(
            self,
            State::Late | State::Failed | State::Blocked | State::Maintenance
        )
    }

    pub fn parse(value: &str) -> Option<Self> {
        State::ALL.into_iter().find(|state| state.as_str() == value)
    }
}

#[derive(Serialize)]
pub struct Acknowledged {
    pub note: String,
    pub by: String,
    pub at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct Status {
    pub id: String,
    pub kind: &'static str,
    pub source: String,
    pub every: String,
    pub after: Vec<String>,
    pub owner: Option<String>,
    pub acknowledged: Option<Acknowledged>,
    pub silenced_until: Option<DateTime<Utc>>,
    pub silence_note: Option<String>,
    pub state: State,
    pub detail: String,
    pub last_ok: Option<DateTime<Utc>>,
    /// a changed sftp host key is waiting to be confirmed
    pub host_key_pending: bool,
    #[serde(skip)]
    pub alerted_state: String,
    #[serde(skip)]
    pub recorded_state: Option<String>,
}

pub async fn all(
    pool: &PgPool,
    flows: &[Flow],
    zone: chrono_tz::Tz,
) -> anyhow::Result<Vec<Status>> {
    let states = db::flow_states(pool).await?;
    let calls = db::call_totals_since(pool, Utc::now() - TimeDelta::hours(1)).await?;
    let pending_host_keys = db::flows_with_pending_host_key(pool).await?;

    let mut statuses: Vec<Status> = flows
        .iter()
        .map(|flow| {
            let state = states.get(&flow.id).cloned().unwrap_or_default();
            let totals = calls.get(&flow.id).copied().unwrap_or_default();
            let schedule = flow.schedule(zone).ok().flatten();
            let (state_now, detail) = evaluate(flow, schedule.as_ref(), &state, totals);
            Status {
                id: flow.id.clone(),
                kind: flow.kind.name(),
                source: flow.source_label(),
                every: schedule
                    .as_ref()
                    .map_or_else(|| human::duration(flow.every), Schedule::label),
                after: flow.after.clone(),
                owner: flow.owner.clone(),
                acknowledged: match (&state.ack_note, &state.ack_by, state.ack_at) {
                    (Some(note), Some(by), Some(at)) => Some(Acknowledged {
                        note: note.clone(),
                        by: by.clone(),
                        at,
                    }),
                    _ => None,
                },
                silenced_until: state.silenced_until.filter(|until| *until > Utc::now()),
                silence_note: state.silence_note.clone(),
                state: state_now,
                detail,
                last_ok: state.last_ok,
                host_key_pending: pending_host_keys.contains(&flow.id),
                alerted_state: state.alerted_state,
                recorded_state: state.recorded_state,
            }
        })
        .collect();
    pipeline::block_downstream(flows, &mut statuses);
    Ok(statuses)
}

/// a flow that is broken while a maintenance window covering it is open is in maintenance
pub fn apply_maintenance(statuses: &mut [Status], config: &Config, zone: Tz, now: DateTime<Utc>) {
    for status in statuses {
        if matches!(status.state, State::Ok | State::Pending) {
            continue;
        }
        let open = config.maintenance.iter().find_map(|(name, window)| {
            let closes = window.open_until(zone, now).ok().flatten()?;
            window.covers(&status.id).then_some((name, closes))
        });
        if let Some((name, closes)) = open {
            status.state = State::Maintenance;
            status.detail = format!(
                "maintenance {name} until {}; {}",
                closes
                    .with_timezone(&zone)
                    .format("%a %H:%M")
                    .to_string()
                    .to_lowercase(),
                status.detail
            );
        }
    }
}

fn warning(state: &FlowState) -> Option<String> {
    let warnings: Vec<&str> = [&state.check_warning, &state.anomaly]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    (!warnings.is_empty()).then(|| warnings.join("; "))
}

fn evaluate(
    flow: &Flow,
    schedule: Option<&Schedule>,
    state: &FlowState,
    calls: CallTotals,
) -> (State, String) {
    if let Some(problem) = &state.problem {
        return (State::Failed, problem.clone());
    }

    if let Kind::Heartbeat {
        max_runtime: Some(limit),
    } = flow.kind
        && let Some(started) = state.running_since
        && (Utc::now() - started)
            .to_std()
            .is_ok_and(|running| running > limit)
    {
        return (
            State::Failed,
            format!("still running after {}", human::since(started)),
        );
    }

    if let Kind::Inbound { max_error_rate } = flow.kind {
        let total = calls.ok + calls.client_errors + calls.server_errors;
        let rate = calls.server_errors as f64 / total.max(1) as f64;
        if total >= MIN_CALLS_FOR_ERROR_RATE && rate > max_error_rate {
            return (
                State::Failed,
                format!("{:.0}% server errors in the last hour", rate * 100.0),
            );
        }
    }

    let Some(last_ok) = state.last_ok else {
        return (State::Pending, "no data yet".into());
    };
    let (late, next) = match schedule {
        Some(schedule) => match schedule.check(last_ok, flow.grace, Utc::now()) {
            Ok(Due::Missed { deadline }) => (
                Some(format!("was due by {}", schedule.local(deadline))),
                None,
            ),
            Ok(Due::Met { next_deadline }) => {
                (None, next_deadline.map(|next| schedule.local(next)))
            }
            Ok(Due::NotYet) => (None, None),
            Err(error) => (Some(format!("schedule: {error:#}")), None),
        },
        None => {
            let overdue = (Utc::now() - last_ok)
                .to_std()
                .is_ok_and(|age| age > flow.every + flow.grace);
            let late = format!(
                "nothing for {}, expected every {}",
                human::since(last_ok),
                human::duration(flow.every)
            );
            (overdue.then_some(late), None)
        }
    };

    if let Some(late) = late {
        (State::Late, late)
    } else if let Some(warning) = warning(state) {
        (State::Warning, warning)
    } else {
        let next = next
            .map(|next| format!(", next due {next}"))
            .unwrap_or_default();
        (
            State::Ok,
            format!("last ok {} ago{next}", human::since(last_ok)),
        )
    }
}
