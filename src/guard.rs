//! protections in front of every request: rate limits on the open endpoints, a cross-site check
//! on writes, and security headers on every response

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::App;

const WINDOW: Duration = Duration::from_secs(60);
/// sign-in pages per address and minute; a person needs a handful
const SIGN_IN_PER_MINUTE: u32 = 30;
/// wrong tokens per address and minute before that address has to wait
const BAD_TOKENS_PER_MINUTE: u32 = 20;
/// everything else that is open, per address and minute
const OTHER_PER_MINUTE: u32 = 600;
/// forget addresses beyond this many, so a flood of new addresses cannot grow memory without end
const MAX_TRACKED: usize = 50_000;

/// requests so far, and when the current window began
type Window = (u32, Instant);

/// counts requests per address in fixed one-minute windows, on this replica
#[derive(Default)]
pub struct Limiter {
    counts: Mutex<HashMap<(&'static str, IpAddr), Window>>,
}

impl Limiter {
    /// counts one request and says whether it is within `per_minute`
    pub fn allow(&self, bucket: &'static str, ip: IpAddr, per_minute: u32) -> bool {
        self.count(bucket, ip, 1) <= per_minute
    }

    /// whether the address has already used up `per_minute`, without counting this request
    pub fn exhausted(&self, bucket: &'static str, ip: IpAddr, per_minute: u32) -> bool {
        self.count(bucket, ip, 0) >= per_minute
    }

    fn count(&self, bucket: &'static str, ip: IpAddr, add: u32) -> u32 {
        let mut counts = self.counts.lock().unwrap();
        if counts.len() > MAX_TRACKED {
            counts.retain(|_, (_, started)| started.elapsed() < WINDOW);
        }
        let entry = counts.entry((bucket, ip)).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= WINDOW {
            *entry = (0, Instant::now());
        }
        entry.0 += add;
        entry.0
    }
}

/// the caller's address; behind a proxy only when LAPLACE_TRUST_PROXY says the proxy sets x-forwarded-for
fn client_ip(app: &App, request: &Request) -> IpAddr {
    let forwarded = app
        .trust_proxy
        .then(|| forwarded_client(request.headers().get("x-forwarded-for")?.to_str().ok()?))
        .flatten();
    forwarded.unwrap_or_else(|| {
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(IpAddr::from([0, 0, 0, 0]), |info| info.0.ip())
    })
}

/// the proxy in front appends the address it saw, so only the last entry can be trusted;
/// anything before it was sent by the client and could be made up to dodge the rate limits
fn forwarded_client(header: &str) -> Option<IpAddr> {
    header.rsplit(',').next()?.trim().parse().ok()
}

fn too_many() -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        "too many requests, wait a minute",
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    response
}

/// in front of the endpoints anyone can reach: sign-in, job reports, the wrapper download
pub async fn limit_open(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let ip = client_ip(&app, &request);
    let path = request.uri().path();
    let reports = path.starts_with("/ping") || path.starts_with("/calls");

    let allowed = if path.starts_with("/auth") {
        app.limiter.allow("sign-in", ip, SIGN_IN_PER_MINUTE)
    } else if reports {
        // job reports are only limited by wrong tokens, so a busy gateway is never throttled
        !app.limiter
            .exhausted("bad-token", ip, BAD_TOKENS_PER_MINUTE)
    } else {
        app.limiter.allow("other", ip, OTHER_PER_MINUTE)
    };
    if !allowed {
        return too_many();
    }

    let response = next.run(request).await;
    if reports && response.status() == StatusCode::UNAUTHORIZED {
        app.limiter.allow("bad-token", ip, BAD_TOKENS_PER_MINUTE);
    }
    response
}

/// a write to the dashboard's api must come from the dashboard itself. the session cookie is
/// already samesite=lax; this also stops a page on another site sending one through the browser.
pub async fn same_origin_writes(request: Request, next: Next) -> Response {
    let writes = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if writes && request.uri().path().starts_with("/api/") {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok());
        let origin_host = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok())
            .and_then(|o| url::Url::parse(o).ok())
            .map(|url| match url.port() {
                Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
                None => url.host_str().unwrap_or_default().to_owned(),
            });
        let forwarded_host = request
            .headers()
            .get("x-forwarded-host")
            .and_then(|h| h.to_str().ok());
        if let Some(origin_host) = origin_host
            && Some(origin_host.as_str()) != host
            && Some(origin_host.as_str()) != forwarded_host
        {
            return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
        }
    }
    next.run(request).await
}

/// the dashboard only loads its own scripts, fonts from google, and cannot be framed
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
font-src https://fonts.gstatic.com; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; \
form-action 'self'";

pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_per_address() {
        let limiter = Limiter::default();
        let (a, b) = (IpAddr::from([10, 0, 0, 1]), IpAddr::from([10, 0, 0, 2]));
        assert!((0..3).all(|_| limiter.allow("t", a, 3)));
        assert!(!limiter.allow("t", a, 3));
        assert!(
            limiter.allow("t", b, 3),
            "another address has its own count"
        );
        assert!(
            limiter.allow("other-bucket", a, 3),
            "buckets are counted apart"
        );
    }

    #[test]
    fn trusts_only_the_address_the_proxy_appended() {
        assert_eq!(
            forwarded_client("203.0.113.9, 198.51.100.7"),
            Some(IpAddr::from([198, 51, 100, 7]))
        );
        assert_eq!(
            forwarded_client("198.51.100.7"),
            Some(IpAddr::from([198, 51, 100, 7]))
        );
        assert_eq!(forwarded_client("made-up, nonsense"), None);
    }

    #[test]
    fn exhausted_does_not_count() {
        let limiter = Limiter::default();
        let a = IpAddr::from([10, 0, 0, 1]);
        assert!(!limiter.exhausted("t", a, 1));
        assert!(!limiter.exhausted("t", a, 1));
        limiter.allow("t", a, 1);
        assert!(limiter.exhausted("t", a, 1));
    }
}
