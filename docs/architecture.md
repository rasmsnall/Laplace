# laplace: Architecture

**Document type** Technical architecture specification
**Status** Complete and implemented. Describes version 0.2.1 as built.
**Audience** Anyone operating, extending or reviewing laplace. No prior context assumed.
**Companion documents** `reference.md` for settings and endpoints, `operations.md` for running it.
**Version** 1.0
**Date** 2026-09-28

---

## Contents

- I. Introduction
  - 1. Purpose
  - 2. Rationale
  - 3. Scope and non-goals
- II. Flow Model
  - 1. States
  - 2. Pushed and polled flows
  - 3. When a flow is due
  - 4. Pipelines
  - 5. Warnings and anomalies
- III. Checks
  - 1. The shape of a check
  - 2. SFTP and object storage
  - 3. Delta tables
  - 4. HTTP and Databricks
- IV. Concurrency and Scheduling
  - 1. Stateless replicas
  - 2. The jobs table
  - 3. Inbound traffic
  - 4. Configuration reload
  - 5. Invariants
- V. Alerting
  - 1. From state to alert
  - 2. What keeps an alert quiet
  - 3. Delivery
  - 4. Acting from an alert
- VI. Data Model and Retention
  - 1. Tables
  - 2. Retention
  - 3. Export to Delta
  - 4. Reports
- VII. Security Model
- VIII. Assessment
  - 1. Advantages
  - 2. Disadvantages
  - 3. Known ceiling
- References
- Appendix A. Alert decisions

### List of Tables

- `<Table 2-1>` Flow states
- `<Table 2-2>` Warnings
- `<Table 4-1>` Scheduled jobs
- `<Table 5-1>` Alert formats by webhook host
- `<Table 6-1>` PostgreSQL tables
- `<Table A-1>` Alert decisions

### List of Figures

- `[Figure 1-1]` Where laplace sits
- `[Figure 4-1]` Replicas sharing one database

---

## I. Introduction

### 1. Purpose

laplace watches the integrations a data platform depends on but cannot see from the inside:
partner files over SFTP, landings in object storage, scheduled scripts on Windows servers,
Databricks jobs, the Delta tables those jobs write, and API traffic in both directions. It
turns them into one list of flows, each ok, late or failed, and alerts whoever owns a flow
when it breaks.

```
partners, scripts, gateways --report--> laplace --state--> PostgreSQL --history--> Delta
                                           |
                                           +--polls--> SFTP, storage, Delta, APIs, Databricks
```

[Figure 1-1] Where laplace sits

### 2. Rationale

The failures that cost the most are silent: a file that never arrives, a job that stops
running, a load that writes a tenth of its usual rows. None of these raise an error anywhere;
something has to notice an absence. laplace does that by knowing, for each flow, when
something should have happened, and comparing that with what did.

It is deliberately separate from what it watches. Databricks, the lake and the partners can
all be down, and laplace still reports it, because it depends on none of them to run.

### 3. Scope and non-goals

In scope: whether each integration ran on time and correctly, who is told when it did not,
and how reliable each one was over a month.

Out of scope:

- **Log collection and tracing.** laplace records runs and their numbers, not their output.
- **Retrying or repairing failed work.** It reports; the job's own scheduler reruns.
- **Metrics of the infrastructure itself.** CPU, memory and disks belong to the platform's
  own monitoring. laplace exposes its own health for that monitoring to scrape.

## II. Flow Model

### 1. States

Every flow is in exactly one state, computed on each read from what was recorded last.

`<Table 2-1>` Flow states

| State | Meaning | Alerts |
|---|---|---|
| ok | The latest expected success arrived | A recovery message, if the flow was alerted before |
| warning | Works, but needs attention soon or looked unusual | Yes |
| late | A deadline passed without a success | Yes |
| failed | The last check or run found a problem | Yes |
| blocked | Late or pending because a step before it in a pipeline is broken | No |
| maintenance | Broken while a maintenance window covering it is open | No |
| pending | Has never reported | No |

### 2. Pushed and polled flows

A **pushed** flow reports itself. A `heartbeat` job calls `/ping` when it starts and ends,
and an `inbound` gateway posts the API calls it saw. Pushed flows need no credentials, so a
job's first report can create its flow without anyone editing configuration.

A **polled** flow cannot report, so laplace looks: it logs in to an SFTP server, lists a
bucket, reads a Delta log, calls an API or asks Databricks. Polled flows need credentials,
which is why they are declared in `flows.toml` and reviewed in git.

