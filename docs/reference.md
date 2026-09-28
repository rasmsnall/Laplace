# laplace: Reference

**Document type** Configuration and interface reference
**Status** Complete. Describes version 0.2.0 as built.
**Audience** Whoever declares flows, connects jobs, or deploys laplace.
**Companion documents** `architecture.md` for the design, `operations.md` for running it.
**Version** 1.0
**Date** 2026-09-28

---

## Contents

- I. Introduction
  - 1. Purpose
  - 2. Conventions
- II. The Flows File
  - 1. Fields common to every flow
  - 2. Fields by kind
  - 3. Owners
  - 4. Maintenance windows
  - 5. Validation
- III. Reporting from Jobs
  - 1. Heartbeats
  - 2. Inbound API traffic
  - 3. Registering a flow by reporting
  - 4. Tokens
- IV. Environment
  - 1. Core settings
  - 2. Sign-in
  - 3. Alert delivery
  - 4. Integrations
- V. HTTP Endpoints
  - 1. Open endpoints
  - 2. Dashboard API
  - 3. Operations port
- VI. Roles
- References

### List of Tables

- `<Table 2-1>` Fields common to every flow
- `<Table 2-2>` Fields by kind
- `<Table 2-3>` Maintenance window fields
- `<Table 3-1>` Heartbeat requests
- `<Table 3-2>` Query parameters a job may send
- `<Table 4-1>` Core settings
- `<Table 4-2>` Sign-in settings
- `<Table 4-3>` Alert delivery settings
- `<Table 4-4>` Integration settings
- `<Table 5-1>` Open endpoints and their limits
- `<Table 5-2>` Dashboard API
- `<Table 6-1>` Roles

---

## I. Introduction

### 1. Purpose

This document lists every setting laplace reads and every endpoint it answers. It is meant
to be looked up, not read through. The reasons behind the design are in `architecture.md`;
how to deploy and recover it is in `operations.md`.

### 2. Conventions

Durations are written as `30s`, `15m`, `1h`, `1d` or combinations such as `1h30m`. Cron
expressions have five fields in the Unix style (`0 7 * * 1-5`) unless a flow sets
`cron_style = "quartz"`, which is the seconds-first form Databricks stores. Flow ids are
1 to 64 characters of lowercase letters, digits, `-`, `_` and `.`.

## II. The Flows File

Flows are declared in `flows.toml`, kept in git and reviewed like code. The path is read
from `LAPLACE_CONFIG`. Each replica rereads the file every two seconds and applies a valid
change without a restart (see `architecture.md`, Chapter IV, Section 4).

### 1. Fields common to every flow

`<Table 2-1>` Fields common to every flow

| Field | Default | Meaning |
|---|---|---|
| `id` | required | Unique name of the flow |
| `kind` | required | One of the kinds in Section 2 |
| `every` | `1d` | A success is expected at least this often. Ignored for lateness when `cron` is set |
| `cron` | none | When the flow is due, e.g. `0 7 * * 1-5` for 07:00 on weekdays |
| `cron_style` | `unix` | `quartz` for seconds-first expressions |
| `calendar` | none | `["se"]`, `["fi"]` or both: only business days in those countries count |
| `timezone` | `LAPLACE_TIMEZONE` | The zone `cron` is read in |
| `grace` | `0` | How long after a deadline a success may still arrive |
| `source` | derived | Shown in the dashboard's source column |
| `after` | none | Flows that must succeed before this one can, making a pipeline |
| `owner` | none | An owner from `[owners]`, whose webhook and addresses get the alerts |
| `sla` | none | Uptime target for reports, e.g. `0.995` |
| `anomaly_tolerance` | `0.5` | How far a run's numbers may stray from the median of the last ten: `0.5` accepts half to double. Above 0 and below 1 |

### 2. Fields by kind

`<Table 2-2>` Fields by kind

