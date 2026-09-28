use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::access::{Caller, Role};
use crate::anomaly::{self, Metrics};
use crate::config::{Flow, Kind};
use crate::db::{self, Outcome};
use crate::inbound::Call;
use crate::registry::{self, PushKind};
use crate::schedule::{CronStyle, Schedule};
use crate::{App, databricks, reports, sso, tokens};
use crate::{status, timeline};

/// reachable without signing in: jobs report here with their own token, and sign-in itself lives here
pub fn open_routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/ping/{flow}", any(ping_ok))
        .route("/ping/{flow}/{result}", any(ping))
        .route("/calls/{flow}", post(calls))
        .route("/connect/heartbeat.ps1", get(heartbeat_script))
        .route("/auth/login", get(sso::login))
        .route("/auth/callback", get(sso::callback))
        .route("/auth/logout", get(sso::logout))
        .with_state(app)
}

/// the dashboard's api, behind sign-in when it is configured
pub fn signed_in_routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/me", get(me))
        .route("/api/flows", get(list_flows))
        .route("/api/flows/{flow}/history", get(history))
        .route("/api/timeline", get(get_timeline))
        .route("/api/reports", get(get_report))
        .route("/api/flows/{flow}/trust-host-key", post(trust_host_key))
        .route(
            "/api/flows/{flow}/acknowledge",
            post(acknowledge).delete(unacknowledge),
        )
        .route("/api/flows/{flow}/silence", post(silence).delete(unsilence))
        .route("/api/connect", get(connect_settings))
        .route("/api/databricks/jobs", get(databricks_jobs))
        .route(
            "/api/databricks/jobs/{job_id}/monitor",
            post(monitor_databricks_job),
        )
        .with_state(app)
}

pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        eprintln!("request failed: {error:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

async fn list_flows(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<status::Status>>> {
    Ok(Json(app.statuses().await?))
}

#[derive(Deserialize)]
struct Window {
    hours: Option<i64>,
}

async fn get_timeline(
    State(app): State<Arc<App>>,
    Query(window): Query<Window>,
) -> ApiResult<Json<timeline::Timeline>> {
    let hours = window.hours.unwrap_or(24).clamp(1, 24 * 7);
    Ok(Json(
        timeline::build(&app.pool, &app.flows().await?, hours).await?,
    ))
}

#[derive(Deserialize)]
struct Period {
    /// `2026-09`; the current month when left out
    month: Option<String>,
}

async fn get_report(
    State(app): State<Arc<App>>,
    Query(period): Query<Period>,
) -> ApiResult<Json<reports::Report>> {
    let (from, to) = month_bounds(app.settings().timezone, period.month.as_deref())
        .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "month: expected yyyy-mm".into()))?;
    Ok(Json(
        reports::build(
            &app.pool,
            &app.flows().await?,
            from,
            to,
            app.settings().timezone,
        )
        .await?,
    ))
}

/// the first instant of the month and of the next one, in the given zone
fn month_bounds(
    zone: chrono_tz::Tz,
    month: Option<&str>,
) -> Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
    use chrono::{Datelike, NaiveDate, TimeZone, Utc};
    let first = match month {
        Some(month) => NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").ok()?,
        None => Utc::now().with_timezone(&zone).date_naive().with_day(1)?,
    };
    let next = first.checked_add_months(chrono::Months::new(1))?;
    let start = |day: NaiveDate| {
        zone.from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
            .earliest()
            .map(|t| t.with_timezone(&Utc))
    };
    Some((start(first)?, start(next)?))
}

async fn me(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
) -> Json<serde_json::Value> {
    let config_error = caller
        .may(Role::Editor)
        .then(|| app.config_error.read().unwrap().clone())
        .flatten();
    Json(serde_json::json!({
        "email": caller.email,
        "role": caller.role,
        "sso": app.sso.is_some(),
        "config_error": config_error,
    }))
}

/// refuses the request unless the caller's role is at least `needed`
pub fn require(caller: &Caller, needed: Role) -> ApiResult<()> {
    if caller.may(needed) {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            format!("needs the {} role", needed.as_str()),
        ))
    }
}

async fn trust_host_key(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(flow): Path<String>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Operator)?;
    let flow = find(&app, &flow).await?;
    if db::trust_pending_host_key(&app.pool, &flow.id).await? {
        db::audit(&app.pool, &caller.email, "trusted a new host key", &flow.id).await?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(
            StatusCode::CONFLICT,
            format!("no changed host key waiting for {}", flow.id),
        ))
    }
}

#[derive(Deserialize)]
struct Note {
    #[serde(default)]
    note: String,
}

