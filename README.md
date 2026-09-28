# laplace

Monitoring for the integrations around a data platform: files arriving over SFTP or in
object storage, scheduled scripts, Databricks jobs, Delta tables, and API traffic in both
directions. One Rust service with a web dashboard, built to run in Kubernetes.

It watches the edges a data warehouse depends on but does not see. Jobs report in, laplace
polls what cannot report, and the result is one list of flows that are ok, late or failed,
with an alert to whoever owns each one.

```
jobs, gateways -> laplace -> PostgreSQL (state) -> Delta (history)
                     |
                     +-> SFTP, storage, Delta, APIs, Databricks
```

## What it does

- **Watches seven kinds of flow.** Jobs that report in (`heartbeat`, `inbound` API
  traffic) and things laplace polls itself (`sftp`, `storage`, `delta`, `http`,
  `databricks`). A new job needs no configuration: its first report creates the flow.
- **Knows when something is due.** Cron schedules limited to Swedish and Finnish business
  days, with each country's public holidays and the days offices close in practice. A
  scheduled flow is late only when a deadline passed without a success after the one before.
- **Alerts once, to the right person.** Each flow has an owner with their own Teams or
  Slack webhook and email addresses. A broken step in a pipeline blocks the steps after it,
  so one fault gives one alert. Maintenance windows, silences and acknowledgements keep
  planned or known problems quiet.
- **Makes alerts actionable.** Teams and Slack cards carry buttons to open the failing run,
  acknowledge it or silence it for an hour. The button opens laplace with the action
  waiting for one more click, so a link preview can never act on its own.
- **Catches the quiet failures.** A run that moves far fewer rows than usual, a certificate
  about to expire, a changed SFTP host key, a Delta table left out of the latest load, or a
  schema that changed overnight all become warnings.
- **Scales out without coordination.** Any number of stateless replicas share one
  PostgreSQL database. Each check runs on exactly one replica, and each alert is sent once.
- **Reports uptime against targets.** Monthly uptime, incidents, longest outage and recovery
  time per flow and owner, with a day-by-day view, measured against each flow's SLA.

## Usage

```
docker compose up -d --build
```

The dashboard is then on http://localhost:8090, with demo flows, a demo SFTP server and a
demo mail server at http://localhost:8025. Flows are declared in `flows.toml`:

```toml
[[flow]]
id = "partner-statements"
kind = "sftp"
owner = "integrations"
host = "sftp.partner.example"
user = "company"
password_env = "SFTP_PARTNER_PASSWORD"
dir = "/out"
pattern = "statement_*.xml"
cron = "0 7 * * 1-5"
calendar = ["se"]
grace = "30m"
```

A Windows Task Scheduler job reports by being wrapped in `scripts/heartbeat.ps1`:

```
powershell.exe -NoProfile -File heartbeat.ps1 -Flow nightly-reconcile -Cron "0 2 * * *" -Command C:\jobs\reconcile.exe
```

The **connect** button in the dashboard writes these lines for each kind of job.

## Building

```
docker build -t laplace .
```

Releases are published to `ghcr.io/rasmsnall/laplace` by CI. The Helm chart in
`deploy/helm/laplace` installs the version named by its `appVersion`, and `deploy/postgres`
runs the database inside the cluster with CloudNativePG. See
[`docs/operations.md`](docs/operations.md), Chapter II.

## Development

```
bash scripts/check.sh
```

This runs every gate CI runs: a secret scan, `cargo fmt`, `cargo clippy -D warnings`,
`cargo test`, `cargo audit`, `npm audit` and the dashboard build, a check of `flows.toml`,
and validation of the Helm chart and the PostgreSQL manifests. Tools that are not
installed locally run in Docker. The Rust toolchain is pinned in `rust-toolchain.toml` so
the local run and CI agree.

`main` is protected: changes arrive through pull requests that pass `checks`. Dependabot
patch updates, and minor updates of packages at 1.0 or above, merge themselves once CI
passes.

## Documentation

Markdown in `docs/` is the single source of truth. Word copies are generated from it by
`python tools/md2docx.py` and are never edited by hand.

| Document | Contents |
|---|---|
| [`docs/architecture.md`](docs/architecture.md) | Flow model, checks, scheduling across replicas, alerting, data model, security model, assessment |
| [`docs/reference.md`](docs/reference.md) | `flows.toml`, environment variables, HTTP endpoints, roles, reporting from jobs |
| [`docs/operations.md`](docs/operations.md) | Deploying to Kubernetes, the database, sign-in and email, monitoring laplace itself, failure and recovery |
| [`SECURITY.md`](SECURITY.md) | Rules every change must follow, and the pre-merge checklist |

## Status

Released as 0.2.1. Every kind of flow, alerting to Teams, Slack and email, sign-in with roles,
reports and the Kubernetes deployment are implemented and tested end to end. The known
gaps are listed in `docs/operations.md`, Appendix B.
