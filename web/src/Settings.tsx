import { useEffect, useState, type ReactNode } from "react";

type Role = "viewer" | "operator" | "editor" | "admin";

interface Grant {
  name: string;
  role: Role;
  added_by: string;
  added_at: string;
}

interface Token {
  id: number;
  name: string;
  created_by: string;
  created_at: string;
  last_used_at: string | null;
  revoked_at: string | null;
}

interface General {
  timezone: string;
  retention_days: number;
  auto_register: boolean;
}

interface Overview {
  sso: boolean;
  you: string;
  general: General;
  defaults: General;
  admins: string[];
  members: Grant[];
  groups: Grant[];
  tokens: Token[];
  legacy_token: boolean;
  owners: { id: string; webhook_env: string | null; webhook_set: boolean; email: string[] }[];
  config_flows: { id: string; kind: string; owner: string | null }[];
  maintenance: { name: string; cron: string; lasts: string; flows: string[]; open_until: string | null }[];
  config_error: string | null;
  email_ready: boolean;
}

interface AuditEntry {
  at: string;
  by: string;
  action: string;
  detail: string;
}

const ROLES: Role[] = ["viewer", "operator", "editor", "admin"];
const ROLE_HELP: Record<Role, string> = {
  viewer: "sees flows, pipelines, the timeline and reports",
  operator: "also acknowledges, silences and trusts changed host keys",
  editor: "also connects jobs and picks databricks jobs",
  admin: "also changes access, tokens and settings",
};
const LOCALE = "en-GB";
const date = (iso: string | null) => (iso ? new Date(iso).toLocaleString(LOCALE) : "never");

