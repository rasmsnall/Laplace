# laplace: Operations

**Document type** Operations manual
**Status** Complete. Describes version 0.2.0 as built.
**Audience** Whoever deploys laplace, keeps it running, and is told when it stops.
**Companion documents** `architecture.md` for the design, `reference.md` for settings and endpoints.
**Version** 1.0
**Date** 2026-09-28

---

## Contents

- I. Introduction
  - 1. Purpose
  - 2. What a deployment consists of
- II. Deployment
  - 1. The image and the chart
  - 2. PostgreSQL in the cluster
  - 3. Any other PostgreSQL
  - 4. Secrets
  - 5. Sign-in
  - 6. Email
  - 7. Network
  - 8. The local demo
  - 9. Databricks cost
- III. Configuration
  - 1. Changing flows
  - 2. Connecting jobs
  - 3. Settings kept in the database
- IV. Sizing
- V. Monitoring laplace Itself
  - 1. Probes
  - 2. Metrics
  - 3. The dead-man's switch
- VI. Failure and Recovery
  - 1. Database failover
  - 2. Restoring the database
  - 3. A flows file that does not parse
  - 4. Alerts that do not arrive
- VII. Change Management
  - 1. Releases
  - 2. Dependency updates
  - 3. Upgrading
- References
- Appendix A. Runbook
- Appendix B. Known gaps

### List of Tables

- `<Table 1-1>` Components of a deployment
- `<Table 2-1>` Secrets and where they are read
- `<Table 4-1>` Default resources
- `<Table 5-1>` What should page someone
- `<Table 6-1>` Symptom to first action
- `<Table B-1>` Known gaps

### List of Figures

- `[Figure 2-1]` The in-cluster database
- `[Figure 6-1]` Restoring next to the broken cluster

---

## I. Introduction

### 1. Purpose

This document covers running laplace in production: deploying it and its database, keeping
its configuration current, watching its health, and recovering when something breaks. It
assumes the design is of no interest until something goes wrong; `architecture.md` explains
it when it is.

### 2. What a deployment consists of

`<Table 1-1>` Components of a deployment

| Component | Provided by | Notes |
|---|---|---|
| laplace, two replicas | `deploy/helm/laplace` | Stateless. The web port behind the ingress, the operations port inside the cluster |
| Migration job | The same chart | Runs before every install and upgrade, with the schema owner's credentials |
| PostgreSQL | `deploy/postgres`, or an existing server | The only stateful part |
| Backups | `deploy/postgres/backups.yaml` | Continuous, to Azure storage or any S3-compatible store |
| An outside heartbeat check | healthchecks.io or similar | Reports when laplace itself goes quiet |

## II. Deployment

### 1. The image and the chart

CI publishes `ghcr.io/rasmsnall/laplace`. A version tag such as `v0.2.0` publishes `:0.2.0`
and `:latest`; every merge to `main` publishes `:main` and `:sha-<commit>`. The chart
installs the version named by its `appVersion` unless `image.tag` says otherwise. The image
is public, so the cluster needs no pull secret.

The chart runs two replicas spread over nodes, with a disruption budget that keeps one
available. Pods run as a non-root user on a read-only root file system, with no capabilities
and no Kubernetes API token. On `SIGTERM` a pod finishes its requests and writes its buffered
call counts before exiting.

### 2. PostgreSQL in the cluster

`deploy/postgres` runs PostgreSQL with CloudNativePG: a primary and a standby on different
nodes, failover by the operator, the write-ahead log archived continuously, and a full backup
every night. Any moment in the last 30 days can be restored.

```
laplace --> laplace-db-rw --> primary --streams--> standby
                                 |
                                 +--WAL and nightly backups--> object store
```

[Figure 2-1] The in-cluster database

Once per cluster, install cert-manager, the operator and its backup plugin:

```
kubectl apply --server-side -f https://github.com/cert-manager/cert-manager/releases/download/v1.21.2/cert-manager.yaml
kubectl apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
kubectl apply --server-side -f https://github.com/cloudnative-pg/plugin-barman-cloud/releases/download/v0.15.0/manifest.yaml
```

Then, with the storage account filled in to `backups.yaml`:

```
kubectl create secret generic laplace-db-app-role --type=kubernetes.io/basic-auth \
  --from-literal=username=laplace_app --from-literal=password="$(openssl rand -hex 24)"
kubectl create secret generic laplace-backup-storage --from-literal=account=... --from-literal=key=...
kubectl apply -f deploy/postgres/backups.yaml -f deploy/postgres/cluster.yaml
kubectl create secret generic laplace --from-literal=LAPLACE_SESSION_KEY="$(openssl rand -base64 64)" ...
helm install laplace deploy/helm/laplace --set-file flows=flows.toml \
  --set database.host=laplace-db-rw \
  --set database.app.passwordSecret=laplace-db-app-role \
  --set database.migrations.passwordSecret=laplace-db-app \
  --set publicUrl=https://laplace.company.se \
  --set ingress.enabled=true --set ingress.host=laplace.company.se
```