async fn acknowledge(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    Path(flow): Path<String>,
    Json(body): Json<Note>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Operator)?;
    let flow = find(&app, &flow).await?;
    let by = if app.sso.is_some() {
        caller.email.clone()
    } else {
        who(&headers)
    };
    db::acknowledge(&app.pool, &flow.id, body.note.trim(), &by).await?;
    db::audit(
        &app.pool,
        &by,
        "acknowledged",
        &format!("{}: {}", flow.id, body.note.trim()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unacknowledge(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(flow): Path<String>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Operator)?;
    let flow = find(&app, &flow).await?;
    db::clear_acknowledgement(&app.pool, &flow.id).await?;
    db::audit(
        &app.pool,
        &caller.email,
        "cleared an acknowledgement",
        &flow.id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Silence {
    hours: f64,
    #[serde(default)]
    note: String,
}

const MAX_SILENCE_HOURS: f64 = 24.0 * 30.0;

async fn silence(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(flow): Path<String>,
    Json(body): Json<Silence>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Operator)?;
    let flow = find(&app, &flow).await?;
    if !(body.hours > 0.0 && body.hours <= MAX_SILENCE_HOURS) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "hours: more than 0, at most 30 days".into(),
        ));
    }
    let until = chrono::Utc::now() + chrono::TimeDelta::seconds((body.hours * 3600.0) as i64);
    let note = Some(body.note.trim()).filter(|note| !note.is_empty());
    db::silence(&app.pool, &flow.id, until, note).await?;
    db::audit(
        &app.pool,
        &caller.email,
        "silenced",
        &format!("{} for {}h: {}", flow.id, body.hours, note.unwrap_or("")),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unsilence(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(flow): Path<String>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Operator)?;
    let flow = find(&app, &flow).await?;
    db::lift_silence(&app.pool, &flow.id).await?;
    db::audit(&app.pool, &caller.email, "lifted a silence", &flow.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// without built-in sign-in, the user as passed on by an sso proxy in front of us
fn who(headers: &HeaderMap) -> String {
    [
        "x-auth-request-email",
        "x-forwarded-email",
        "x-forwarded-user",
    ]
    .into_iter()
    .find_map(|name| headers.get(name)?.to_str().ok())
    .unwrap_or("someone")
    .to_owned()
}

async fn history(
    State(app): State<Arc<App>>,
    Path(flow): Path<String>,
) -> ApiResult<Json<Vec<db::Event>>> {
    let flow = find(&app, &flow).await?;
    let events = match flow.kind {
        Kind::Inbound { .. } => db::recent_calls(&app.pool, &flow.id, 60).await?,
        _ => db::recent_events(&app.pool, &flow.id, 50).await?,
    };
    Ok(Json(events))
}

/// what a job may say about itself in the query string when it reports
#[derive(Deserialize)]
struct Announce {
    /// how often the job is expected, e.g. `15m` or `1d`
    every: Option<String>,
    /// flows that must succeed first, comma separated
    after: Option<String>,
    /// who looks after the flow; their webhook gets its alerts
    owner: Option<String>,
    /// when the job is due, e.g. `0 2 * * *`; replaces `every` for lateness
    cron: Option<String>,
    /// business-day calendars, comma separated: `se`, `fi`
    calendar: Option<String>,
    timezone: Option<String>,
    /// how long past its deadline the job may still succeed, e.g. `45m`
    grace: Option<String>,
}

impl Announce {
    fn settings(
        &self,
        owners: &std::collections::HashMap<String, crate::config::Owner>,
    ) -> ApiResult<registry::Settings> {
        let every = duration("every", self.every.as_deref(), 60)?;
        let grace = duration("grace", self.grace.as_deref(), 0)?;
        let after = self.after.as_ref().map(|after| {
            after
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect()
        });
        // an owner missing from [owners] is ignored rather than refused: refusing would stop the
        // job's reports, and a monitor must never lose those over a misrouted alert
        let owner = self.owner.clone().filter(|owner| {
            let known = owners.contains_key(owner);
            if !known {
                eprintln!("a job named owner {owner}, which is not in [owners]; ignoring it");
            }
            known
        });
        let calendar: Option<Vec<String>> = self.calendar.as_ref().map(|calendar| {
            calendar
                .split(',')
                .map(str::trim)
                .filter(|code| !code.is_empty())
                .map(str::to_lowercase)
                .collect()
        });
        if let Some(cron) = &self.cron {
            let zone = Schedule::zone(self.timezone.as_deref(), chrono_tz::UTC)
                .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
            Schedule::parse(
                cron,
                CronStyle::Unix,
                zone,
                calendar.as_deref().unwrap_or_default(),
            )
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
        }
        Ok(registry::Settings {
            every,
            after,
            owner,
            cron: self.cron.clone(),
            calendar,
            timezone: self.timezone.clone(),
            grace,
        })
    }
}

fn duration(
    name: &str,
    value: Option<&str>,
    min_seconds: u64,
) -> ApiResult<Option<std::time::Duration>> {
    let Some(value) = value else { return Ok(None) };
    let parsed = humantime::parse_duration(value).map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            format!("{name}: not a duration: {value}"),
        )
    })?;
    if parsed.as_secs() < min_seconds {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("{name}: at least {min_seconds}s"),
        ));
    }
    Ok(Some(parsed))
}

