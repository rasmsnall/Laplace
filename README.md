# laplace

monitors the integrations around us: files arriving on sftp or in object storage, scheduled
scripts, databricks jobs, and api traffic in both directions.

a flow is **ok**, **warning** (works, but needs attention soon or looked unusual), **late**
(a deadline passed without a success), **failed** (its last check found a problem),
**blocked** (waiting on a broken step before it in a pipeline) or **pending** (has not
reported yet).

| kind | how it is watched |
|---|---|
| `heartbeat` | the job calls `/ping/{flow}/start` and `/ping/{flow}/{exit code}`. task scheduler jobs use `scripts/heartbeat.ps1` |
| `databricks` | polls the jobs api for the latest runs of `job_id`, or pick jobs from the workspace in the connect panel |
| `sftp` | logs in, lists `dir`: new, empty and uncollected files. works for our servers and partners' |
| `storage` | same rules for `gs://`, `abfss://` or a local path, via `object_store` |
| `delta` | reads a delta table's log, or every table in a folder such as pgdelta's output: last commit, rows, files, bytes, schema changes, and tables left out of the latest load |
| `http` | probes an api we call: status, latency and certificate expiry |
| `inbound` | the gateway in front of an api partners call posts `{"status": 200, "millis": 35}` (or an array) to `/calls/{flow}`. alerts on silence and on the server error rate |

## when a flow is due

`every = "1d"` expects a success at least that often. for real schedules use cron:

```toml
cron = "0 7 * * 1-5"      # due by 07:00 on weekdays
calendar = ["se", "fi"]   # only on swedish and finnish business days: no weekends or public holidays
timezone = "Europe/Helsinki"   # LAPLACE_TIMEZONE when left out
grace = "30m"
```

a scheduled flow is late when the latest deadline passed and no success came after the one
before it. holidays follow the countries' rules, including easter, midsummer and all saints'
day, and the days offices close in practice (midsummer eve, christmas eve, swedish new year's
eve). databricks jobs picked from the workspace use the job's own schedule.

## warnings and anomalies

- **certificates**: an https api whose certificate expires within 14 days is a warning.
- **sftp host keys**: the first key seen is trusted. if it changes, laplace stops logging in
  (a changed key can mean someone is in the middle) until someone confirms the new key in
  the dashboard.
- **numbers per run**: a job can send numbers with its last ping, `{"rows": 1204331}`.
  laplace adds `seconds` for runs, `bytes` for files. a number under half or over double the
  median of the last 10 runs is a warning; `anomaly_tolerance` changes the band.

## delta tables

```toml
[[flow]]
id = "erp-tables"
kind = "delta"
url = "abfss://raw@account.dfs.core.windows.net/pg/"
after = ["erp-pgdelta-load"]
cron = "0 7 * * *"
```

laplace reads the transaction logs straight from storage, with no cluster. a new commit counts
as a success and reports `rows`, `files`, `bytes` and `tables`, so a load that suddenly writes far
fewer rows is an anomaly warning. a table whose last commit is more than six hours older than the
newest one was left out of the load, and a changed schema (columns added, dropped or retyped) is a
warning for a day. `poll` defaults to 15 minutes, since reading hundreds of logs is heavier than
listing a folder.

## pipelines

`after = ["erp-dump-adls"]` (or `?after=` from a job) says a flow waits for others. when a
step is late or failed, the steps after it are **blocked** and do not alert, so one broken
step gives one alert. the dashboard draws each pipeline.

## people

- `owner = "data-team"` routes a flow's alerts to that owner's webhook and addresses
  (`[owners.data-team]` with `webhook_env = "DATA_TEAM_WEBHOOK"` and `email = ["data@company.se"]`),
  or to `ALERT_WEBHOOK` and `ALERT_EMAIL`.
- **acknowledge** a problem with a note; it clears when the flow is ok again.
- **silence** a flow for a while; if it is still broken when the silence ends, the alert goes out.
- **maintenance windows** are planned downtime that comes back, such as a partner patching
  its sftp server every sunday night:

  ```toml
  [maintenance.partner-patching]
  cron = "0 2 * * 0"     # when it opens, in LAPLACE_TIMEZONE unless timezone is set
  lasts = "2h"
  flows = ["partner-*"]  # ids or patterns, which also match flows jobs registered
  ```

  a covered flow that breaks while the window is open shows **maintenance** and alerts nobody.
  if it is still broken when the window closes, the alert goes out; if it recovered, nothing
  is sent. time in maintenance does not count against uptime in reports.
- `sla = 0.995` holds a flow to an uptime target. **reports** shows uptime, incidents, the
  longest outage, recovery time and runs per month and owner.

## alerts

the format follows the webhook address:

- `hooks.slack.com` gets a block kit message.
- teams workflows (`*.logic.azure.com`, `*.powerplatform.com`) and the older `*.webhook.office.com`
  connectors get an adaptive card.
- anything else gets json: `text`, `flow`, `state`, `detail`, `owner`, `last_ok` and `link`.

