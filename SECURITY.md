# security rules

rules for anyone changing laplace, people and coding agents alike. run `scripts/check.sh` before
every merge.

## no secrets in code

- never commit database passwords, session keys, oidc client secrets, webhook urls, databricks
  or cloud tokens. they come from the environment, in production from a secret store.
- the one exception is `compose.yaml`: throwaway passwords for the local demo containers
  (`watch`/`watch`, `partner`/`partner`). they must never be valid anywhere else.
- the browser gets no secrets. the dashboard is static files that call laplace's own api; only
  the rust server holds `DATABASE_URL` and the other keys.

## the server decides

- every api handler checks the caller's role itself (`require(&caller, Role::...)`). hiding a
  button in the ui is convenience, never protection.
- the browser never talks to postgres, databricks or storage directly.
- sessions are encrypted, signed cookies (`httponly`, `samesite=lax`, `secure` behind https).
  roles are looked up on every request, so removing someone takes effect at once.
- writes to `/api/` from another site are refused (origin check), on top of `samesite`.
- nobody can remove or demote themselves, and `LAPLACE_ADMINS` are always admins, so access
  cannot be lost by accident.

## open endpoints

only these answer without a signed-in user: `/ping`, `/calls`, `/connect/heartbeat.ps1`,
`/auth/*` and `/healthz`. anything added there must:

- validate every input (ids, durations, cron, numbers, sizes) and reject the rest with a reason,
- stay within the rate limits in `src/guard.rs`,
- never reveal whether a flow, user or token exists beyond what a job needs.

job tokens are stored as sha-256 hashes and shown once. from the first token on, reporting
always needs one, even when every token is revoked.

## database

- queries are static sql with bound parameters; sqlx 0.9 refuses sql built at runtime, keep it that way.
- no `security definer` functions. if one is ever needed: fixed `search_path`, execute granted
  only to the role that needs it.
- in production run migrations with an owner role and laplace with a role that can read and
  write the tables but not change them.

## accepted advisories

`cargo audit` reads `.cargo/audit.toml`. each entry there says why it cannot hurt laplace today and
when to remove it:

- `RUSTSEC-2023-0071` (rsa, marvin): only public-key signature checks, no private keys held.
- `RUSTSEC-2026-0194`, `RUSTSEC-2026-0195` (quick-xml): only parses azure and gcs responses over
  tls; waits on a deltalake release that moves to object_store 0.14. pgdelta shares it.

## pre-merge checklist

`scripts/check.sh` runs these; all must pass:

- secret scan of the repository (gitleaks)
- `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`
- `cargo audit` for known vulnerable rust crates, `npm audit` for the dashboard
- the dashboard builds

and by hand, when the change touches them:

- a new open endpoint: validated, rate limited, listed above
- a new api handler: has a role check, and a test or curl check that a lower role gets 403
- a new setting or secret: read from the environment or the settings table, never hard-coded
