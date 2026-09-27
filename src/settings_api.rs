//! the settings page: access, job tokens, general settings and the audit log. admins only.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::access::{Caller, Role};
use crate::api::{ApiError, ApiResult, require};
use crate::settings::Settings;
use crate::{App, db, tokens};

pub fn routes(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/settings", get(overview))
        .route("/api/settings/general", put(save_general))
        .route("/api/settings/members", post(add_member))
        .route("/api/settings/members/{email}", delete(remove_member))
        .route("/api/settings/groups", post(add_group))
        .route("/api/settings/groups/{group}", delete(remove_group))
        .route("/api/settings/tokens", post(create_token))
        .route("/api/settings/tokens/{id}", delete(revoke_token))
        .route("/api/settings/audit", get(audit_log))
        .with_state(app)
}

#[derive(Serialize, sqlx::FromRow)]
struct Grant {
    name: String,
    role: String,
    added_by: String,
    added_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct Owner {
    id: String,
    webhook_env: Option<String>,
    webhook_set: bool,
    email: Vec<String>,
}

#[derive(Serialize)]
struct Window {
    name: String,
    cron: String,
    lasts: String,
    flows: Vec<String>,
    open_until: Option<DateTime<Utc>>,
    next_open: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct ConfigFlow {
    id: String,
    kind: &'static str,
    owner: Option<String>,
}

#[derive(Serialize)]
struct Overview {
    sso: bool,
    you: String,
    general: Settings,
    defaults: Settings,
    admins: Vec<String>,
    members: Vec<Grant>,
    groups: Vec<Grant>,
    tokens: Vec<tokens::Token>,
    legacy_token: bool,
    owners: Vec<Owner>,
    config_flows: Vec<ConfigFlow>,
    maintenance: Vec<Window>,
    config_error: Option<String>,
    email_ready: bool,
}

async fn overview(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
) -> ApiResult<Json<Overview>> {
    require(&caller, Role::Admin)?;
    let members = sqlx::query_as(
        "select email as name, role, added_by, added_at from members order by email",
    )
    .fetch_all(&app.pool)
    .await
    .map_err(anyhow::Error::from)?;
    let groups = sqlx::query_as(
        "select group_name as name, role, added_by, added_at from group_roles order by group_name",
    )
    .fetch_all(&app.pool)
    .await
    .map_err(anyhow::Error::from)?;

    let config = app.config();
    let mut owners: Vec<Owner> = config
        .owners
        .iter()
        .map(|(id, owner)| Owner {
            id: id.clone(),
            webhook_env: owner.webhook_env.clone(),
            webhook_set: owner
                .webhook_env
                .as_deref()
                .and_then(crate::optional_env)
                .is_some(),
            email: owner.email.clone(),
        })
        .collect();
    owners.sort_by(|a, b| a.id.cmp(&b.id));
    let zone = app.settings().timezone;
    let mut maintenance: Vec<Window> = config
        .maintenance
        .iter()
        .map(|(name, window)| Window {
            name: name.clone(),
            cron: window.cron.clone(),
            lasts: crate::human::duration(window.lasts),
            flows: window.flows.clone(),
            open_until: window.open_until(zone, Utc::now()).ok().flatten(),
            next_open: window.next_opening(zone, Utc::now()).ok().flatten(),
        })
        .collect();
    maintenance.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(Overview {
        sso: app.sso.is_some(),
        you: caller.email,
        general: app.settings(),
        defaults: app.default_settings.clone(),
        admins: app.admins.clone(),
        members,
        groups,
        tokens: tokens::list(&app.pool).await?,
        legacy_token: app.ingest_token.is_some(),
        owners,
        config_flows: config
            .flows
            .iter()
            .map(|flow| ConfigFlow {
                id: flow.id.clone(),
                kind: flow.kind.name(),
                owner: flow.owner.clone(),
            })
            .collect(),
        maintenance,
        config_error: app.config_error.read().unwrap().clone(),
        email_ready: app.mailer.is_some(),
    }))
}

async fn save_general(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Json(changes): Json<HashMap<String, String>>,
) -> ApiResult<Json<Settings>> {
    require(&caller, Role::Admin)?;
    let saved = Settings::save(&app.pool, &app.settings(), &changes, &caller.email)
        .await
        .map_err(|error| ApiError(StatusCode::BAD_REQUEST, format!("{error:#}")))?;
    app.reload_settings().await?;
    let mut changed: Vec<String> = changes
        .iter()
        .map(|(key, value)| format!("{key} = {value}"))
        .collect();
    changed.sort();
    db::audit(
        &app.pool,
        &caller.email,
        "changed settings",
        &changed.join(", "),
    )
    .await?;
    Ok(Json(saved))
}

#[derive(Deserialize)]
struct NewGrant {
    name: String,
    role: String,
}

fn parse_role(role: &str) -> ApiResult<Role> {
    Role::parse(role).ok_or_else(|| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "role: viewer, operator, editor or admin".into(),
        )
    })
}