with `LAPLACE_PUBLIC_URL` set, slack and teams cards get buttons: **open in laplace**, **acknowledge**
and **silence 1h**. the buttons open the failing run in laplace with the action waiting for one
more click, so a link preview or a forwarded message never acts, and the click goes through
sign-in, roles and the audit log like any other.

**email** has the same facts and links as the cards, sent from `LAPLACE_MAIL_FROM` through one of:

- **microsoft graph**, for microsoft 365 where smtp with a password is turned off. register an app
  in entra id, give it the `Mail.Send` application permission with admin consent, and set
  `LAPLACE_GRAPH_TENANT_ID`, `LAPLACE_GRAPH_CLIENT_ID` and `LAPLACE_GRAPH_CLIENT_SECRET` (a secret).
  `Mail.Send` alone lets the app send as any mailbox, so limit it to the laplace mailbox with
  exchange rbac for applications (or an application access policy). national clouds set
  `LAPLACE_GRAPH_LOGIN_HOST` and `LAPLACE_GRAPH_HOST`.
- **smtp**: `LAPLACE_SMTP_URL`, e.g. `smtp://user:password@smtp.company.se:587?tls=required`, kept in a secret.

an alert counts as delivered when the webhook or the email got through; a failed one is logged.

every flow, run and report month has its own link: `?flow=sftp-bank`, `?flow=sftp-bank&at=...`,
`?reports&month=2026-09`. use **copy link** on a flow or report, or click a run's time.

## design

```
gateways, scripts ──► laplace replicas (stateless) ──► postgres: current state, recent history
                            │                            │
                            └──► sftp, gcs, adls, apis   └──► delta in adls: full history
```

- **any number of replicas.** due checks live in the `jobs` table and are claimed with
  `for update skip locked`, so each runs on exactly one replica. alert state is in the
  database, so an alert is sent once. flows registered while running are picked up without a restart.
- **flows.toml reloads by itself.** each replica rereads it every tick; an edit that does not
  parse is refused, the running version stays, and editors see why at the top of the dashboard.
  in kubernetes a changed configmap reaches the pods within about a minute, without a rollout.
- **inbound traffic is counted per minute** in memory and upserted every 2 seconds, so the
  write rate does not grow with request volume.
- **postgres holds the current state and `RETENTION_DAYS` (30) of history**, and a year of
  state changes for reports. events, calls and state changes are appended to delta every 5
  minutes for databricks. a crash between the delta commit and the watermark update can
  append a batch twice; deduplicate on `id` and `(calls.flow, calls.minute)`.
- **independent of what it watches.** databricks and the lake can be down and laplace still
  reports it.

## run in kubernetes

`deploy/helm/laplace` is a helm chart. postgres is not part of it; use a managed one, e.g.
azure database for postgresql flexible server, with two roles: an owner for migrations and an
app role that can read and write tables but not change them (`deploy/test/postgres.yaml` shows
the grants).

```
kubectl create secret generic laplace --from-literal=DATABASE_URL=postgres://laplace_app:...@db/laplace ...
kubectl create secret generic laplace-migrations --from-literal=DATABASE_URL=postgres://laplace_owner:...@db/laplace
helm install laplace deploy/helm/laplace --set publicUrl=https://laplace.company.se   --set ingress.enabled=true --set ingress.host=laplace.company.se --set-file flows=flows.toml
```

- **migrations** run as a job before every install and upgrade (`laplace migrate`), with the
  owner's credentials; the pods run with `RUN_MIGRATIONS=false` and the app role.
- **two replicas** spread over nodes, a disruption budget, rolling updates without dropped
  requests. a changed `flows.toml` rolls the pods.
- **locked down**: non-root, read-only root filesystem, no capabilities, no kubernetes api token.
- **probes and metrics** on the internal port 9090, never exposed through the ingress:
  `/healthz` (the scheduler is ticking), `/readyz` (postgres answers), `/metrics` for prometheus
  (`metrics.serviceMonitor=true` adds a servicemonitor).
- **graceful shutdown**: on sigterm laplace finishes requests and writes buffered call counts.
- **a dead-man's switch**: set `LAPLACE_HEARTBEAT_URL` to a healthchecks.io (or similar) check.
  laplace calls it every minute while its scheduler and postgres work, so if laplace itself
  stops, that service tells you.

`laplace check flows.toml` validates a flows file without a database, for ci.

## run

```
docker compose up -d --build     # dashboard on http://localhost:8090
```

the compose file includes a demo sftp server and demo landing folders.