The operator creates the schema owner, `laplace_owner`, and puts its password in
`laplace-db-app`. The application role, `laplace_app`, may read and write tables but not
change them. The chart reads each password straight from its secret, so no connection string
is ever assembled by hand.

### 3. Any other PostgreSQL

A managed server, such as Azure Database for PostgreSQL, or one a database team runs, works
the same way. Create the two roles with the grants in `deploy/test/postgres.yaml`, then use
`database.host` as above, or put complete connection strings in the two secrets:

```
kubectl create secret generic laplace --from-literal=DATABASE_URL=postgres://laplace_app:...@db/laplace
kubectl create secret generic laplace-migrations --from-literal=DATABASE_URL=postgres://laplace_owner:...@db/laplace
```

### 4. Secrets

Nothing secret belongs in `flows.toml`, the chart values or git. Fields such as
`password_env` and `webhook_env` name an environment variable instead.

`<Table 2-1>` Secrets and where they are read

| Secret | Kubernetes secret | Read by |
|---|---|---|
| Application database password | `laplace-db-app-role`, or `DATABASE_URL` in `laplace` | The pods |
| Schema owner password | `laplace-db-app`, or `DATABASE_URL` in `laplace-migrations` | The migration job only |
| `LAPLACE_SESSION_KEY`, `LAPLACE_OIDC_CLIENT_SECRET` | `laplace` | The pods |
| Webhooks, `LAPLACE_GRAPH_CLIENT_SECRET` or `LAPLACE_SMTP_URL` | `laplace` | The pods |
| SFTP passwords, `DATABRICKS_TOKEN`, storage keys | `laplace` | The pods |
| Backup storage key | `laplace-backup-storage` | The backup plugin |

### 5. Sign-in

Register an application with the identity provider, with the redirect URI
`{LAPLACE_PUBLIC_URL}/auth/callback`, and set the variables in `reference.md`, Chapter IV,
Section 2. List at least one address in `LAPLACE_ADMINS`; those people are always admins, so
access can never be lost entirely. Without sign-in everyone is an admin, which only suits a
single workstation.

### 6. Email

For Microsoft 365, where SMTP with a password is being retired, send through Microsoft Graph:

1. Register an application in Entra ID and give it the `Mail.Send` application permission,
   with admin consent.
2. Limit it to the laplace mailbox. `Mail.Send` alone lets the application send as any
   mailbox in the tenant; Exchange role-based access control for applications, or an
   application access policy, restricts it to one.
3. Set `LAPLACE_GRAPH_TENANT_ID`, `LAPLACE_GRAPH_CLIENT_ID`, `LAPLACE_GRAPH_CLIENT_SECRET`
   and `LAPLACE_MAIL_FROM`.

laplace signs in as the application and reuses its token until two minutes before expiry.
Any other provider can be reached with `LAPLACE_SMTP_URL` instead.

### 7. Network

laplace listens on all interfaces and uses relative paths, so it works under any host name and
behind a reverse proxy, including under a sub-path. Before exposing it: put TLS in front of
it, turn on sign-in, and make sure Databricks clusters and the servers running scheduled jobs
have a route to `LAPLACE_PUBLIC_URL`. With `networkPolicy.enabled`, only the ingress
controller reaches the web port and only Prometheus the operations port.

### 8. The local demo

```
docker compose up -d --build
```

The dashboard is on http://localhost:8090 and demo alert emails on http://localhost:8025.
The compose file includes a demo SFTP server and demo landing folders. Its passwords are
throwaway values that must never be valid anywhere else.

### 9. Databricks cost

laplace can show what each Databricks job costs, next to its runs and in the monthly report,
and warn when a job suddenly costs far more than usual. It reads Databricks' billing system
tables through a SQL warehouse:

1. Pick or create a SQL warehouse; a small serverless one is enough. Set
   `DATABRICKS_WAREHOUSE_ID` to its id and `DATABRICKS_WORKSPACE_ID` to this workspace's id.
2. Give laplace's token, preferably a service principal's, `CAN USE` on the warehouse and
   read access to the billing tables:

```
GRANT USE CATALOG ON CATALOG system TO `laplace`;
GRANT USE SCHEMA ON SCHEMA system.billing TO `laplace`;
GRANT SELECT ON TABLE system.billing.usage TO `laplace`;
GRANT SELECT ON TABLE system.billing.list_prices TO `laplace`;
```

