use anyhow::{Result, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use url::form_urlencoded;

use crate::App;
use crate::db;
use crate::status::{State, Status};

/// records every state change for reports, and alerts on a move into or out of warning/late/failed, once, whichever replica runs the job.
/// a flow whose alert could not be delivered, or that is silenced, keeps its old alerted state,
/// so the alert goes out on a later run if the flow is still in that state.
pub async fn run(app: &App) -> Result<()> {
    db::end_expired_silences(&app.pool).await?;
    let zone = app.settings().timezone;
    for flow in app.statuses().await? {
        if flow.state == State::Ok && flow.acknowledged.is_some() {
            db::clear_acknowledgement(&app.pool, &flow.id).await?;
        }
        if flow.recorded_state.as_deref() != Some(flow.state.as_str()) {
            db::record_state(&app.pool, &flow.id, flow.state.as_str(), &flow.detail).await?;
        }

        // maintenance leaves the alerted state alone, so what the flow is in afterwards
        // is compared with what was last said about it
        let alerted = flow.alerted_state.as_str();
        if alerted == flow.state.as_str()
            || flow.state == State::Maintenance
            || flow.silenced_until.is_some_and(|until| until > Utc::now())
        {
            continue;
        }

        let needed_attention = State::parse(alerted).is_some_and(State::needs_attention);
        if flow.state.needs_attention() || (needed_attention && flow.state == State::Ok) {
            let failed_at = match flow.state {
                State::Failed => db::last_failure(&app.pool, &flow.id).await?,
                _ => None,
            };
            let alert = Alert {
                flow: &flow,
                links: app
                    .public_url
                    .as_deref()
                    .map(|base| Links::new(base, &flow.id, failed_at)),
                last_ok: flow.last_ok.map_or("never".into(), |at| {
                    at.with_timezone(&zone)
                        .format("%Y-%m-%d %H:%M %Z")
                        .to_string()
                }),
            };
            println!("{}", alert.text());
            if let Err(error) = notify(app, &alert).await {
                eprintln!("alert for {} not delivered: {error:#}", flow.id);
                continue;
            }
        }
        db::set_alerted(&app.pool, &flow.id, flow.state.as_str()).await?;
    }
    Ok(())
}

/// the owner's own webhook and addresses, each falling back to the shared ones
fn recipients(app: &App, owner: Option<&str>) -> (Option<String>, Vec<String>) {
    let config = app.config();
    let owner = owner.and_then(|owner| config.owners.get(owner));
    let webhook = owner
        .and_then(|owner| owner.webhook_env.as_deref())
        .and_then(crate::optional_env)
        .or_else(|| app.alert_webhook.clone());
    let email = owner
        .map(|owner| owner.email.clone())
        .filter(|email| !email.is_empty())
        .unwrap_or_else(|| app.alert_email.clone());
    (webhook, email)
}

/// delivered when at least one way got through, so a broken channel does not repeat
/// the alert on the others every minute; the failures are logged
async fn notify(app: &App, alert: &Alert<'_>) -> Result<()> {
    let (webhook, email) = recipients(app, alert.flow.owner.as_deref());
    let mut delivered = false;
    let mut failures = Vec::new();

    if let Some(webhook) = webhook {
        let sent = app
            .client
            .post(&webhook)
            .json(&alert.payload(Format::of(&webhook)))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        match sent {
            Ok(_) => delivered = true,
            Err(error) => failures.push(format!("webhook: {error:#}")),
        }
    }
    if let Some(mailer) = &app.mailer
        && !email.is_empty()
    {
        let (subject, text, html) = alert.email();
        match mailer.send(&email, &subject, text, html).await {
            Ok(()) => delivered = true,
            Err(error) => failures.push(format!("email: {error:#}")),
        }
    }

    if failures.is_empty() {
        return Ok(());
    }
    if delivered {
        eprintln!(
            "alert for {} partly delivered: {}",
            alert.flow.id,
            failures.join("; ")
        );
        return Ok(());
    }
    bail!(failures.join("; "))
}

/// the buttons only open laplace with the action ready; the click there is what acts,
/// so a link preview or a forwarded message can never acknowledge anything
struct Links {
    open: String,
    acknowledge: String,
    silence: String,
}

impl Links {
    fn new(base: &str, flow: &str, failed_at: Option<DateTime<Utc>>) -> Self {
        let link = |action: Option<&str>| {
            let mut query = form_urlencoded::Serializer::new(String::new());
            query.append_pair("flow", flow);
            if let Some(at) = failed_at {
                query.append_pair("at", &at.to_rfc3339_opts(SecondsFormat::Micros, true));
            }
            if let Some(action) = action {
                query.append_pair("do", action);
            }
            format!("{base}/?{}", query.finish())
        };
        Links {
            open: link(None),
            acknowledge: link(Some("acknowledge")),
            silence: link(Some("silence")),
        }
    }

    /// a recovered flow has nothing left to acknowledge
    fn buttons(&self, state: State) -> Vec<(&'static str, &str)> {
        let mut buttons = vec![("open in laplace", self.open.as_str())];
        if state.needs_attention() {
            buttons.push(("acknowledge", &self.acknowledge));
            buttons.push(("silence 1h", &self.silence));
        }
        buttons
    }
}

#[derive(Debug, PartialEq)]
enum Format {
    Slack,
    Teams,
    Plain,
}

impl Format {
    fn of(webhook: &str) -> Self {
        let host = url::Url::parse(webhook)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        if host == "hooks.slack.com" {
            Format::Slack
        } else if [
            ".webhook.office.com",
            ".logic.azure.com",
            ".powerplatform.com",
        ]
        .iter()
        .any(|suffix| host.ends_with(suffix))
        {
            Format::Teams
        } else {
            Format::Plain
        }
    }
}

struct Alert<'a> {
    flow: &'a Status,
    links: Option<Links>,
    last_ok: String,
}