### 3. When a flow is due

A flow with only `every` is late when no success arrived within `every` plus `grace`.

A flow with `cron` is late when the latest deadline, plus `grace`, has passed and no success
arrived after the deadline before it. This rule accepts a file that arrives early and still
catches one that never comes. With `calendar`, only the business days of the named countries
count: weekends, and each country's public holidays, including Easter, Midsummer, All
Saints' Day, Sweden's National Day and Finland's Independence Day, as well as the days offices
close in practice (Midsummer Eve, Christmas Eve, and in Sweden New Year's Eve). A Databricks job picked from the workspace uses the job's own schedule.

### 4. Pipelines

`after` makes a flow wait for others. When a step is late or failed, every step after it is
**blocked**: shown, but not alerted, so one broken step gives one alert rather than one per
step downstream. A cycle in `after` is rejected when the file is loaded.

### 5. Warnings and anomalies

`<Table 2-2>` Warnings

| Warning | Source |
|---|---|
| Certificate expires within 14 days | `http` checks |
| SFTP host key changed | `sftp` checks. The check stops logging in until an operator confirms the key |
| A run's numbers are unusual | Any flow. A number below half or above double the median of the last ten runs, with the band set by `anomaly_tolerance` |
| A table was left out of the latest load | `delta` checks. Its last commit is more than six hours older than the newest |
| A schema changed | `delta` checks. Columns added, dropped or retyped, shown for 24 hours |

laplace adds numbers of its own to the ones jobs send: `seconds` for heartbeat runs, `bytes`
for files, and `rows`, `files`, `bytes` and `tables` for Delta loads. A load that suddenly
writes far fewer rows is therefore a warning without any extra configuration.

## III. Checks

### 1. The shape of a check

Every polled check returns the same report: the successes it found since the last one, a
problem if there is one, a warning if there is one, and for SFTP the host key it saw. The
caller records the report in one transaction, so a check either counts in full or not at all.

### 2. SFTP and object storage

Both list a folder and apply the same rules to matching files. Each poll's listing is
remembered, file by file, and compared with the next:

- A file counts as arrived once it is unchanged, same size and modified time, on two polls in
  a row. An upload still in progress, or one cut off part way and still growing when seen,
  is therefore not taken for a finished file. The cost is one poll: a file counts at most
  `poll` after it appeared, which the schedule's `grace` should cover.
- A file counts once. One rewritten under the same name counts again when it settles, and one
  removed and uploaded again counts again.
- Arrival is when laplace first saw the file, not its modified time. A file uploaded with an
  older, preserved time (`put -p`, `scp -p`, rsync) still counts, and `pickup` runs from
  arrival too.
- A file that stays empty is a problem; an upload that starts as an empty file is not.
- A flow that ran before files were remembered takes what is in the folder on its first
  remembered poll as its starting point, so nothing old is reported as new.

Storage uses the `object_store` crate for `gs://`, `abfss://` and local paths alike, and
keys files by their full path, so files with the same name in different folders stay apart.

Watching a folder cannot see a file that arrives and is collected between two polls. The
job that does the transfer reporting its own runs covers that; see `operations.md`,
Chapter III, Section 2.

On first contact with an SFTP server, its host key is stored (trust on first use) unless
`host_key` pins it. A later change is treated as a possible interception: laplace stops
logging in until an operator confirms the new key in the dashboard.

### 3. Delta tables

A `delta` flow points at one table or a folder of them, such as pgdelta's output. laplace
walks up to three levels of the folder looking for `_delta_log`, stops at 2,000 tables, and
reads up to eight transaction logs at a time, straight from storage and with no cluster.

A new commit on any table is a success. The check sums rows, files and bytes across tables
(rows only when every table reports a count), remembers each table's schema, and compares it
with the last one seen. The default poll is 15 minutes, since reading hundreds of logs costs
more than listing a folder.

### 4. HTTP and Databricks

`http` calls the URL and checks the status, the latency against `max_latency`, and the
expiry of the certificate. `databricks` asks the Jobs API for the latest runs of `job_id`.

## IV. Concurrency and Scheduling

### 1. Stateless replicas

```
            +-- replica A --+
jobs  ----> +-- replica B --+ ----> PostgreSQL
            +-- replica C --+
```

[Figure 4-1] Replicas sharing one database

Replicas hold no state of their own beyond a short buffer of inbound calls (Section 3).
Everything that must survive a restart, or be seen by every replica, lives in PostgreSQL.
Any replica can serve any request, and replicas can be added or removed at any time.

### 2. The jobs table

Periodic work is a row in the `jobs` table with the time it is next due. Every two seconds,
each replica claims due rows with `SELECT ... FOR UPDATE SKIP LOCKED`, which gives each row to
exactly one replica without any other coordination, and moves its next due time forward.

`<Table 4-1>` Scheduled jobs

| Job | Every | Does |
|---|---|---|
| Check, one per polled flow | The flow's `poll` | Runs the check and records its report |
| Alerts | 30 seconds | Computes every flow's state, records changes and sends alerts |
| Heartbeat | 60 seconds | Calls `LAPLACE_HEARTBEAT_URL`, if set |
| Export | 5 minutes | Appends new history to Delta, if `DELTA_URI` is set |
| Prune | 1 hour | Deletes history past its retention |

The set of jobs is rebuilt from the current flows on every tick, so a flow added to the file
or registered by a job starts being checked within seconds.

### 3. Inbound traffic

A busy gateway can post thousands of calls a minute. Writing each one would tie database load
to partner traffic, so each replica counts calls per flow and per minute in memory and upserts
the counts every two seconds. On shutdown, the buffer is written before the process exits.

### 4. Configuration reload

Each replica rereads `flows.toml` on every tick and compares it with the text it last read.
A changed file that parses replaces the configuration in use; one that does not is refused,
the previous version stays in use, and editors see the reason at the top of the dashboard.
In Kubernetes, a changed ConfigMap reaches the pods within about a minute, with no restart.

### 5. Invariants

- A due job runs on exactly one replica.
- An alert for a state change is sent once, whichever replica computes it, because the state
  last alerted is stored per flow and updated only after delivery.
- A report from a job is recorded in full or not at all.
- The configuration in use always parsed and passed validation.

## V. Alerting

### 1. From state to alert

The alerts job compares each flow's current state with the state it last alerted on. A move
into warning, late or failed sends an alert; a move back to ok from one of those sends a
recovery message. Anything else is recorded for reports but not sent. The full decision is
in Appendix A.

### 2. What keeps an alert quiet

- **Blocked** flows do not alert; the broken step before them already did.
- **Silenced** flows do not alert until the silence ends. If the flow is still broken then,
  the alert goes out.
- **Maintenance** does not alert, and leaves the last alerted state untouched, so what the
  flow is in when the window closes is compared with what was last said about it: a flow
  still broken alerts, a flow that recovered sends nothing new.
- **Acknowledging** a problem records who is on it and shows it in the dashboard. It clears
  itself when the flow is ok again.

### 3. Delivery

An alert goes to the flow owner's webhook and addresses, each falling back to the shared
`ALERT_WEBHOOK` and `ALERT_EMAIL`. The payload follows the webhook's host.

`<Table 5-1>` Alert formats by webhook host

| Host | Format |
|---|---|
| `hooks.slack.com` | Slack Block Kit |
| `*.logic.azure.com`, `*.powerplatform.com` (Teams workflows), `*.webhook.office.com` | Adaptive Card |
| Anything else | JSON with `text`, `flow`, `state`, `detail`, `owner`, `last_ok` and `link` |

Email goes through Microsoft Graph or SMTP (see `reference.md`, Chapter IV, Section 3). An
alert counts as delivered when at least one channel accepted it, so a broken channel does
not repeat the alert on the working one every 30 seconds; the failure is logged. When no
channel accepted it, the state is not marked as alerted, and the next run tries again.

### 4. Acting from an alert

With `LAPLACE_PUBLIC_URL` set, cards and emails carry links to open the failing run,
acknowledge it, or silence it for an hour. A link only opens laplace with the action waiting;
the action happens on a click there. A link preview, a mail scanner or a forwarded message
therefore cannot act, and every action goes through sign-in, the role check and the audit
log like any other.

## VI. Data Model and Retention

### 1. Tables

`<Table 6-1>` PostgreSQL tables

| Table | Holds |
|---|---|
| `flow_state` | One row per flow: last success, current problem and warnings, acknowledgement, silence, last alerted state |
| `events` | Runs: start, success or failure, with duration and numbers |
| `calls` | Inbound calls counted per flow and minute |
| `state_changes` | Every change of state, for reports |
| `registered_flows` | Flows created by jobs reporting in |
| `jobs` | Periodic work and when it is next due |
| `host_keys`, `delta_schemas` | What SFTP servers and Delta tables looked like last time |
| `members`, `group_roles`, `job_tokens`, `settings`, `audit` | Access, settings and the audit log |

Tokens are stored as SHA-256 hashes. Queries are static SQL with bound parameters.

### 2. Retention

Runs and calls are kept for `RETENTION_DAYS`, 30 by default. State changes are kept for 400
days, so a year of reports can be compared. The prune job enforces both.

### 3. Export to Delta

With `DELTA_URI` set, new events, calls and state changes are appended to Delta tables every
five minutes, from a watermark stored in PostgreSQL, so Databricks holds the full history.

Row ids are handed out when a row is inserted, but rows commit in their own order: id 101 can
be visible while id 100 is still being written. An export that took 101 would move the
watermark past 100 and skip it for good. Each row therefore records when it was written, and
the export stops at the first row written less than a minute ago, taking the rest next time.
Calls are exported only for minutes that have ended.

A crash between the Delta commit and the watermark update can append a batch twice; consumers
deduplicate on `id`, and on `(flow, minute)` for calls.

### 4. Reports

Monthly reports are computed from `state_changes`. Each state lasts until the next change.
Time in ok or warning counts as up; pending and maintenance are not counted at all; the rest
is down. The same calculation, cut at local midnights, gives the uptime of each day.

## VII. Security Model

The rules every change must follow are in `SECURITY.md`. In summary: no secret is in the
code or reaches the browser; every handler checks the caller's role on the server; writes
from other sites are refused; the open endpoints validate every input and are rate limited;
the database is reached with a role that cannot change the schema; and the container runs as
a non-root user on a read-only file system with no Kubernetes API token.

## VIII. Assessment

### 1. Advantages

- **Absence is detected, not just errors.** Schedules and calendars say when something should
  have happened.
- **One broken step, one alert.** Pipelines, silences, maintenance windows and delivery
  deduplication keep alerts rare enough to be read.
- **No coordination service.** PostgreSQL row locks are the only mechanism between replicas.
- **Independent of what it watches.** No dependency on Databricks, the lake or any partner.
- **Configuration is code.** Polled flows, owners and windows are reviewed in git, while jobs
  that report in need no configuration at all.

### 2. Disadvantages

- **PostgreSQL is a single dependency.** When it is down, laplace cannot record or alert. The
  dead-man's switch reports this from outside (see `operations.md`, Chapter V, Section 3).
- **Polling has a cost and a delay.** A polled flow is seen at most as often as `poll`.
- **Roles are global.** There is no per-flow permission; any operator can silence any flow.
- **Anomaly detection is simple.** A median of ten runs suits steady loads and flags seasonal
  ones, such as month-end, until the band is widened.

### 3. Known ceiling

Each polled flow is one row in `jobs` and one check per `poll`, and the alerts job reads every
flow every 30 seconds. This comfortably covers thousands of flows on a small PostgreSQL.
Beyond that, the alerts job is the first thing to split, by flow, across replicas.

## References

1. PostgreSQL Global Development Group. *SELECT, The Locking Clause*, PostgreSQL 17
   Documentation. https://www.postgresql.org/docs/17/sql-select.html#SQL-FOR-UPDATE-SHARE
2. Delta Lake Project. *Delta Transaction Log Protocol*.
   https://github.com/delta-io/delta/blob/master/PROTOCOL.md
3. Apache Arrow Project. *object_store*. https://docs.rs/object_store/0.13
4. delta-rs Project. *deltalake*. https://docs.rs/deltalake/1.0
5. Databricks. *Jobs API 2.1*. https://docs.databricks.com/api/workspace/jobs
6. IETF. *RFC 4251, The Secure Shell (SSH) Protocol Architecture*, Section 4.1, Host Keys.
   https://www.rfc-editor.org/rfc/rfc4251#section-4.1
7. Slack. *Block Kit*. https://api.slack.com/block-kit
8. Microsoft. *Adaptive Cards*. https://adaptivecards.io/

---

## Appendix A. Alert decisions

The alerts job applies this table to every flow on every run.

`<Table A-1>` Alert decisions

| Current state | Last alerted | Silenced | Result |
|---|---|---|---|
| Same as last alerted | any | any | Nothing |
| maintenance | any | any | Nothing; the last alerted state is kept |
| any | any | yes | Nothing; the last alerted state is kept |
| warning, late or failed | anything else | no | Alert, then record it as alerted |
| ok | warning, late or failed | no | Recovery message, then record it as alerted |
| any other change | any | no | Record the new state without sending |
