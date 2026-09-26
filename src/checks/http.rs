use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::tls::TlsInfo;

use super::{Report, Success};
use crate::human;

const CERTIFICATE_WARNING: TimeDelta = TimeDelta::days(14);

pub async fn probe(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
    max_latency: Option<Duration>,
) -> Result<Report> {
    let started = Instant::now();
    let response = client.get(url).timeout(timeout).send().await?;
    let elapsed = started.elapsed();
    let millis = elapsed.as_millis() as i32;
    let warning = certificate_expiry(&response).and_then(expiry_warning);

    let status = response.status();
    if !status.is_success() {
        return Ok(Report {
            warning,
            ..Report::problem(format!("http {status}"))
        });
    }
    if max_latency.is_some_and(|limit| elapsed > limit) {
        return Ok(Report {
            warning,
            ..Report::problem(format!("slow response: {millis} ms"))
        });
    }
    Ok(Report {
        successes: vec![Success {
            at: Utc::now(),
            detail: format!("http {}", status.as_u16()),
            millis: Some(millis),
            metrics: Default::default(),
        }],
        warning,
        ..Report::default()
    })
}

/// only https responses carry a certificate; the client must be built with `tls_info(true)`
fn certificate_expiry(response: &reqwest::Response) -> Option<DateTime<Utc>> {
    let der = response.extensions().get::<TlsInfo>()?.peer_certificate()?;
    let (_, certificate) = x509_parser::parse_x509_certificate(der).ok()?;
    DateTime::from_timestamp(certificate.validity().not_after.timestamp(), 0)
}

fn expiry_warning(expires: DateTime<Utc>) -> Option<String> {
    let left = expires - Utc::now();
    (left < CERTIFICATE_WARNING).then(|| {
        format!(
            "tls certificate expires in {} ({})",
            human::duration(left.to_std().unwrap_or_default()),
            expires.format("%Y-%m-%d")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_only_inside_the_window() {
        assert!(expiry_warning(Utc::now() + TimeDelta::days(90)).is_none());
        let warning = expiry_warning(Utc::now() + TimeDelta::days(5)).unwrap();
        assert!(
            warning.starts_with("tls certificate expires in"),
            "{warning}"
        );
    }

    #[tokio::test]
    #[ignore = "needs network"]
    async fn reads_the_expiry_of_a_real_certificate() {
        let client = reqwest::Client::builder().tls_info(true).build().unwrap();
        let response = client.get("https://example.com").send().await.unwrap();
        let expires = certificate_expiry(&response).expect("no certificate read");
        assert!(expires > Utc::now(), "{expires}");
        println!("example.com certificate expires {expires}");
    }
}
