use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use lettre::message::{Mailbox, MultiPart};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;
use serde_json::json;

use crate::optional_env;

/// a token is renewed this long before it runs out, so a send never races its expiry
const TOKEN_MARGIN: Duration = Duration::from_secs(120);

pub enum Mailer {
    Smtp {
        transport: AsyncSmtpTransport<Tokio1Executor>,
        from: Mailbox,
    },
    Graph(Graph),
}

/// microsoft graph, for microsoft 365 tenants where smtp with a password is turned off
pub struct Graph {
    client: reqwest::Client,
    tenant: String,
    client_id: String,
    client_secret: String,
    /// the mailbox alerts are sent from
    from: String,
    login_host: String,
    graph_host: String,
    token: Mutex<Option<(String, Instant)>>,
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    expires_in: u64,
}

impl Mailer {
    /// graph when LAPLACE_GRAPH_TENANT_ID is set, otherwise smtp when LAPLACE_SMTP_URL is,
    /// such as `smtp://user:password@smtp.example.com:587?tls=required`
    pub fn from_env(client: &reqwest::Client) -> Result<Option<Self>> {
        let graph = optional_env("LAPLACE_GRAPH_TENANT_ID");
        let smtp = optional_env("LAPLACE_SMTP_URL");
        if graph.is_none() && smtp.is_none() {
            return Ok(None);
        }
        let from =
            optional_env("LAPLACE_MAIL_FROM").context("email alerts need LAPLACE_MAIL_FROM")?;
        from.parse::<lettre::Address>()
            .context("LAPLACE_MAIL_FROM is not an email address")?;

        if let Some(tenant) = graph {
            let needed = |name: &str| {
                optional_env(name).with_context(|| format!("LAPLACE_GRAPH_TENANT_ID needs {name}"))
            };
            return Ok(Some(Mailer::Graph(Graph {
                client: client.clone(),
                tenant,
                client_id: needed("LAPLACE_GRAPH_CLIENT_ID")?,
                client_secret: needed("LAPLACE_GRAPH_CLIENT_SECRET")?,
                from,
                login_host: optional_env("LAPLACE_GRAPH_LOGIN_HOST")
                    .unwrap_or_else(|| "https://login.microsoftonline.com".into()),
                graph_host: optional_env("LAPLACE_GRAPH_HOST")
                    .unwrap_or_else(|| "https://graph.microsoft.com".into()),
                token: Mutex::default(),
            })));
        }

        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(&smtp.unwrap_or_default())
            .context("LAPLACE_SMTP_URL is not a valid smtp url")?
            .build();
        Ok(Some(Mailer::Smtp {
            transport,
            from: from.parse()?,
        }))
    }

    pub async fn send(
        &self,
        to: &[String],
        subject: &str,
        text: String,
        html: String,
    ) -> Result<()> {
        match self {
            Mailer::Smtp { transport, from } => {
                let mut message = Message::builder().from(from.clone()).subject(subject);
                for address in to {
                    message = message.to(address.parse()?);
                }
                let message = message.multipart(MultiPart::alternative_plain_html(text, html))?;
                transport.send(message).await?;
                Ok(())
            }
            Mailer::Graph(graph) => graph.send(to, subject, html).await,
        }
    }
}

impl Graph {
    async fn send(&self, to: &[String], subject: &str, html: String) -> Result<()> {
        let token = self.token().await?;
        let response = self
            .client
            .post(format!(
                "{}/v1.0/users/{}/sendMail",
                self.graph_host,
                urlencode(&self.from)
            ))
            .bearer_auth(token)
            .json(&sendmail_body(to, subject, html))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            bail!(
                "graph sendMail: {status} {}",
                response.text().await.unwrap_or_default()
            );
        }
        Ok(())
    }

    /// an app-only token from the client credentials, reused until shortly before it expires
    async fn token(&self) -> Result<String> {
        if let Some((token, expires)) = self.token.lock().unwrap().as_ref()
            && Instant::now() < *expires
        {
            return Ok(token.clone());
        }
        let response = self
            .client
            .post(format!(
                "{}/{}/oauth2/v2.0/token",
                self.login_host,
                urlencode(&self.tenant)
            ))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("scope", &format!("{}/.default", self.graph_host)),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            bail!(
                "graph sign-in: {status} {}",
                response.text().await.unwrap_or_default()
            );
        }
        let token: Token = response.json().await?;
        let expires =
            Instant::now() + Duration::from_secs(token.expires_in).saturating_sub(TOKEN_MARGIN);
        *self.token.lock().unwrap() = Some((token.access_token.clone(), expires));
        Ok(token.access_token)
    }
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn sendmail_body(to: &[String], subject: &str, html: String) -> serde_json::Value {
    let recipients: Vec<_> = to
        .iter()
        .map(|address| json!({ "emailAddress": { "address": address } }))
        .collect();
    json!({
        "message": {
            "subject": subject,
            "body": { "contentType": "HTML", "content": html },
            "toRecipients": recipients,
        },
        "saveToSentItems": false,
    })
}

/// ALERT_EMAIL: addresses, comma separated, that get alerts for flows whose owner has none
pub fn shared_recipients() -> Result<Vec<String>> {
    let addresses: Vec<String> = optional_env("ALERT_EMAIL")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(bad) = addresses
        .iter()
        .find(|address| address.parse::<lettre::Address>().is_err())
    {
        bail!("ALERT_EMAIL: {bad:?} is not an email address");
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_gets_one_recipient_entry_per_address() {
        let body = sendmail_body(
            &["a@example.com".into(), "b@example.com".into()],
            "laplace: x is failed",
            "<p>x</p>".into(),
        );
        assert_eq!(
            body["message"]["toRecipients"][1]["emailAddress"]["address"],
            "b@example.com"
        );
        assert_eq!(body["message"]["body"]["contentType"], "HTML");
        assert_eq!(body["saveToSentItems"], false);
    }
}