/// an admin who is not in LAPLACE_ADMINS could otherwise lock themselves out
fn not_yourself(app: &App, caller: &Caller, email: &str) -> ApiResult<()> {
    let protected_by_env = app
        .admins
        .iter()
        .any(|admin| admin.eq_ignore_ascii_case(&caller.email));
    if email.eq_ignore_ascii_case(&caller.email) && !protected_by_env {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "you cannot change your own access; ask another admin".into(),
        ));
    }
    Ok(())
}

async fn add_member(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Json(grant): Json<NewGrant>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Admin)?;
    let email = grant.name.trim().to_lowercase();
    if !email.contains('@') || email.len() > 254 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "email: not an email address".into(),
        ));
    }
    let role = parse_role(&grant.role)?;
    not_yourself(&app, &caller, &email)?;
    sqlx::query(
        "insert into members (email, role, added_by) values ($1, $2, $3)
         on conflict (email) do update set role = excluded.role, added_by = excluded.added_by, added_at = now()",
    )
    .bind(&email)
    .bind(role.as_str())
    .bind(&caller.email)
    .execute(&app.pool)
    .await
    .map_err(anyhow::Error::from)?;
    db::audit(
        &app.pool,
        &caller.email,
        "gave access",
        &format!("{email} as {}", role.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(email): Path<String>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Admin)?;
    not_yourself(&app, &caller, &email)?;
    sqlx::query("delete from members where lower(email) = lower($1)")
        .bind(&email)
        .execute(&app.pool)
        .await
        .map_err(anyhow::Error::from)?;
    db::audit(&app.pool, &caller.email, "removed access", &email).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_group(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Json(grant): Json<NewGrant>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Admin)?;
    let group = grant.name.trim();
    if group.is_empty() || group.len() > 200 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "group: a name or object id".into(),
        ));
    }
    let role = parse_role(&grant.role)?;
    sqlx::query(
        "insert into group_roles (group_name, role, added_by) values ($1, $2, $3)
         on conflict (group_name) do update set role = excluded.role, added_by = excluded.added_by, added_at = now()",
    )
    .bind(group)
    .bind(role.as_str())
    .bind(&caller.email)
    .execute(&app.pool)
    .await
    .map_err(anyhow::Error::from)?;
    db::audit(
        &app.pool,
        &caller.email,
        "gave a group access",
        &format!("{group} as {}", role.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_group(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(group): Path<String>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Admin)?;
    sqlx::query("delete from group_roles where group_name = $1")
        .bind(&group)
        .execute(&app.pool)
        .await
        .map_err(anyhow::Error::from)?;
    db::audit(&app.pool, &caller.email, "removed a group's access", &group).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewToken {
    name: String,
}

async fn create_token(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<NewToken>,
) -> ApiResult<Json<serde_json::Value>> {
    require(&caller, Role::Admin)?;
    let name = body.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "name: say what the token is for".into(),
        ));
    }
    let token = tokens::create(&app.pool, name, &caller.email).await?;
    app.tokens.forget();
    db::audit(&app.pool, &caller.email, "created a job token", name).await?;
    Ok(Json(serde_json::json!({ "token": token })))
}

async fn revoke_token(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require(&caller, Role::Admin)?;
    let Some(name) = tokens::revoke(&app.pool, id).await? else {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "no such active token".into(),
        ));
    };
    app.tokens.forget();
    db::audit(&app.pool, &caller.email, "revoked a job token", &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, sqlx::FromRow)]
struct AuditEntry {
    at: DateTime<Utc>,
    by: String,
    action: String,
    detail: String,
}

async fn audit_log(
    State(app): State<Arc<App>>,
    Extension(caller): Extension<Caller>,
) -> ApiResult<Json<Vec<AuditEntry>>> {
    require(&caller, Role::Admin)?;
    let entries = sqlx::query_as(
        "select at, by, action, detail from audit order by at desc, id desc limit 200",
    )
    .fetch_all(&app.pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(Json(entries))
}