| Kind | Fields | What counts as a success |
|---|---|---|
| `heartbeat` | `max_runtime` (optional) | A `/ping` with exit code 0. A run still going after `max_runtime` fails |
| `inbound` | `max_error_rate` (default `0.05`) | Calls arriving. Fails above the server error rate in the last hour, once at least 10 calls arrived |
| `http` | `url`, `timeout` (`10s`), `max_latency` | A 2xx answer within the limits. A certificate expiring within 14 days is a warning |
| `sftp` | `host`, `port` (`22`), `user`, `password_env` or `key_file`, `host_key`, `dir`, `pattern` (`*`), `pickup`, `poll` (`5m`) | A new, non-empty matching file. A file older than `pickup` has not been collected |
| `storage` | `url` (`gs://`, `abfss://` or a local path), `pattern`, `pickup`, `poll` (`5m`) | As for `sftp` |
| `delta` | `url` (one table or a folder of tables), `poll` (`15m`) | A new commit. Reports `rows`, `files`, `bytes` and `tables` |
| `databricks` | `job_id`, `poll` (`5m`) | A successful run of the job |

For `sftp`, `host_key` pins the server key as printed by `ssh-keygen -lf` (`SHA256:...`).
Without it, the first key seen is trusted and a later change stops the check until an
operator confirms it.

### 3. Owners

```toml
[owners.data-team]
webhook_env = "DATA_TEAM_WEBHOOK"
email = ["data@company.se"]
```

`webhook_env` names the environment variable that holds the owner's Teams or Slack webhook,
so the address itself stays out of git. A flow whose owner has no webhook or no addresses
falls back to `ALERT_WEBHOOK` and `ALERT_EMAIL`.

### 4. Maintenance windows

```toml
[maintenance.partner-patching]
cron = "0 2 * * 0"
lasts = "2h"
flows = ["partner-*"]
```

`<Table 2-3>` Maintenance window fields

| Field | Default | Meaning |
|---|---|---|
| `cron` | required | When the window opens |
| `cron_style` | `unix` | As for flows |
| `timezone` | `LAPLACE_TIMEZONE` | The zone `cron` is read in |
| `lasts` | required | How long the window stays open |
| `flows` | required | Flow ids or glob patterns. Patterns also match flows that jobs registered |

### 5. Validation

`laplace check flows.toml` validates a file without a database and prints how many flows,
owners and maintenance windows it declares. CI runs it on every change. It rejects duplicate
ids, unknown owners, `after` entries that name no flow or form a cycle, schedules that do
not parse, SFTP flows without credentials, and email addresses that are not valid.

## III. Reporting from Jobs

### 1. Heartbeats

`<Table 3-1>` Heartbeat requests

| Request | Meaning |
|---|---|
| `/ping/{flow}` | The job succeeded |
| `/ping/{flow}/start` | The job started. Its duration is measured from here |
| `/ping/{flow}/{exit code}` | The job ended. `0` is a success, anything else a failure |

Any method works, so `curl` and `Invoke-WebRequest` need no flags. The exit code is read as a
number, so `0`, `00` and `+0` are all a success. A JSON body of up to 20
numbers, such as `{"rows": 1204331}`, is recorded with the run and compared with earlier runs.

`scripts/heartbeat.ps1`, also served at `/connect/heartbeat.ps1`, wraps a Task Scheduler job,
reports its start and exit code, and runs the job even when laplace cannot be reached.

### 2. Inbound API traffic

A gateway in front of an API that partners call posts each call, or an array of up to 10,000
calls, to `/calls/{flow}`:

```json
{"status": 200, "millis": 35}
```

Calls are counted per minute, not stored one by one (see `architecture.md`, Chapter IV,
Section 3). Bodies are limited to 1 MB. `millis` below 0 counts as 0, and above one day as one
day, so a gateway reporting nonsense cannot disturb the counts.

### 3. Registering a flow by reporting

When auto-registration is on, the first report under a new name creates the flow. The query
string may describe it:

`<Table 3-2>` Query parameters a job may send

| Parameter | Example | Meaning |
|---|---|---|
| `every` | `every=1h` | As in `flows.toml` |
| `cron` | `cron=0%202%20*%20*%20*` | As in `flows.toml`, URL-encoded |
| `calendar` | `calendar=se` | Comma separated |
| `timezone` | `timezone=Europe/Helsinki` | As in `flows.toml` |
| `grace` | `grace=45m` | As in `flows.toml` |
| `after` | `after=erp-dump-adls` | Comma separated |
| `owner` | `owner=data-team` | An owner from `[owners]`. One that is not there is ignored and logged, so the report still counts |

