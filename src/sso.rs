//! sign-in with any openid connect provider: entra id, google, okta, keycloak.
//! the session lives in an encrypted cookie, so every replica can serve every request.

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar, SameSite};
use base64::{Engine, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeDelta, Utc};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse,
};
use serde::{Deserialize, Serialize};

use crate::{App, access};

const SESSION: &str = "laplace_session";
const PENDING: &str = "laplace_login";
const SESSION_LENGTH: TimeDelta = TimeDelta::hours(12);

type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

pub struct Sso {
    issuer: IssuerUrl,
    client_id: ClientId,
    client_secret: Option<ClientSecret>,
    public_url: String,
    allowed_domains: Vec<String>,
    http: openidconnect::reqwest::Client,
    key: Key,
}

/// what the session cookie holds; the role is looked up on each request, so changes apply at once
#[derive(Clone, Serialize, Deserialize)]
pub struct User {
    pub email: String,
    expires: DateTime<Utc>,
}

/// what the provider must hand back unchanged, kept between leaving for sign-in and returning
#[derive(Serialize, Deserialize)]
struct Pending {
    state: String,
    nonce: String,
    verifier: String,
    return_to: String,
}

impl Sso {
    /// sign-in is on when LAPLACE_OIDC_ISSUER is set
    pub fn from_env(public_url: Option<&str>) -> Result<Option<Self>> {
        let Some(issuer) = crate::optional_env("LAPLACE_OIDC_ISSUER") else {
            return Ok(None);
        };
        let public_url =
            public_url.context("sign-in needs LAPLACE_PUBLIC_URL for the callback address")?;
        let client_id = crate::optional_env("LAPLACE_OIDC_CLIENT_ID")
            .context("LAPLACE_OIDC_CLIENT_ID is not set")?;
        let key = crate::optional_env("LAPLACE_SESSION_KEY")
            .context("sign-in needs LAPLACE_SESSION_KEY, the same on every replica: `openssl rand -base64 64`")?;
        let key = Key::try_from(
            STANDARD
                .decode(key.trim())
                .context("LAPLACE_SESSION_KEY is not base64")?
                .as_slice(),
        )
        .map_err(|_| anyhow::anyhow!("LAPLACE_SESSION_KEY must be at least 64 bytes"))?;

        Ok(Some(Self {
            issuer: IssuerUrl::new(issuer)?,
            client_id: ClientId::new(client_id),
            client_secret: crate::optional_env("LAPLACE_OIDC_CLIENT_SECRET").map(ClientSecret::new),
            public_url: public_url.to_owned(),
            allowed_domains: crate::optional_env("LAPLACE_OIDC_ALLOWED_DOMAINS")
                .map(|domains| {
                    domains
                        .split(',')
                        .map(|d| d.trim().to_lowercase())
                        .filter(|d| !d.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            // following redirects would let a provider send us anywhere
            http: openidconnect::reqwest::ClientBuilder::new()
                .redirect(openidconnect::reqwest::redirect::Policy::none())
                // sign-ins are rare; a kept-alive connection is usually dropped by some proxy before the next one
                .pool_max_idle_per_host(0)
                .build()?,
            key,
        }))
    }

    /// discovery on each sign-in: rare enough not to cache, and picks up rotated keys for free
    async fn client(&self) -> Result<Client> {
        let metadata =
            CoreProviderMetadata::discover_async(self.issuer.clone(), &self.http).await?;
        let redirect = RedirectUrl::new(format!("{}/auth/callback", self.public_url))?;
        Ok(CoreClient::from_provider_metadata(
            metadata,
            self.client_id.clone(),
            self.client_secret.clone(),
        )
        .set_redirect_uri(redirect))
    }

    fn allowed(&self, email: &str) -> bool {
        let domain = email.rsplit('@').next().unwrap_or_default().to_lowercase();
        self.allowed_domains.is_empty() || self.allowed_domains.contains(&domain)
    }

    fn secure(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}

impl Sso {
    fn jar(&self, headers: &HeaderMap) -> PrivateCookieJar {
        PrivateCookieJar::from_headers(headers, self.key.clone())
    }

    /// the signed-in user, if the session cookie is ours and still fresh
    pub fn session(&self, headers: &HeaderMap) -> Option<User> {
        self.jar(headers)
            .get(SESSION)
            .and_then(|cookie| serde_json::from_str::<User>(cookie.value()).ok())
            .filter(|user| user.expires > Utc::now())
    }

    pub fn login_url(&self, return_to: &str) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("return_to", return_to)
            .finish();
        format!("{}/auth/login?{query}", self.public_url)
    }

    pub fn logout_url(&self) -> String {
        format!("{}/auth/logout", self.public_url)
    }
}

fn cookie(name: &'static str, value: String, sso: &Sso, lasts: TimeDelta) -> Cookie<'static> {
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .secure(sso.secure())
        .same_site(SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(lasts.num_seconds()))
        .build()
}

/// keeps a return path on this site, so the sign-in round trip cannot be used to redirect elsewhere
fn local_path(path: Option<&str>) -> String {
    match path {
        Some(path) if path.starts_with('/') && !path.starts_with("//") => path.to_owned(),
        _ => "/".to_owned(),
    }
}

#[derive(Deserialize)]
pub struct LoginQuery {
    return_to: Option<String>,
}

pub async fn login(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(query): Query<LoginQuery>,
) -> Response {
    let Some(sso) = &app.sso else {
        return Redirect::to("/").into_response();
    };
    let jar = sso.jar(&headers);
    let client = match sso.client().await {
        Ok(client) => client,
        Err(error) => {
            return unavailable(
                &error,
                &sso.login_url(&local_path(query.return_to.as_deref())),
            );
        }
    };

    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(challenge)
        .url();

    let pending = Pending {
        state: state.secret().clone(),
        nonce: nonce.secret().clone(),
        verifier: verifier.secret().clone(),
        return_to: local_path(query.return_to.as_deref()),
    };
    let pending = serde_json::to_string(&pending).expect("serializes");
    let jar = jar.add(cookie(PENDING, pending, sso, TimeDelta::minutes(10)));
    (jar, Redirect::to(url.as_str())).into_response()
}

#[derive(Deserialize)]
pub struct Callback {
    code: String,
    state: String,
}

pub async fn callback(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(callback): Query<Callback>,
) -> Response {
    let Some(sso) = &app.sso else {
        return Redirect::to("/").into_response();
    };
    let jar = sso.jar(&headers);
    let pending = jar
        .get(PENDING)
        .and_then(|c| serde_json::from_str::<Pending>(c.value()).ok());
    let Some(pending) = pending.filter(|pending| pending.state == callback.state) else {
        let again = sso.login_url("/");
        return page(
            StatusCode::BAD_REQUEST,
            "sign-in expired",
            "the sign-in took too long or was started in another tab.",
            &again,
            "try again",
        );
    };

    let identity = match verified_identity(sso, callback.code, &pending).await {
        Ok(identity) => identity,
        Err(error) => {
            eprintln!("sign-in failed: {error:#}");
            let again = sso.login_url(&pending.return_to);
            return page(
                StatusCode::UNAUTHORIZED,
                "sign-in failed",
                "the answer from the sign-in provider could not be verified.",
                &again,
                "try again",
            );
        }
    };
    if !sso.allowed(&identity.email) {
        let message = format!(
            "{} is not in a domain allowed to use laplace.",
            identity.email
        );
        return page(
            StatusCode::FORBIDDEN,
            "not allowed",
            &message,
            &sso.logout_url(),
            "sign in with another account",
        );
    }
    if let Err(error) = access::remember_groups(&app.pool, &identity.email, &identity.groups).await
    {
        eprintln!("storing groups failed: {error:#}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let user = User {
        email: identity.email,
        expires: Utc::now() + SESSION_LENGTH,
    };
    let jar = jar.remove(Cookie::build(PENDING).path("/")).add(cookie(
        SESSION,
        serde_json::to_string(&user).expect("serializes"),
        sso,
        SESSION_LENGTH,
    ));
    (
        jar,
        Redirect::to(&format!("{}{}", sso.public_url, pending.return_to)),
    )
        .into_response()
}

struct Identity {
    email: String,
    groups: Vec<String>,
}

/// exchanges the code, then checks the id token's signature, issuer, audience, expiry and nonce
async fn verified_identity(sso: &Sso, code: String, pending: &Pending) -> Result<Identity> {
    let client = sso.client().await?;
    let tokens = client
        .exchange_code(AuthorizationCode::new(code))?
        .set_pkce_verifier(PkceCodeVerifier::new(pending.verifier.clone()))
        .request_async(&sso.http)
        .await?;
    let id_token = tokens.id_token().context("the provider sent no id token")?;
    let claims = id_token.claims(
        &client.id_token_verifier(),
        &Nonce::new(pending.nonce.clone()),
    )?;

    // entra id only sends `email` when configured to, but its username is the email address
    let email = claims
        .email()
        .map(|email| email.as_str().to_owned())
        .or_else(|| {
            claims
                .preferred_username()
                .map(|name| name.as_str().to_owned())
        })
        .filter(|email| email.contains('@'))
        .context("the id token has no email address")?;
    Ok(Identity {
        email,
        groups: groups(&id_token.to_string()),
    })
}

/// the `groups` claim of a token whose signature was already checked
fn groups(verified_token: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Claims {
        #[serde(default)]
        groups: Vec<String>,
    }
    verified_token
        .split('.')
        .nth(1)
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|json| serde_json::from_slice::<Claims>(&json).ok())
        .map(|claims| claims.groups)
        .unwrap_or_default()
}

/// shows a page rather than bouncing straight back to the provider, which would sign the user in again
pub async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some(sso) = &app.sso else {
        return Redirect::to("/").into_response();
    };
    let jar = sso.jar(&headers).remove(Cookie::build(SESSION).path("/"));
    let page = page(
        StatusCode::OK,
        "signed out",
        "you are signed out of laplace.",
        &sso.login_url("/"),
        "sign in again",
    );
    (jar, page).into_response()
}

fn unavailable(error: &anyhow::Error, retry: &str) -> Response {
    eprintln!("sign-in provider unreachable: {error:#}");
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        "sign-in is unavailable",
        "the sign-in provider cannot be reached. try again in a minute.",
        retry,
        "try again",
    )
}

/// a small page in the dashboard's colours for sign-in outcomes
pub fn page(
    status: StatusCode,
    title: &str,
    message: &str,
    link: &str,
    link_text: &str,
) -> Response {
    let html = PAGE
        .replace("{title}", &escape(title))
        .replace("{message}", &escape(message))
        .replace("{link}", &escape(link))
        .replace("{link_text}", &escape(link_text));
    (status, Html(html)).into_response()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>laplace</title>
<style>
  :root { --bg: #0b0c07; --fg: #e1e5e5; --muted: #878b88; --line: rgb(225 229 229 / 0.14); --accent: #d4a537; color-scheme: dark; }
  @media (prefers-color-scheme: light) {
    :root { --bg: #e6e9e9; --fg: #0b0c07; --muted: #5d605b; --line: rgb(11 12 7 / 0.14); --accent: #8c6508; color-scheme: light; }
  }
  body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: var(--bg); color: var(--fg);
         font: 15px/1.6 "Archivo", ui-sans-serif, system-ui, sans-serif; }
  main { width: min(440px, calc(100% - 32px)); }
  .name { font-size: 13px; font-weight: 700; letter-spacing: 0.35em; text-transform: uppercase; }
  .rule { height: 1px; background: var(--line); margin: 20px 0 48px; }
  h1 { font-size: 30px; font-weight: 500; margin: 0 0 8px; }
  p { color: var(--muted); margin: 0 0 32px; }
  a { display: inline-block; border: 1px solid var(--line); border-radius: 4px; padding: 8px 14px; color: var(--fg); text-decoration: none; }
  a:hover, a:focus-visible { border-color: var(--accent); outline: none; }
</style>
</head>
<body>
<main>
  <div class="name">laplace</div>
  <div class="rule"></div>
  <h1>{title}</h1>
  <p>{message}</p>
  <a href="{link}">{link_text}</a>
</main>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::local_path;

    #[test]
    fn return_paths_stay_on_this_site() {
        assert_eq!(local_path(Some("/#reports")), "/#reports");
        assert_eq!(local_path(Some("//evil.example")), "/");
        assert_eq!(local_path(Some("https://evil.example")), "/");
        assert_eq!(local_path(None), "/");
    }
}
