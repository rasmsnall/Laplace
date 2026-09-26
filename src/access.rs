//! who may do what. roles are global and ordered: each includes the ones below it.

use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::App;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// sees flows, pipelines, the timeline and reports
    Viewer,
    /// acknowledges, silences, trusts a changed host key
    Operator,
    /// connects jobs and picks databricks jobs
    Editor,
    /// changes access, tokens and settings
    Admin,
}

impl Role {
    pub fn parse(value: &str) -> Option<Self> {
        [Role::Viewer, Role::Operator, Role::Editor, Role::Admin]
            .into_iter()
            .find(|role| role.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }
}

/// the person behind a dashboard request, put on every request by `identify`
#[derive(Clone, Serialize)]
pub struct Caller {
    pub email: String,
    pub role: Role,
}

impl Caller {
    pub fn may(&self, needed: Role) -> bool {
        self.role >= needed
    }
}

/// the highest role the person has: from LAPLACE_ADMINS, their own entry, or any of their groups
pub async fn role_of(pool: &PgPool, admins: &[String], email: &str) -> Result<Option<Role>> {
    if admins.iter().any(|admin| admin.eq_ignore_ascii_case(email)) {
        return Ok(Some(Role::Admin));
    }
    let roles: Vec<String> = sqlx::query_scalar(
        "select role from members where lower(email) = lower($1)
         union all
         select g.role from group_roles g
         join user_groups u on g.group_name = any(u.groups)
         where lower(u.email) = lower($1)",
    )
    .bind(email)
    .fetch_all(pool)
    .await?;
    Ok(roles.iter().filter_map(|role| Role::parse(role)).max())
}

pub async fn remember_groups(pool: &PgPool, email: &str, groups: &[String]) -> Result<()> {
    sqlx::query(
        "insert into user_groups (email, groups) values (lower($1), $2)
         on conflict (email) do update set groups = excluded.groups, signed_in_at = now()",
    )
    .bind(email)
    .bind(groups)
    .execute(pool)
    .await?;
    Ok(())
}

/// in front of the dashboard and its api. without sign-in everyone is an admin, which only
/// suits running laplace on your own machine.
pub async fn identify(State(app): State<Arc<App>>, mut request: Request, next: Next) -> Response {
    let Some(sso) = &app.sso else {
        request.extensions_mut().insert(Caller {
            email: "local".into(),
            role: Role::Admin,
        });
        return next.run(request).await;
    };

    let is_api = request.uri().path().starts_with("/api/");
    let Some(user) = sso.session(request.headers()) else {
        if is_api {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({ "error": "sign in first" })),
            )
                .into_response();
        }
        let here = request.uri().path_and_query().map_or("/", |p| p.as_str());
        return Redirect::to(&sso.login_url(here)).into_response();
    };

    match role_of(&app.pool, &app.admins, &user.email).await {
        Ok(Some(role)) => {
            request.extensions_mut().insert(Caller {
                email: user.email,
                role,
            });
            next.run(request).await
        }
        Ok(None) if is_api => (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "error": "no access yet, ask an admin" })),
        )
            .into_response(),
        Ok(None) => crate::sso::page(
            StatusCode::FORBIDDEN,
            "no access yet",
            &format!(
                "{} is signed in but has no role in laplace. ask an admin to add you.",
                user.email
            ),
            &sso.logout_url(),
            "sign in with another account",
        ),
        Err(error) => {
            eprintln!("looking up access failed: {error:#}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_roles_include_lower_ones() {
        let editor = Caller {
            email: "a@b".into(),
            role: Role::Editor,
        };
        assert!(editor.may(Role::Viewer) && editor.may(Role::Operator) && editor.may(Role::Editor));
        assert!(!editor.may(Role::Admin));
    }
}