Sending different values later updates the flow. A flow declared in `flows.toml` always wins
and cannot be changed this way.

### 4. Tokens

Jobs send a token as `Authorization: Bearer <token>`. Tokens are created on the settings page
or given in `LAPLACE_TOKEN`. Until the first token exists, reports need none; from then on,
every report does, even after every token is revoked.

## IV. Environment

### 1. Core settings

`<Table 4-1>` Core settings

| Variable | Default | Meaning |
|---|---|---|
| `DATABASE_URL` | none | PostgreSQL connection. Alternatively `PGHOST`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`, `PGSSLMODE` |
| `LAPLACE_CONFIG` | `flows.toml` | Path to the flows file |
| `LAPLACE_PUBLIC_URL` | none | The address browsers and jobs use. Needed for sign-in and for links in alerts |
| `LAPLACE_TIMEZONE` | `Europe/Stockholm` | Zone for schedules without their own and for report months. Changeable in settings |
| `RETENTION_DAYS` | `30` | Days of run history kept in PostgreSQL. Changeable in settings |
| `AUTO_REGISTER` | `true` | Whether reports under a new name create a flow. Changeable in settings |
| `LAPLACE_TOKEN` | none | A token jobs may send, in addition to those made in settings |
| `LAPLACE_TRUST_PROXY` | `false` | Trust the last `X-Forwarded-For` address, which the proxy in front appends, so rate limits see the real client |
| `RUN_MIGRATIONS` | `true` | Whether `serve` migrates the schema at start. The Helm chart sets `false` |
| `PORT`, `LAPLACE_OPS_PORT` | `8090`, `9090` | The public port, and the internal port for probes and metrics |
| `LAPLACE_HEARTBEAT_URL` | none | An outside check laplace calls every minute while healthy |

### 2. Sign-in

Sign-in is on when `LAPLACE_OIDC_ISSUER` is set. Any OpenID Connect provider works: Entra
ID, Google, Okta or Keycloak.

`<Table 4-2>` Sign-in settings

| Variable | Meaning |
|---|---|
| `LAPLACE_OIDC_ISSUER` | e.g. `https://login.microsoftonline.com/{tenant}/v2.0` |
| `LAPLACE_OIDC_CLIENT_ID`, `LAPLACE_OIDC_CLIENT_SECRET` | From the app registration. Its redirect URI is `{LAPLACE_PUBLIC_URL}/auth/callback` |
| `LAPLACE_OIDC_ALLOWED_DOMAINS` | Optional, e.g. `company.se,company.fi` |
| `LAPLACE_SESSION_KEY` | 64 random bytes as base64, the same on every replica: `openssl rand -base64 64` |
| `LAPLACE_ADMINS` | Addresses that are always admins, so nobody can lock everyone out |

### 3. Alert delivery

`<Table 4-3>` Alert delivery settings

| Variable | Meaning |
|---|---|
| `ALERT_WEBHOOK` | Teams, Slack or any webhook, for flows whose owner has none |
| `ALERT_EMAIL` | Addresses, comma separated, for flows whose owner has none |
| `LAPLACE_MAIL_FROM` | The mailbox email alerts are sent from |
| `LAPLACE_GRAPH_TENANT_ID`, `LAPLACE_GRAPH_CLIENT_ID`, `LAPLACE_GRAPH_CLIENT_SECRET` | Send through Microsoft Graph. Takes precedence over SMTP |
| `LAPLACE_GRAPH_LOGIN_HOST`, `LAPLACE_GRAPH_HOST` | Only for national clouds |
| `LAPLACE_SMTP_URL` | Send through SMTP, e.g. `smtp://user:password@smtp.company.se:587?tls=required` |

### 4. Integrations

`<Table 4-4>` Integration settings

| Variable | Meaning |
|---|---|
| `DATABRICKS_HOST`, `DATABRICKS_TOKEN` | For `databricks` flows and for picking jobs from the workspace |
| `DELTA_URI` | Where run history is exported, e.g. `abfss://laplace@account.dfs.core.windows.net/history` |
| `AZURE_STORAGE_*`, `GOOGLE_APPLICATION_CREDENTIALS` | For `storage` and `delta` flows and the export |
| Any name in `password_env` or `webhook_env` | The secret that field refers to |