Every hour laplace fetches the cost per job and day for the last 40 days and replaces what it
had, so late corrections land. Usage reaches the tables within about 12 hours, so costs run
up to yesterday, not up to the minute.

The numbers are Databricks' own charge at list price: before any discount the account has,
and on Azure without the charge for the virtual machines of classic compute. They show which
job costs what and when that changes; the invoice remains the source for the total.

## III. Configuration

### 1. Changing flows

Change `flows.toml` in a pull request; CI runs `laplace check` on it. After merging, update
the ConfigMap with `helm upgrade --reuse-values --set-file flows=flows.toml`. The running pods
pick up the change within about a minute, with no restart. A file that does not parse is
refused and the previous version stays in use (Chapter VI, Section 3).

### 2. Connecting jobs

The **connect** button in the dashboard writes the line or block for each kind of job. Jobs
that report in need no change to `flows.toml`: their first report creates the flow. A Windows
Task Scheduler job is wrapped in `scripts/heartbeat.ps1`, with `LAPLACE_URL` and
`LAPLACE_TOKEN` set for the account the task runs as. Databricks jobs can be picked from the
workspace with one click.

For a file transfer, use two flows: the job doing the transfer reports its own runs, and an
`sftp` or `storage` flow watches the folder, declared `after` the job so one fault gives one
alert. The job's reports catch a transfer that failed or never ran, including one whose file
was collected between two polls, which no folder check can see. The folder check catches what
the job cannot see about itself: the file was never there, was empty, or was not collected.

### 3. Settings kept in the database

People, groups, job tokens, the time zone, retention and auto-registration are changed on the
settings page and take effect on every replica within seconds. Every change is written to the
audit log.

## IV. Sizing

laplace is light: the work is waiting on other systems. The chart's defaults suit a few
hundred flows.

`<Table 4-1>` Default resources

| Part | Default |
|---|---|
| laplace pods | 2 replicas, 100m CPU and 128 MiB requested, 512 MiB limit |
| PostgreSQL | 2 instances, 10 GiB each, 100m CPU and 256 MiB requested, 1 GiB limit |

Storage grows with `RETENTION_DAYS` and the number of runs. Many `delta` flows over large
folders cost the most, since each poll reads the transaction logs; raise their `poll` before
adding resources.

## V. Monitoring laplace Itself

### 1. Probes

The liveness probe calls `/healthz` on the operations port, which fails when the scheduler has
not ticked for 60 seconds, so a stuck pod is restarted. The readiness probe calls `/readyz`,
which fails when PostgreSQL does not answer within two seconds, so a pod without a database is
taken out of the service.

### 2. Metrics

`/metrics` on the operations port serves Prometheus metrics. `metrics.serviceMonitor=true`
adds a ServiceMonitor.

`<Table 5-1>` What should page someone

| Condition | Meaning |
|---|---|
| `laplace_scheduler_lag_seconds` above 60 | Checks and alerts have stopped on that replica |
| No ready laplace pods | Nobody can see the dashboard, and jobs cannot report |
| The outside heartbeat check fails | laplace, or its database, is down |

### 3. The dead-man's switch

laplace cannot report its own absence. Set `LAPLACE_HEARTBEAT_URL` to a check on
healthchecks.io or a similar service. laplace calls it every minute while its scheduler runs
and PostgreSQL answers, so when either stops, that service raises the alarm. Keep the check
outside the cluster laplace runs in.

## VI. Failure and Recovery

### 1. Database failover

When the primary fails, the operator promotes the standby. Measured on a test cluster, the
new primary took over three seconds after the old one was killed, and one of 90 job reports
sent during the switch failed. laplace reconnects by itself. `scripts/heartbeat.ps1` tries each
report three times, two seconds apart; other jobs should retry a failed report the same way.

### 2. Restoring the database

`deploy/postgres/restore.yaml` builds a new cluster from the backups next to the broken one,
optionally up to a moment before something went wrong.

```
backups --> laplace-db-restored --> check the data --> point laplace at it
```

[Figure 6-1] Restoring next to the broken cluster

1. If needed, set `recoveryTarget.targetTime` in `restore.yaml`, then apply it and wait for
   the new cluster to be ready.
2. Check the data in `laplace-db-restored`.
3. Switch laplace over: `helm upgrade --reuse-values --set database.host=laplace-db-restored-rw
   --set database.migrations.passwordSecret=laplace-db-restored-app`.
4. Point `laplace-db-nightly` in `backups.yaml` at the new cluster, so nightly backups follow.

Tested end to end: a restore brought back every row, including rows written after the last
nightly backup, and the restored cluster archives its own log.

