import { useEffect, useRef, useState } from "react";

import { Icon } from "./icons";
import { writeClipboard } from "./CopyLink";

interface Settings {
  token_required: boolean;
  auto_register: boolean;
  public_url: string | null;
}

type Method = "task scheduler" | "databricks" | "shell" | "inbound api" | "polled";
type Polled = "sftp" | "storage" | "delta" | "http" | "databricks job";

const METHODS: Method[] = ["task scheduler", "databricks", "shell", "inbound api", "polled"];
const POLLED: Polled[] = ["sftp", "storage", "delta", "http", "databricks job"];
const SCHEDULES = ["15m", "1h", "1d", "7d"];

const notes: Record<Method, string> = {
  "task scheduler": "run the first block once per server, then use the second as the task's action. the job still runs if laplace is down.",
  databricks: "put this around the notebook's work. the first run creates the flow. the cluster needs network access to laplace.",
  shell: "for cron or any script. the job's exit code decides ok or failed.",
  "inbound api": "have the gateway post each call, or a batch every few seconds. laplace counts them per minute.",
  polled: "laplace checks these itself, so they need credentials: add the block to flows.toml and restart.",
};

export function Connect({ onClose }: { onClose: () => void }) {
  const [name, setName] = useState("");
  const [every, setEvery] = useState("1d");
  const [method, setMethod] = useState<Method>("task scheduler");
  const [polled, setPolled] = useState<Polled>("sftp");
  const [cron, setCron] = useState("");
  const [calendar, setCalendar] = useState<string[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const nameInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    nameInput.current?.focus();
    fetch("api/connect")
      .then((response) => response.json())
      .then(setSettings, () => {});
    const closeOnEscape = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose]);

  const id = slug(name) || "my-job";
  const code = snippet(method, polled, {
    origin: settings?.public_url ?? dashboardUrl(),
    id,
    every,
    cron: cron.trim(),
    calendar,
    token: settings?.token_required ?? false,
  });

  return (
    <div className="fixed inset-0 z-40 flex justify-end" role="dialog" aria-modal="true" aria-labelledby="connect-title">
      <div className="absolute inset-0 bg-black/50" onClick={onClose} />
      <aside className="relative flex h-full w-full max-w-xl flex-col overflow-y-auto border-l border-line bg-ink-950 p-6">
        <div className="mb-8 flex items-start justify-between gap-4">
          <div>
            <h2 id="connect-title" className="display text-[22px]">
              connect a job
            </h2>
            <p className="mt-2 text-[13px] text-fog-500">one line in the job. the flow appears the first time it reports.</p>
          </div>
          <button onClick={onClose} aria-label="close" className="grid size-8 place-items-center rounded-[4px] text-fog-500 hover:bg-ink-850 hover:text-fog-100">
            <Icon name="close" className="size-4" />
          </button>
        </div>

        <div className="grid grid-cols-[1fr_auto] gap-3">
          <label className="text-[12px] text-fog-500">
            flow name
            <input
              ref={nameInput}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="my-job"
              className="mt-1.5 h-9 w-full rounded-[4px] border border-line bg-ink-900 px-3 font-mono text-[13px] text-fog-100 outline-none placeholder:text-fog-700 focus-visible:border-gold-500"
            />
          </label>
          <label className="text-[12px] text-fog-500">
            expected every
            <select
              value={every}
              onChange={(e) => setEvery(e.target.value)}
              className="mt-1.5 block h-9 rounded-[4px] border border-line bg-ink-900 px-3 font-mono text-[13px] text-fog-100 outline-none focus-visible:border-gold-500"
            >
              {SCHEDULES.map((schedule) => (
                <option key={schedule}>{schedule}</option>
              ))}
            </select>
          </label>
        </div>
        {name && slug(name) !== name && <p className="mt-2 font-mono text-[11px] text-fog-500">saved as {id}</p>}

        <div className="mt-4 grid grid-cols-[1fr_auto] items-end gap-3">
          <label className="text-[12px] text-fog-500">
            or a schedule (cron, optional)
            <input
              value={cron}
              onChange={(e) => setCron(e.target.value)}
              placeholder="0 7 * * 1-5"
              className="mt-1.5 h-9 w-full rounded-[4px] border border-line bg-ink-900 px-3 font-mono text-[13px] text-fog-100 outline-none placeholder:text-fog-700 focus-visible:border-gold-500"
            />
          </label>
          <fieldset className="flex h-9 items-center gap-1">
            <legend className="sr-only">business days</legend>
            {["se", "fi"].map((code) => (
              <button
                key={code}
                type="button"
                aria-pressed={calendar.includes(code)}
                onClick={() => setCalendar((all) => (all.includes(code) ? all.filter((c) => c !== code) : [...all, code]))}
                title={`skip ${code === "se" ? "swedish" : "finnish"} weekends and public holidays`}
                className={`h-9 rounded-[4px] border px-3 font-mono text-[12px] ${
                  calendar.includes(code) ? "border-fog-100 bg-fog-100 text-ink-950" : "border-line text-fog-300 hover:text-fog-100"
                }`}
              >
                {code}
              </button>
            ))}
          </fieldset>
        </div>

        <div className="mt-6 flex flex-wrap gap-1 border-b border-line pb-3" role="tablist">
          {METHODS.map((m) => (
            <button
              key={m}
              role="tab"
              aria-selected={m === method}
              onClick={() => setMethod(m)}
              className={`rounded-[4px] px-2.5 py-1 text-[13px] ${m === method ? "bg-fog-100 text-ink-950" : "text-fog-300 hover:bg-ink-850"}`}
            >
              {m}
            </button>
          ))}
        </div>

        {method === "polled" && (
          <div className="mt-3 flex gap-1">
            {POLLED.map((p) => (
              <button
                key={p}
                onClick={() => setPolled(p)}
                className={`rounded-[4px] border px-2 py-0.5 font-mono text-[12px] ${
                  p === polled ? "border-fog-500 text-fog-100" : "border-line text-fog-500 hover:text-fog-300"
                }`}
              >
                {p}
              </button>
            ))}
          </div>
        )}

        <p className="mt-4 text-[13px] leading-relaxed text-fog-300">{notes[method]}</p>
        {settings && !settings.auto_register && method !== "polled" && (
          <p className="mt-2 text-[13px] text-gold-300">auto-registration is off here, so add the flow to flows.toml first.</p>
        )}

        <CodeBlock code={code} />

        {method === "databricks" && <WorkspaceJobs />}
      </aside>
    </div>
  );
}