impl Alert<'_> {
    fn text(&self) -> String {
        let flow = self.flow;
        let owner = flow
            .owner
            .as_deref()
            .map(|owner| format!(", owner {owner}"))
            .unwrap_or_default();
        format!(
            "laplace: {} is {} ({}){owner}",
            flow.id,
            flow.state.as_str(),
            flow.detail
        )
    }

    fn buttons(&self) -> Vec<(&'static str, &str)> {
        self.links
            .as_ref()
            .map(|links| links.buttons(self.flow.state))
            .unwrap_or_default()
    }

    fn payload(&self, format: Format) -> Value {
        match format {
            Format::Slack => self.slack(),
            Format::Teams => self.teams(),
            Format::Plain => self.plain(),
        }
    }

    fn slack(&self) -> Value {
        let flow = self.flow;
        let escape = |text: &str| {
            text.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let mut blocks = vec![
            json!({
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": format!("*{}* is *{}*\n{}", escape(&flow.id), flow.state.as_str(), escape(&flow.detail)),
                },
            }),
            json!({
                "type": "context",
                "elements": [{
                    "type": "mrkdwn",
                    "text": escape(&format!(
                        "owner {} · last ok {} · {}",
                        flow.owner.as_deref().unwrap_or("none"),
                        self.last_ok,
                        flow.kind
                    )),
                }],
            }),
        ];
        let buttons = self.buttons();
        if !buttons.is_empty() {
            let elements: Vec<Value> = buttons
                .into_iter()
                .map(|(label, url)| {
                    json!({ "type": "button", "text": { "type": "plain_text", "text": label }, "url": url })
                })
                .collect();
            blocks.push(json!({ "type": "actions", "elements": elements }));
        }
        json!({ "text": self.text(), "blocks": blocks })
    }

    /// an adaptive card, which both teams workflows and the older connectors accept
    fn teams(&self) -> Value {
        let flow = self.flow;
        let color = match flow.state {
            State::Failed => "Attention",
            State::Ok => "Good",
            _ => "Warning",
        };
        let actions: Vec<Value> = self
            .buttons()
            .into_iter()
            .map(|(label, url)| json!({ "type": "Action.OpenUrl", "title": label, "url": url }))
            .collect();
        let card = json!({
            "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
            "type": "AdaptiveCard",
            "version": "1.4",
            "body": [
                {
                    "type": "TextBlock",
                    "text": format!("{} is {}", flow.id, flow.state.as_str()),
                    "weight": "Bolder",
                    "size": "Medium",
                    "color": color,
                    "wrap": true,
                },
                { "type": "TextBlock", "text": flow.detail, "wrap": true },
                {
                    "type": "FactSet",
                    "facts": [
                        { "title": "owner", "value": flow.owner.as_deref().unwrap_or("none") },
                        { "title": "last ok", "value": self.last_ok },
                        { "title": "type", "value": flow.kind },
                        { "title": "source", "value": flow.source },
                    ],
                },
            ],
            "actions": actions,
        });
        json!({
            "type": "message",
            "attachments": [{ "contentType": "application/vnd.microsoft.card.adaptive", "content": card }],
        })
    }

    fn email(&self) -> (String, String, String) {
        let flow = self.flow;
        let subject = format!("laplace: {} is {}", flow.id, flow.state.as_str());
        let facts = [
            ("owner", flow.owner.as_deref().unwrap_or("none")),
            ("last ok", self.last_ok.as_str()),
            ("type", flow.kind),
            ("source", flow.source.as_str()),
        ];
        let buttons = self.buttons();

        let mut text = format!("{}\n\n", self.text());
        for (label, value) in facts {
            text.push_str(&format!("{label}: {value}\n"));
        }
        for (label, url) in &buttons {
            text.push_str(&format!("\n{label}: {url}"));
        }

        let rows: String = facts
            .iter()
            .map(|(label, value)| {
                format!(
                    "<tr><td style=\"color:#6b6b6b;padding:2px 16px 2px 0\">{label}</td><td>{}</td></tr>",
                    escape_html(value)
                )
            })
            .collect();
        let links: String = buttons
            .iter()
            .map(|(label, url)| {
                format!(
                    "<a href=\"{}\" style=\"display:inline-block;margin-right:8px;padding:6px 12px;border:1px solid #b8912f;color:#111;text-decoration:none\">{label}</a>",
                    escape_html(url)
                )
            })
            .collect();
        let html = format!(
            "<div style=\"font-family:Arial,sans-serif;font-size:14px;color:#111\">\
             <p style=\"font-size:16px;margin:0 0 4px\"><b>{}</b> is <b>{}</b></p>\
             <p style=\"margin:0 0 12px\">{}</p>\
             <table style=\"border-collapse:collapse;margin-bottom:16px\">{rows}</table>\
             <p>{links}</p></div>",
            escape_html(&flow.id),
            flow.state.as_str(),
            escape_html(&flow.detail),
        );
        (subject, text, html)
    }

    /// keeps `text` so any receiver that only shows text still works
    fn plain(&self) -> Value {
        let flow = self.flow;
        json!({
            "text": self.text(),
            "flow": flow.id,
            "state": flow.state.as_str(),
            "detail": flow.detail,
            "owner": flow.owner,
            "last_ok": flow.last_ok,
            "link": self.links.as_ref().map(|links| &links.open),
        })
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: State) -> Status {
        Status {
            id: "partner <statements>".into(),
            kind: "sftp",
            source: "sftp://bank@host/out".into(),
            link: None,
            every: "1d".into(),
            after: vec![],
            owner: Some("finance".into()),
            acknowledged: None,
            silenced_until: None,
            silence_note: None,
            state,
            detail: "no file & no reply".into(),
            last_ok: None,
            host_key_pending: false,
            cost: None,
            alerted_state: "ok".into(),
            recorded_state: None,
        }
    }

    #[test]
    fn picks_the_format_from_the_webhook_host() {
        assert_eq!(
            Format::of("https://hooks.slack.com/services/a/b/c"),
            Format::Slack
        );
        assert_eq!(
            Format::of("https://contoso.webhook.office.com/webhookb2/x"),
            Format::Teams
        );
        assert_eq!(
            Format::of("https://prod-12.westeurope.logic.azure.com:443/workflows/x"),
            Format::Teams
        );
        assert_eq!(
            Format::of("https://hooks.slack.com.evil.example/x"),
            Format::Plain
        );
        assert_eq!(Format::of("not a url"), Format::Plain);
    }

    #[test]
    fn links_carry_the_flow_run_and_action() {
        let at = "2026-09-26T07:00:00Z".parse().unwrap();
        let links = Links::new("https://laplace.example", "a b&c", Some(at));
        assert_eq!(
            links.acknowledge,
            "https://laplace.example/?flow=a+b%26c&at=2026-09-26T07%3A00%3A00.000000Z&do=acknowledge"
        );
        assert!(!links.open.contains("do="));
    }

    #[test]
    fn a_recovered_flow_only_gets_the_open_button() {
        let links = Links::new("https://laplace.example", "x", None);
        assert_eq!(links.buttons(State::Failed).len(), 3);
        assert_eq!(links.buttons(State::Ok).len(), 1);
    }

    #[test]
    fn slack_text_is_escaped() {
        let flow = status(State::Failed);
        let alert = Alert {
            flow: &flow,
            links: None,
            last_ok: "never".into(),
        };
        let payload = alert.payload(Format::Slack);
        let section = payload["blocks"][0]["text"]["text"].as_str().unwrap();
        assert!(section.contains("partner &lt;statements&gt;"));
        assert!(section.contains("no file &amp; no reply"));
        assert_eq!(payload["blocks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn email_is_escaped_and_carries_the_links() {
        let flow = status(State::Failed);
        let alert = Alert {
            flow: &flow,
            links: Some(Links::new("https://laplace.example", "x", None)),
            last_ok: "never".into(),
        };
        let (subject, text, html) = alert.email();
        assert_eq!(subject, "laplace: partner <statements> is failed");
        assert!(text.contains("acknowledge: https://laplace.example/?flow=x&do=acknowledge"));
        assert!(html.contains("partner &lt;statements&gt;"));
        assert!(html.contains("?flow=x&amp;do=silence"));
    }

    #[test]
    fn teams_gets_an_adaptive_card_with_buttons() {
        let flow = status(State::Late);
        let alert = Alert {
            flow: &flow,
            links: Some(Links::new("https://laplace.example", "x", None)),
            last_ok: "never".into(),
        };
        let payload = alert.payload(Format::Teams);
        let card = &payload["attachments"][0]["content"];
        assert_eq!(card["type"], "AdaptiveCard");
        assert_eq!(card["actions"][2]["title"], "silence 1h");
    }
}