### 3. A flows file that does not parse

The pods keep the previous version, log the reason, and show it to editors at the top of the
dashboard and on the settings page. Fix the file and apply it again. A new pod started while
the file is broken cannot start at all, which is why the file is checked in CI.

### 4. Alerts that do not arrive

`<Table 6-1>` Symptom to first action

| Symptom | Likely cause | First action |
|---|---|---|
| Nothing arrives, and the flow shows the problem | No webhook or address for its owner, and no shared one | Check the owner in the settings page, where missing webhooks are marked |
| Log says `alert ... not delivered` | Every channel refused it | Check the webhook address and email settings. The alert is retried every 30 seconds |
| Log says `partly delivered` | One channel failed, another worked | Fix the failing channel. The alert is not repeated |
| Email fails with `graph sign-in` | Wrong tenant, client id or secret, or an expired secret | Renew the client secret in Entra ID |
| Email fails with `graph sendMail: 403` | The application may not send as that mailbox | Check the permission and the access policy |
| Cards arrive without buttons | `LAPLACE_PUBLIC_URL` is not set | Set it |
| A flow never alerts | It is silenced, in maintenance, or blocked | The dashboard shows which |

## VII. Change Management

### 1. Releases

A release is a pull request that sets the chart's `version` and `appVersion` and the crate
version, followed by a tag `vX.Y.Z` on its merge commit. The tag publishes `:X.Y.Z`, which the
chart then installs by default.

### 2. Dependency updates

Dependabot opens updates weekly. Patch updates, and minor updates of packages at 1.0 or above,
merge themselves once CI passes; major updates, and minor updates of 0.x packages, wait for a
person. `object_store` moves only when `deltalake` does, so the binary carries one copy. The
image is also rebuilt every Monday, which picks up fixes in its Debian base.

### 3. Upgrading

`helm upgrade` runs the migration job first; the pods roll one at a time, and requests are not
dropped. The migrations so far only add to the schema or widen it, so the previous version
keeps working against the new schema while the rollout runs.

## References

1. CloudNativePG. *CloudNativePG documentation*. https://cloudnative-pg.io/documentation/current/
2. CloudNativePG. *Barman Cloud plugin*. https://cloudnative-pg.io/plugin-barman-cloud/
3. Microsoft. *Limit Microsoft Graph access to specific Exchange mailboxes*, role-based access
   control for applications in Exchange Online.
   https://learn.microsoft.com/en-us/exchange/permissions-exo/application-rbac
4. Microsoft. *user: sendMail*, Microsoft Graph v1.0.
   https://learn.microsoft.com/en-us/graph/api/user-sendmail
5. Kubernetes. *Configure Liveness, Readiness and Startup Probes*.
   https://kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes/
6. Healthchecks.io. *Documentation*. https://healthchecks.io/docs/
7. GitHub. *Dependabot options reference*.
   https://docs.github.com/en/code-security/dependabot/working-with-dependabot/dependabot-options-reference

---

## Appendix A. Runbook

```
# is laplace healthy?
kubectl get pods -l app.kubernetes.io/name=laplace
kubectl logs deploy/laplace --since=15m

# is the database healthy, and which instance is primary?
kubectl get cluster laplace-db

# apply a changed flows.toml
helm upgrade laplace deploy/helm/laplace --reuse-values --set-file flows=flows.toml

# validate a flows file before merging
laplace check flows.toml

# take a backup now
kubectl apply -f - <<EOF
apiVersion: postgresql.cnpg.io/v1
kind: Backup
metadata: { name: laplace-db-now }
spec:
  cluster: { name: laplace-db }
  method: plugin
  pluginConfiguration: { name: barman-cloud.cloudnative-pg.io }
EOF
```

## Appendix B. Known gaps

`<Table B-1>` Known gaps

| Gap | Effect | Workaround |
|---|---|---|
| Roles are global | Any operator can silence any flow | Keep the operator role to the people on call |
| No escalation | An unacknowledged alert is not sent anywhere else | Route owners to a channel that is watched |
| Buttons in Slack and Teams open laplace | One more click than acting inside the chat | None needed; this is also what keeps previews from acting |
| Anomalies use a median of ten runs | Month-end and other seasonal loads can warn | Raise `anomaly_tolerance` for those flows |
| Graph email is HTML only | Mail clients without HTML show little | Use SMTP, which sends plain text as well |
| A restore does not move the nightly schedule | Backups keep targeting the old cluster | Step 4 of Chapter VI, Section 2 |
| Databricks cost is at list price | Discounts and the cloud's machine charges are not in it | Use it to compare jobs and spot changes; the invoice for the total |