interface WorkspaceJob {
  job_id: number;
  name: string;
  cron: string | null;
  timezone: string | null;
  paused: boolean;
  monitored_as: string | null;
}

/** every job in the databricks workspace, each one click away from being monitored */
function WorkspaceJobs() {
  const [jobs, setJobs] = useState<WorkspaceJob[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const response = await fetch("api/databricks/jobs");
      const body = await response.json();
      if (!response.ok) throw new Error(body.error ?? response.statusText);
      setJobs(body);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  const monitor = async (job: WorkspaceJob) => {
    const response = await fetch(`api/databricks/jobs/${job.job_id}/monitor`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    });
    const body = await response.json();
    if (!response.ok) return setError(body.error ?? response.statusText);
    setJobs((all) => all?.map((j) => (j.job_id === job.job_id ? { ...j, monitored_as: body.flow } : j)) ?? null);
  };

  return (
    <section className="mt-8 border-t border-line pt-6">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h3 className="text-[13px] font-medium text-fog-100">or pick jobs from the workspace</h3>
          <p className="mt-1 text-[12px] text-fog-500">no code in the notebook: laplace polls the job's runs. needs DATABRICKS_HOST and DATABRICKS_TOKEN.</p>
        </div>
        <button
          onClick={load}
          disabled={loading}
          className="h-8 shrink-0 rounded-[4px] border border-line px-3 text-[13px] text-fog-300 hover:border-fog-700 hover:text-fog-100 disabled:opacity-50"
        >
          {loading ? "loading" : jobs ? "refresh" : "list workspace jobs"}
        </button>
      </div>
      {error && <p className="mt-3 text-[13px] text-red-300">{error}</p>}
      {jobs && (
        <ul className="mt-4 divide-y divide-line rounded-md border border-line">
          {jobs.length === 0 && <li className="px-3 py-3 text-[13px] text-fog-500">no jobs in this workspace</li>}
          {jobs.map((job) => (
            <li key={job.job_id} className="flex items-center justify-between gap-3 px-3 py-2.5">
              <div className="min-w-0">
                <div className="truncate text-[13px] text-fog-100">{job.name}</div>
                <div className="truncate font-mono text-[11px] text-fog-500">
                  {job.cron ? `${job.cron} ${job.timezone ?? ""}` : "no schedule"}
                  {job.paused && " · paused"}
                </div>
              </div>
              {job.monitored_as ? (
                <span className="shrink-0 font-mono text-[12px] text-fog-500">watching as {job.monitored_as}</span>
              ) : (
                <button
                  onClick={() => monitor(job)}
                  className="h-7 shrink-0 rounded-[4px] bg-fog-100 px-2.5 text-[12px] font-medium text-ink-950 hover:bg-fog-300"
                >
                  monitor
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function CodeBlock({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);

  const pre = useRef<HTMLPreElement>(null);

  const copy = async () => {
    if (await writeClipboard(code, pre.current)) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }
  };

  return (
    <div className="relative mt-4">
      <button
        onClick={copy}
        className="absolute top-2 right-2 flex items-center gap-1.5 rounded-[4px] border border-line bg-ink-950 px-2 py-1 text-[12px] text-fog-300 hover:text-fog-100"
      >
        <Icon name={copied ? "check" : "copy"} className="size-3.5" />
        {copied ? "copied" : "copy"}
      </button>
      <pre ref={pre} className="overflow-x-auto rounded-md border border-line bg-ink-900 p-4 pt-10 font-mono text-[12px] leading-relaxed text-fog-100">
        {code}
      </pre>
    </div>
  );
}

/** the dashboard's own address, including any sub-path a proxy serves it under */
function dashboardUrl() {
  return new URL(".", window.location.href).href.replace(/\/$/, "");
}

function slug(name: string) {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

interface Target {
  origin: string;
  id: string;
  every: string;
  cron: string;
  calendar: string[];
  token: boolean;
}

/** how the job describes its own schedule when it reports */
function announce({ every, cron, calendar }: Target) {
  let query = cron ? `cron=${encodeURIComponent(cron)}` : `every=${every}`;
  if (calendar.length) query += `&calendar=${calendar.join(",")}`;
  return query;
}

function snippet(method: Method, polled: Polled, target: Target): string {
  const { origin, id, every, cron, calendar, token } = target;
  const query = announce(target);
  switch (method) {
    case "task scheduler":
      return `# once per server
New-Item -ItemType Directory -Force C:\\laplace | Out-Null
Invoke-WebRequest ${origin}/connect/heartbeat.ps1 -OutFile C:\\laplace\\heartbeat.ps1
[Environment]::SetEnvironmentVariable("LAPLACE_URL", "${origin}", "Machine")${
        token ? `\n[Environment]::SetEnvironmentVariable("LAPLACE_TOKEN", "<token>", "Machine")` : ""
      }

# the task's action
powershell.exe -NoProfile -File C:\\laplace\\heartbeat.ps1 -Flow ${id} ${
        cron ? `-Cron "${cron}"` : `-Every ${every}`
      }${calendar.length ? ` -Calendar ${calendar.join(",")}` : ""} -Command C:\\jobs\\your-job.exe`;

    case "databricks":
      return `import requests

LAPLACE = "${origin}"
HEADERS = ${token ? `{"Authorization": "Bearer " + dbutils.secrets.get("laplace", "token")}` : "{}"}

def report(result, numbers=None):
    try:
        requests.post(f"{LAPLACE}/ping/${id}/{result}?${query}", json=numbers, headers=HEADERS, timeout=10)
    except requests.RequestException:
        pass  # never fail the job because monitoring is down

report("start")
try:
    rows = ...  # the job
    report(0, {"rows": rows})  # optional numbers; laplace warns when they look unusual
except Exception:
    report(1)
    raise`;

    case "shell": {
      const auth = token ? ` -H "Authorization: Bearer $LAPLACE_TOKEN"` : "";
      return `LAPLACE_URL=${origin}
curl -fsS -m 10 -X POST "$LAPLACE_URL/ping/${id}/start?${query}"${auth} || true
your-job; code=$?
# optional: add -d '{"rows": 1204331}' to report numbers about the run
curl -fsS -m 10 -X POST "$LAPLACE_URL/ping/${id}/$code?${query}"${auth} || true
exit $code`;
    }

    case "inbound api": {
      const auth = token ? ` \\\n  -H "Authorization: Bearer $LAPLACE_TOKEN"` : "";
      return `curl -X POST "${origin}/calls/${id}?${query}" \\
  -H "content-type: application/json"${auth} \\
  -d '[{"status": 200, "millis": 35}, {"status": 503, "millis": 900}]'`;
    }

    case "polled":
      return polledBlock(polled, id, every, cron, calendar);
  }
}

function polledBlock(polled: Polled, id: string, every: string, cron: string, calendar: string[]): string {
  const when = cron
    ? `cron = "${cron}"${calendar.length ? `\ncalendar = [${calendar.map((c) => `"${c}"`).join(", ")}]` : ""}`
    : `every = "${every}"`;
  const head = `[[flow]]\nid = "${id}"\n${when}\ngrace = "1h"`;
  switch (polled) {
    case "sftp":
      return `${head}
kind = "sftp"
host = "sftp.partner.example"
user = "partner"
key_file = "/keys/partner"      # or password_env = "SFTP_PARTNER_PASSWORD"
dir = "/outgoing"
pattern = "*.xml"
pickup = "1h"                   # optional: alert when a file is not collected`;
    case "storage":
      return `${head}
kind = "storage"
url = "abfss://landing@account.dfs.core.windows.net/pg/"   # or gs://bucket/prefix/
pattern = "*.sql.gz"`;
    case "delta":
      return `${head}
kind = "delta"
url = "abfss://raw@account.dfs.core.windows.net/pg/"   # one table, or a folder of them
# checks the last commit, rows, files, bytes and schema of every table beneath`;
    case "http":
      return `${head}
kind = "http"
url = "https://api.partner.example/health"
max_latency = "2s"`;
    case "databricks job":
      return `${head}
kind = "databricks"
job_id = 123456                 # needs DATABRICKS_HOST and DATABRICKS_TOKEN`;
  }
}
