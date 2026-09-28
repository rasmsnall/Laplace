# Security Rules

Rules for anyone changing laplace, people and coding agents alike. Run `scripts/check.sh`
before every merge; CI runs it on every pull request, and `main` accepts nothing that fails.

## No secrets in code

- Never commit database passwords, session keys, OpenID Connect client secrets, webhook
  addresses, or Databricks and cloud credentials. They come from the environment, in
  production from Kubernetes secrets.
- The one exception is `compose.yaml`: throwaway passwords for the local demo containers
  (`watch`/`watch`, `partner`/`partner`), and the made-up emulator key in
  `deploy/test/cnpg-azurite.yaml`. They must never be valid anywhere else.
- The browser gets no secrets. The dashboard is static files that call laplace's own API;
  only the Rust server holds `DATABASE_URL` and the other keys.

## The server decides

- Every API handler checks the caller's role itself (`require(&caller, Role::...)`). Hiding a
  button in the dashboard is a convenience, never a protection.
- The browser never talks to PostgreSQL, Databricks or storage directly.
- Sessions are encrypted, signed cookies (`HttpOnly`, `SameSite=Lax`, `Secure` behind HTTPS).
  Roles are looked up on every request, so removing someone takes effect at once.
- Writes to `/api/` from another site are refused by an origin check, on top of `SameSite`.
- Nobody can remove or demote themselves, and `LAPLACE_ADMINS` are always admins, so access
  cannot be lost by accident.
- A link in an alert only opens laplace; the action needs a click there, through sign-in,
  the role check and the audit log.

## Open endpoints

Only these answer without a signed-in user: `/ping`, `/calls`, `/connect/heartbeat.ps1`,
`/auth/*` and `/healthz`. Anything added to them must:

- validate every input (ids, durations, cron expressions, numbers, sizes) and reject the rest
  with a reason,
- stay within the rate limits in `src/guard.rs`,
- never reveal whether a flow, user or token exists beyond what a job needs.

Job tokens are stored as SHA-256 hashes and shown once. From the first token on, reporting
always needs one, even when every token is revoked.

## Database

- Queries are static SQL with bound parameters. sqlx 0.9 refuses SQL built at run time; keep
  it that way.
- No `SECURITY DEFINER` functions. If one is ever needed, give it a fixed `search_path` and
  grant `EXECUTE` only to the role that needs it.
- In production, migrations run as the schema owner and laplace as a role that can read and
  write the tables but not change them.

## Accepted advisories

`cargo audit` reads `.cargo/audit.toml`. Each entry there says why it cannot affect laplace
today and when to remove it:

- `RUSTSEC-2023-0071` (rsa, Marvin attack): used only for public-key signature checks; no
  private keys are held.
- `RUSTSEC-2026-0194`, `RUSTSEC-2026-0195` (quick-xml): parses only Azure and Google Cloud
  Storage responses over TLS. It goes when `deltalake` moves to `object_store` 0.14. pgdelta
  shares it.

## Pre-merge checklist

`scripts/check.sh` runs these, and all must pass:

- a secret scan of the repository (gitleaks),
- `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test`,
- `cargo audit` for known vulnerable Rust crates, and `npm audit` for the dashboard,
- the dashboard build,
- `laplace check flows.toml`, and validation of the Helm chart and the PostgreSQL manifests.

And by hand, when a change touches them:

- a new open endpoint: validated, rate limited, and listed above,
- a new API handler: a role check, and a test or request showing that a lower role gets 403,
- a new setting or secret: read from the environment or the settings table, never hard-coded.