async function send(url: string, method: string, body?: unknown) {
  const response = await fetch(url, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await response.text();
  const data = text ? JSON.parse(text) : null;
  if (!response.ok) throw new Error(data?.error ?? response.statusText);
  return data;
}

export function Settings() {
  const [overview, setOverview] = useState<Overview | null>(null);
  const [audit, setAudit] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    send("api/settings", "GET").then(setOverview, (e: Error) => setError(e.message));
    send("api/settings/audit", "GET").then(setAudit, () => {});
  };
  useEffect(load, []);

  /** runs a change, reloads on success and shows the server's reason on failure */
  const change = async (url: string, method: string, body?: unknown) => {
    try {
      const result = await send(url, method, body);
      setError(null);
      load();
      return result;
    } catch (e) {
      setError((e as Error).message);
      return null;
    }
  };

  if (!overview) {
    return <section className="pt-12 text-fog-500">{error ?? "loading"}</section>;
  }

  return (
    <section className="pt-12">
      <h2 className="display mb-8 text-[32px] leading-tight">Settings</h2>

      {!overview.sso && (
        <p className="mb-8 border-l-2 border-gold-500 pl-3 text-[13px] text-gold-300">
          sign-in is off, so everyone who can reach laplace is an admin. set LAPLACE_OIDC_ISSUER to use the access rules below.
        </p>
      )}
      {error && (
        <p role="alert" className="mb-8 border-l-2 border-red-500 pl-3 font-mono text-[13px] text-red-300">
          {error}
        </p>
      )}

      <div className="grid gap-12 lg:grid-cols-12">
        <div className="space-y-12 lg:col-span-7">
          <Section title="people" hint="a person's role is the highest one they get from here, their groups or LAPLACE_ADMINS">
            <GrantList
              grants={[
                ...overview.admins.map((email) => ({ name: email, role: "admin" as Role, added_by: "LAPLACE_ADMINS", added_at: "" })),
                ...overview.members,
              ]}
              fixed={(g) => g.added_by === "LAPLACE_ADMINS"}
              you={overview.you}
              onRole={(g, role) => change("api/settings/members", "POST", { name: g.name, role })}
              onRemove={(g) => change(`api/settings/members/${encodeURIComponent(g.name)}`, "DELETE")}
            />
            <AddGrant placeholder="name@company.se" label="email" onAdd={(name, role) => change("api/settings/members", "POST", { name, role })} />
          </Section>

          <Section title="groups" hint="everyone in an sso group gets its role. entra id sends group object ids unless the app is set to send names">
            <GrantList
              grants={overview.groups}
              fixed={() => false}
              onRole={(g, role) => change("api/settings/groups", "POST", { name: g.name, role })}
              onRemove={(g) => change(`api/settings/groups/${encodeURIComponent(g.name)}`, "DELETE")}
            />
            <AddGrant placeholder="sg-it-operations or an object id" label="group" onAdd={(name, role) => change("api/settings/groups", "POST", { name, role })} />
          </Section>

          <Section title="job tokens" hint="jobs send one on /ping and /calls. from the first token on, reporting always needs one, even if every token is revoked">
            <Tokens tokens={overview.tokens} legacy={overview.legacy_token} change={change} />
          </Section>
        </div>

        <div className="space-y-12 lg:col-span-5">
          <Section title="roles">
            <dl className="space-y-2 text-[13px]">
              {ROLES.map((role) => (
                <div key={role} className="grid grid-cols-[80px_1fr] gap-3">
                  <dt className="font-mono text-fog-100">{role}</dt>
                  <dd className="text-fog-500">{ROLE_HELP[role]}</dd>
                </div>
              ))}
            </dl>
          </Section>

          <Section title="general">
            <GeneralForm general={overview.general} defaults={overview.defaults} onSave={(values) => change("api/settings/general", "PUT", values)} />
          </Section>

          <Section title="from git" hint="flows.toml is reviewed in git, so these are read-only here">
            <ul className="divide-y divide-line rounded-md border border-line text-[13px]">
              {overview.owners.map((owner) => (
                <li key={owner.id} className="flex items-center justify-between gap-3 px-3 py-2">
                  <span className="text-fog-100">{owner.id}</span>
                  <span className="flex flex-wrap justify-end gap-x-3 font-mono text-[11px]">
                    {owner.email.length > 0 && (
                      <span className={overview.email_ready ? "text-fog-500" : "text-gold-300"} title={overview.email_ready ? undefined : "LAPLACE_SMTP_URL is not set"}>
                        {owner.email.join(", ")}
                      </span>
                    )}
                    <span className={owner.webhook_set ? "text-fog-500" : "text-gold-300"}>
                      {owner.webhook_env ? `${owner.webhook_env} ${owner.webhook_set ? "set" : "not set"}` : "shared webhook"}
                    </span>
                  </span>
                </li>
              ))}
              {overview.maintenance.map((window) => (
                <li key={window.name} className="flex items-center justify-between gap-3 px-3 py-2">
                  <span className="text-fog-100">
                    {window.name} <span className="text-fog-500">· maintenance</span>
                  </span>
                  <span className="font-mono text-[11px] text-fog-500">
                    {window.open_until
                      ? <span className="text-gold-300">open until {new Date(window.open_until).toLocaleString("en-GB")}</span>
                      : `${window.cron} for ${window.lasts}`}
                    {" · "}
                    {window.flows.join(", ")}
                  </span>
                </li>
              ))}
              <li className="px-3 py-2 text-fog-500">{overview.config_flows.length} flows defined in flows.toml</li>
            </ul>
            {overview.config_error && (
              <p className="mt-3 border-l-2 border-red-500 pl-3 font-mono text-[12px] whitespace-pre-wrap text-red-300">{overview.config_error}</p>
            )}
          </Section>
        </div>
      </div>

      <Section title="audit log" hint="the last 200 changes and actions" className="mt-12">
        <div className="max-h-96 overflow-y-auto rounded-md border border-line">
          <table className="w-full text-left text-[13px]">
            <tbody className="divide-y divide-line">
              {audit.length === 0 && (
                <tr>
                  <td className="px-3 py-3 text-fog-500">nothing yet</td>
                </tr>
              )}
              {audit.map((entry, i) => (
                <tr key={i}>
                  <td className="w-44 px-3 py-2 font-mono text-[12px] whitespace-nowrap text-fog-500">{date(entry.at)}</td>
                  <td className="w-56 truncate px-3 py-2 text-fog-300">{entry.by}</td>
                  <td className="px-3 py-2 text-fog-100">
                    {entry.action} <span className="text-fog-500">{entry.detail}</span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Section>
    </section>
  );
}

function Section({ title, hint, className = "", children }: { title: string; hint?: string; className?: string; children: ReactNode }) {
  return (
    <section className={className}>
      <h3 className="text-[14px] font-medium text-fog-300">{title}</h3>
      {hint && <p className="mt-1 mb-3 text-[12px] text-fog-700">{hint}</p>}
      <div className={hint ? "" : "mt-3"}>{children}</div>
    </section>
  );
}

function RoleSelect({ value, onChange, disabled }: { value: Role; onChange: (role: Role) => void; disabled?: boolean }) {
  return (
    <select
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value as Role)}
      aria-label="role"
      className="h-7 rounded-[4px] border border-line bg-ink-900 px-2 text-[12px] text-fog-100 disabled:opacity-60"
    >
      {ROLES.map((role) => (
        <option key={role}>{role}</option>
      ))}
    </select>
  );
}

function GrantList({
  grants,
  fixed,
  you,
  onRole,
  onRemove,
}: {
  grants: Grant[];
  fixed: (grant: Grant) => boolean;
  you?: string;
  onRole: (grant: Grant, role: Role) => void;
  onRemove: (grant: Grant) => void;
}) {
  if (grants.length === 0) return <p className="mb-3 text-[13px] text-fog-500">none yet</p>;
  return (
    <ul className="mb-3 divide-y divide-line rounded-md border border-line text-[13px]">
      {grants.map((grant) => (
        <li key={`${grant.added_by}:${grant.name}`} className="flex flex-wrap items-center gap-3 px-3 py-2">
          <span className="min-w-0 flex-1 truncate text-fog-100">
            {grant.name}
            {grant.name === you && <span className="text-fog-500"> · you</span>}
          </span>
          <span className="hidden font-mono text-[11px] text-fog-700 sm:inline">{fixed(grant) ? "from LAPLACE_ADMINS" : `by ${grant.added_by}`}</span>
          <RoleSelect value={grant.role} disabled={fixed(grant)} onChange={(role) => onRole(grant, role)} />
          <button
            onClick={() => onRemove(grant)}
            disabled={fixed(grant)}
            className="h-7 rounded-[4px] px-2 text-[12px] text-fog-500 hover:text-red-300 disabled:invisible"
          >
            remove
          </button>
        </li>
      ))}
    </ul>
  );
}

function AddGrant({ placeholder, label, onAdd }: { placeholder: string; label: string; onAdd: (name: string, role: Role) => Promise<unknown> }) {
  const [name, setName] = useState("");
  const [role, setRole] = useState<Role>("viewer");
  return (
    <form
      className="flex flex-wrap gap-2"
      onSubmit={async (e) => {
        e.preventDefault();
        if (name.trim() && (await onAdd(name.trim(), role)) !== null) setName("");
      }}
    >
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder={placeholder}
        aria-label={label}
        className="h-8 min-w-0 flex-1 rounded-[4px] border border-line bg-ink-900 px-2.5 text-[13px] text-fog-100 outline-none placeholder:text-fog-700 focus-visible:border-gold-500"
      />
      <RoleSelect value={role} onChange={setRole} />
      <button type="submit" className="h-8 rounded-[4px] bg-fog-100 px-3 text-[13px] font-medium text-ink-950 hover:bg-fog-300">
        add
      </button>
    </form>
  );
}

function Tokens({
  tokens,
  legacy,
  change,
}: {
  tokens: Token[];
  legacy: boolean;
  change: (url: string, method: string, body?: unknown) => Promise<{ token?: string } | null>;
}) {
  const [name, setName] = useState("");
  const [created, setCreated] = useState<{ name: string; token: string } | null>(null);

  return (
    <>
      {legacy && <p className="mb-3 text-[12px] text-fog-500">LAPLACE_TOKEN is also accepted, from the environment.</p>}
      {created && (
        <div className="mb-3 rounded-md border border-gold-500/50 p-3 text-[13px]">
          <p className="text-gold-300">copy the token for {created.name} now. it is not shown again.</p>
          <code className="mt-2 block font-mono text-[12px] break-all text-fog-100 select-all">{created.token}</code>
        </div>
      )}
      {tokens.length > 0 && (
        <ul className="mb-3 divide-y divide-line rounded-md border border-line text-[13px]">
          {tokens.map((token) => (
            <li key={token.id} className={`flex flex-wrap items-center gap-3 px-3 py-2 ${token.revoked_at ? "opacity-50" : ""}`}>
              <span className="min-w-0 flex-1 truncate text-fog-100">{token.name}</span>
              <span className="font-mono text-[11px] text-fog-500">
                {token.revoked_at ? `revoked ${date(token.revoked_at)}` : `last used ${date(token.last_used_at)}`}
              </span>
              {!token.revoked_at && (
                <button
                  onClick={() => change(`api/settings/tokens/${token.id}`, "DELETE")}
                  className="h-7 rounded-[4px] px-2 text-[12px] text-fog-500 hover:text-red-300"
                >
                  revoke
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      <form
        className="flex gap-2"
        onSubmit={async (e) => {
          e.preventDefault();
          const result = await change("api/settings/tokens", "POST", { name: name.trim() });
          if (result?.token) {
            setCreated({ name: name.trim(), token: result.token });
            setName("");
          }
        }}
      >
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="what it is for, e.g. task scheduler app01"
          aria-label="token name"
          className="h-8 min-w-0 flex-1 rounded-[4px] border border-line bg-ink-900 px-2.5 text-[13px] text-fog-100 outline-none placeholder:text-fog-700 focus-visible:border-gold-500"
        />
        <button type="submit" className="h-8 rounded-[4px] bg-fog-100 px-3 text-[13px] font-medium text-ink-950 hover:bg-fog-300">
          create
        </button>
      </form>
    </>
  );
}

function GeneralForm({ general, defaults, onSave }: { general: General; defaults: General; onSave: (values: Record<string, string>) => Promise<unknown> }) {
  const [timezone, setTimezone] = useState(general.timezone);
  const [retention, setRetention] = useState(String(general.retention_days));
  const [autoRegister, setAutoRegister] = useState(general.auto_register);
  const [saved, setSaved] = useState(false);

  const input =
    "h-8 w-full rounded-[4px] border border-line bg-ink-900 px-2.5 text-[13px] text-fog-100 outline-none focus-visible:border-gold-500";
  return (
    <form
      className="space-y-4 text-[13px]"
      onSubmit={async (e) => {
        e.preventDefault();
        const result = await onSave({ timezone, retention_days: retention, auto_register: String(autoRegister) });
        setSaved(result !== null);
      }}
    >
      <label className="block text-fog-500">
        time zone <span className="text-fog-700">(default {defaults.timezone})</span>
        <input value={timezone} onChange={(e) => setTimezone(e.target.value)} className={`mt-1 ${input}`} />
      </label>
      <label className="block text-fog-500">
        days of history in postgres <span className="text-fog-700">(default {defaults.retention_days})</span>
        <input type="number" min={1} value={retention} onChange={(e) => setRetention(e.target.value)} className={`mt-1 ${input}`} />
      </label>
      <label className="flex items-center gap-2 text-fog-300">
        <input type="checkbox" checked={autoRegister} onChange={(e) => setAutoRegister(e.target.checked)} className="accent-gold-500" />
        jobs reporting under a new name create their flow
      </label>
      <div className="flex items-center gap-3">
        <button type="submit" className="h-8 rounded-[4px] bg-fog-100 px-3 font-medium text-ink-950 hover:bg-fog-300">
          save
        </button>
        {saved && <span className="text-fog-500">saved</span>}
      </div>
    </form>
  );
}