async fn ping_ok(
    state: State<Arc<App>>,
    headers: HeaderMap,
    Path(flow): Path<String>,
    announce: Query<Announce>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    ping(state, headers, Path((flow, "0".into())), announce, body).await
}

const MAX_METRICS: usize = 20;

/// an optional json object of numbers in the ping body, e.g. {"rows": 1204331}
fn parse_metrics(body: &[u8]) -> ApiResult<Option<Metrics>> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let metrics: Metrics = serde_json::from_slice(body).map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "body: expected a json object of numbers".into(),
        )
    })?;
    if metrics.len() > MAX_METRICS || metrics.keys().any(|name| !registry::valid_id(name)) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("body: up to {MAX_METRICS} numbers with simple names"),
        ));
    }
    Ok(Some(metrics))
}

/// `start`, then the exit code: 0 is success, anything else a failure
async fn ping(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path((flow, result)): Path<(String, String)>,
    Query(announce): Query<Announce>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    authorize(&app, &headers).await?;
    let metrics = parse_metrics(&body)?;
    let flow = find_or_register(
        &app,
        &flow,
        PushKind::Heartbeat,
        announce.settings(&app.config().owners)?,
    )
    .await?;

    if result == "start" {
        db::record(&app.pool, &flow.id, Outcome::Start, "started", None).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let code: i64 = result.parse().map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "expected start or an exit code".into(),
        )
    })?;
    match code {
        0 => {
            db::record(&app.pool, &flow.id, Outcome::Ok, "exit 0", metrics.as_ref()).await?;
            anomaly::evaluate(&app.pool, &flow.id, flow.anomaly_tolerance).await?;
        }
        code => {
            let detail = format!("exited with code {code}");
            db::record(
                &app.pool,
                &flow.id,
                Outcome::Fail,
                &detail,
                metrics.as_ref(),
            )
            .await?
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Calls {
    One(Call),
    Many(Vec<Call>),
}

const MAX_CALLS_PER_REPORT: usize = 10_000;

async fn calls(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(flow): Path<String>,
    Query(announce): Query<Announce>,
    Json(body): Json<Calls>,
) -> ApiResult<StatusCode> {
    authorize(&app, &headers).await?;
    let flow = find_or_register(
        &app,
        &flow,
        PushKind::Inbound,
        announce.settings(&app.config().owners)?,
    )
    .await?;

    match body {
        Calls::One(call) => app.calls.add(&flow.id, &[call]),
        Calls::Many(calls) if calls.len() > MAX_CALLS_PER_REPORT => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("at most {MAX_CALLS_PER_REPORT} calls per report"),
            ));
        }
        Calls::Many(calls) => app.calls.add(&flow.id, &calls),
    }
    Ok(StatusCode::ACCEPTED)
}

async fn find(app: &App, id: &str) -> ApiResult<Flow> {
    app.flow(id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("unknown flow: {id}")))
}

/// a job reporting under a new name becomes a flow; this is what makes connecting a job one line
async fn find_or_register(
    app: &App,
    id: &str,
    kind: PushKind,
    settings: registry::Settings,
) -> ApiResult<Flow> {
    let flow = match app.flow(id).await? {
        Some(flow) if settings_changed(app, &flow, kind, &settings) => {
            register(app, id, kind, &settings).await?
        }
        Some(flow) => flow,
        None if !app.settings().auto_register => {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                format!("unknown flow: {id}"),
            ));
        }
        None if !registry::valid_id(id) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "flow ids use a-z, 0-9, '-', '_' and '.', up to 64".into(),
            ));
        }
        None => register(app, id, kind, &settings).await?,
    };

    if push_kind(&flow) != Some(kind) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("{} is a {} flow", flow.id, flow.kind.name()),
        ));
    }
    Ok(flow)
}

/// config flows are changed in the file; registered ones follow what their job sends
fn settings_changed(app: &App, flow: &Flow, kind: PushKind, settings: &registry::Settings) -> bool {
    app.config().flow(&flow.id).is_none()
        && push_kind(flow) == Some(kind)
        && (settings.every.is_some_and(|every| every != flow.every)
            || settings
                .after
                .as_ref()
                .is_some_and(|after| *after != flow.after)
            || settings
                .owner
                .as_ref()
                .is_some_and(|owner| Some(owner) != flow.owner.as_ref())
            || settings
                .cron
                .as_ref()
                .is_some_and(|cron| Some(cron) != flow.cron.as_ref())
            || settings
                .calendar
                .as_ref()
                .is_some_and(|calendar| *calendar != flow.calendar)
            || settings
                .timezone
                .as_ref()
                .is_some_and(|zone| Some(zone) != flow.timezone.as_ref())
            || settings.grace.is_some_and(|grace| grace != flow.grace))
}