| variable | meaning |
|---|---|
| `DATABASE_URL` | postgres |
| `LAPLACE_CONFIG` | path to `flows.toml` |
| `LAPLACE_TOKEN` | a token jobs may send on `/ping` and `/calls`; tokens made in the settings page work too |
| `LAPLACE_PUBLIC_URL` | the address jobs and browsers use to reach laplace; the connect panel puts it in every snippet |
| `LAPLACE_TIMEZONE` | zone for schedules without their own and for report months, default `Europe/Stockholm`. can be changed in settings |
| `AUTO_REGISTER` | `false` stops jobs from creating flows by reporting in; default on. can be changed in settings |
| `LAPLACE_TRUST_PROXY` | `true` when a proxy in front sets `x-forwarded-for`, so rate limits see the real client |
| `ALERT_WEBHOOK` | slack, teams or any webhook, see [alerts](#alerts); owners can have their own |
| `ALERT_EMAIL` | addresses, comma separated, for flows whose owner has none |
| `LAPLACE_MAIL_FROM` | the mailbox email alerts are sent from |
| `LAPLACE_GRAPH_TENANT_ID`, `LAPLACE_GRAPH_CLIENT_ID`, `LAPLACE_GRAPH_CLIENT_SECRET` | send email through microsoft graph |
| `LAPLACE_SMTP_URL` | send email through an smtp server instead |
| `DELTA_URI` | where history is exported, e.g. `abfss://laplace@account.dfs.core.windows.net/history` |
| `RETENTION_DAYS` | history kept in postgres, default 30. can be changed in settings |
| `DATABRICKS_HOST`, `DATABRICKS_TOKEN` | for `databricks` flows and picking jobs from the workspace |
| `AZURE_STORAGE_*`, `GOOGLE_APPLICATION_CREDENTIALS` | for `storage` flows and the delta export |

## sign-in

sign-in is on when `LAPLACE_OIDC_ISSUER` is set. it works with any openid connect provider:
entra id, google, okta, keycloak.

| variable | meaning |
|---|---|
| `LAPLACE_OIDC_ISSUER` | e.g. `https://login.microsoftonline.com/{tenant}/v2.0` |
| `LAPLACE_OIDC_CLIENT_ID`, `LAPLACE_OIDC_CLIENT_SECRET` | from the app registration; its redirect uri is `{LAPLACE_PUBLIC_URL}/auth/callback` |
| `LAPLACE_OIDC_ALLOWED_DOMAINS` | optional, e.g. `company.se,company.fi` |
| `LAPLACE_SESSION_KEY` | 64 random bytes as base64, the same on every replica: `openssl rand -base64 64` |
| `LAPLACE_PUBLIC_URL` | required with sign-in |
| `LAPLACE_ADMINS` | emails that are always admins, e.g. `rasmus@company.se`; nobody can lock them out |

sessions last 12 hours in an encrypted cookie. `/ping`, `/calls`, `/connect/heartbeat.ps1` and
`/healthz` stay open for jobs; everything else needs a signed-in user with a role.

## access and settings

roles are global and each includes the ones before it:

| role | may |
|---|---|
| viewer | see flows, pipelines, the timeline and reports |
| operator | acknowledge, silence, trust a changed sftp host key |
| editor | connect jobs, pick databricks jobs |
| admin | the settings page |

a person gets the highest role from `LAPLACE_ADMINS`, their own entry, or any sso group mapped to a
role (entra id sends group object ids in the `groups` claim unless the app registration is set to
send names; above 200 groups entra sends none, so map the few groups that matter). the settings
page also holds job tokens (stored hashed, shown once), time zone, retention, auto-registration,
the owners and flows from `flows.toml` (read-only, they live in git) and an audit log of every
change and action.

without sign-in everyone is an admin, which only suits running laplace on your own machine.

see `SECURITY.md` for the rules changes must follow, and run `scripts/check.sh` before merging.

## beyond localhost

laplace listens on all interfaces and the dashboard uses relative paths, so it works on any host
name and behind a reverse proxy, including under a sub-path such as `https://tools.company.se/laplace/`.
before exposing it:

- put tls in front of it (ingress or proxy). tokens and flow data should not cross the network in plain http.
- turn on sign-in, or put it behind your sso proxy.
- databricks clusters and task scheduler servers need a network route to `LAPLACE_PUBLIC_URL`.

## connecting a job

press **connect** in the dashboard: name the flow, give its schedule, pick where the job runs,
copy the line.

jobs that report to us (task scheduler, databricks notebooks, cron, gateways) need no config:
the first `/ping/{flow}` or `/calls/{flow}` creates the flow. the query string says the rest:
`every=1h` or `cron=0%202%20*%20*%20*`, `calendar=se`, `grace=45m`, `after=other-flow`,
`owner=data-team`. sending different values later updates the flow. flows in `flows.toml`
always win over registered ones and cannot be changed this way.

things laplace polls itself (sftp, storage, http, databricks by job id) need credentials, so
they stay in `flows.toml`; the connect panel writes the block. databricks jobs can also be
picked from the workspace with one click.

task scheduler wraps the job with `scripts/heartbeat.ps1`, which laplace also serves at
`/connect/heartbeat.ps1`:

```
powershell.exe -NoProfile -File heartbeat.ps1 -Flow nightly-reconcile -Cron "0 2 * * *" -Calendar se -Command C:\jobs\reconcile.exe
```

set `LAPLACE_URL` (and `LAPLACE_TOKEN`) for the account the task runs as. the job still runs
when laplace is unreachable.