## V. HTTP Endpoints

### 1. Open endpoints

These answer without a signed-in user. Rate limits are per client address and per minute.

`<Table 5-1>` Open endpoints and their limits

| Endpoint | Purpose | Limit |
|---|---|---|
| `/ping/...`, `/calls/{flow}` | Job reports | 20 wrong tokens, then refused for the rest of the minute. Correct reports are not limited |
| `/auth/login`, `/auth/callback`, `/auth/logout` | Sign-in | 30 |
| `/connect/heartbeat.ps1`, `/healthz` | The heartbeat script, a liveness answer | 600 |

### 2. Dashboard API

Every other path needs a signed-in user, and every handler checks the caller's role itself.
Writes from another site are refused.

`<Table 5-2>` Dashboard API

| Endpoint | Role | Purpose |
|---|---|---|
| `GET /api/me` | viewer | Who is signed in, their role, and any `flows.toml` error for editors |
| `GET /api/flows` | viewer | Every flow with its state |
| `GET /api/flows/{flow}/history` | viewer | Recent runs |
| `GET /api/timeline?hours=` | viewer | Runs for the timeline, up to seven days |
| `GET /api/reports?month=` | viewer | Uptime and incidents per flow, with uptime per day |
| `POST`, `DELETE /api/flows/{flow}/acknowledge` | operator | Acknowledge a problem with a note, or clear it |
| `POST`, `DELETE /api/flows/{flow}/silence` | operator | Silence alerts for some hours, or lift it |
| `POST /api/flows/{flow}/trust-host-key` | operator | Accept a changed SFTP host key |
| `GET /api/connect` | editor | Values for the connect panel |
| `GET /api/databricks/jobs` | editor | Jobs in the workspace |
| `POST /api/databricks/jobs/{job_id}/monitor` | editor | Start monitoring a job |
| `GET /api/settings`, `PUT /api/settings/general` | admin | Settings |
| `POST`, `DELETE /api/settings/members`, `/groups` | admin | Grant or remove access |
| `POST`, `DELETE /api/settings/tokens` | admin | Create or revoke job tokens |
| `GET /api/settings/audit` | admin | The last 200 changes and actions |

### 3. Operations port

On `LAPLACE_OPS_PORT`, never exposed through the ingress: `/healthz` fails when the scheduler
has not ticked for 60 seconds, `/readyz` fails when PostgreSQL does not answer within two
seconds, and `/metrics` serves Prometheus metrics: `laplace_flow_state`,
`laplace_flow_last_ok_age_seconds`, `laplace_flows` and `laplace_scheduler_lag_seconds`.

## VI. Roles

Roles are global, and each includes the ones before it. A person gets the highest role from
`LAPLACE_ADMINS`, their own entry, or any single sign-on group mapped to a role.

`<Table 6-1>` Roles

| Role | May |
|---|---|
| viewer | See flows, pipelines, the timeline and reports |
| operator | Also acknowledge, silence, and trust a changed host key |
| editor | Also connect jobs and pick Databricks jobs |
| admin | Also change access, tokens and settings |

Entra ID sends group object ids in the `groups` claim unless the app registration is set to
send names, and sends no groups at all above 200, so map the few groups that matter. Without
sign-in, everyone is an admin, which only suits a single workstation.

## References

1. Microsoft. *Microsoft identity platform and OpenID Connect protocol*.
   https://learn.microsoft.com/en-us/entra/identity-platform/v2-protocols-oidc
2. Microsoft. *user: sendMail*, Microsoft Graph v1.0.
   https://learn.microsoft.com/en-us/graph/api/user-sendmail
3. Slack. *Block Kit*. https://api.slack.com/block-kit
4. Microsoft. *Adaptive Cards*. https://adaptivecards.io/
5. Databricks. *Jobs API 2.1*. https://docs.databricks.com/api/workspace/jobs
6. Delta Lake Project. *Delta Transaction Log Protocol*.
   https://github.com/delta-io/delta/blob/master/PROTOCOL.md
7. Prometheus. *Exposition formats*.
   https://prometheus.io/docs/instrumenting/exposition_formats/