async fn register(
    app: &App,
    id: &str,
    kind: PushKind,
    settings: &registry::Settings,
) -> ApiResult<Flow> {
    if let Some(after) = &settings.after {
        let mut flows = app.flows().await?;
        if let Some(unknown) = after
            .iter()
            .find(|upstream| !flows.iter().any(|flow| flow.id == **upstream))
        {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("after: unknown flow {unknown}"),
            ));
        }
        flows.retain(|flow| flow.id != id);
        let candidate = Flow {
            id: id.to_owned(),
            after: after.clone(),
            ..registry::placeholder(kind)
        };
        flows.push(candidate);
        if crate::pipeline::find_cycle(&flows).is_some() {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "after: that would make a circle".into(),
            ));
        }
    }
    Ok(registry::register(&app.pool, id, kind, settings).await?)
}

fn push_kind(flow: &Flow) -> Option<PushKind> {
    match flow.kind {
        Kind::Heartbeat { .. } => Some(PushKind::Heartbeat),
        Kind::Inbound { .. } => Some(PushKind::Inbound),
        _ => None,
    }
}

#[derive(Serialize)]
struct WorkspaceJob {
    #[serde(flatten)]
    job: databricks::Job,
    /// the flow already watching this job, if any
    monitored_as: Option<String>,
}

async fn databricks_jobs(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
) -> ApiResult<Json<Vec<WorkspaceJob>>> {
    require(&caller, Role::Editor)?;
    let workspace = databricks::Workspace::from_env()
        .map_err(|e| ApiError(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    let flows = app.flows().await?;
    let jobs = databricks::list_jobs(&app.client, &workspace)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("databricks: {e:#}")))?;
    Ok(Json(
        jobs.into_iter()
            .map(|job| WorkspaceJob {
                monitored_as: monitoring_flow(&flows, job.job_id),
                job,
            })
            .collect(),
    ))
}

fn monitoring_flow(flows: &[Flow], job_id: u64) -> Option<String> {
    flows
        .iter()
        .find(|flow| matches!(flow.kind, Kind::Databricks { job_id: id, .. } if id == job_id))
        .map(|flow| flow.id.clone())
}

#[derive(Deserialize)]
struct Monitor {
    owner: Option<String>,
}

async fn monitor_databricks_job(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(job_id): Path<u64>,
    Json(body): Json<Monitor>,
) -> ApiResult<Json<serde_json::Value>> {
    require(&caller, Role::Editor)?;
    let workspace = databricks::Workspace::from_env()
        .map_err(|e| ApiError(StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
    let flows = app.flows().await?;
    if let Some(existing) = monitoring_flow(&flows, job_id) {
        return Ok(Json(serde_json::json!({ "flow": existing })));
    }

    let job = databricks::get_job(&app.client, &workspace, job_id)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("databricks: {e:#}")))?;
    let mut id = databricks::flow_id(&job.name);
    if id.is_empty() || flows.iter().any(|flow| flow.id == id) {
        id = format!(
            "{}-{job_id}",
            if id.is_empty() { "databricks" } else { &id }
        )
        .chars()
        .take(64)
        .collect();
    }
    let owner = body
        .owner
        .as_deref()
        .filter(|owner| registry::valid_id(owner));
    let flow = registry::register_databricks(&app.pool, &id, &job, owner).await?;
    db::audit(
        &app.pool,
        &caller.email,
        "monitored a databricks job",
        &format!("{} as {}", job.name, flow.id),
    )
    .await?;
    Ok(Json(serde_json::json!({ "flow": flow.id })))
}

async fn connect_settings(State(app): State<Arc<App>>) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({
        "token_required": app.ingest_token.is_some() || app.tokens.required(&app.pool).await?,
        "auto_register": app.settings().auto_register,
        "public_url": app.public_url,
    })))
}

async fn heartbeat_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        include_str!("../scripts/heartbeat.ps1"),
    )
}

/// jobs need a token once one exists, either LAPLACE_TOKEN or one made in the dashboard
async fn authorize(app: &App, headers: &HeaderMap) -> ApiResult<()> {
    let sent = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    if app.ingest_token.is_none() && !app.tokens.required(&app.pool).await? {
        return Ok(());
    }
    let valid = match sent {
        Some(token)
            if app
                .ingest_token
                .as_deref()
                .is_some_and(|expected| tokens::hash(expected) == tokens::hash(token)) =>
        {
            true
        }
        Some(token) => app.tokens.valid(&app.pool, token).await?,
        None => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "missing or wrong token".into(),
        ))
    }
}
