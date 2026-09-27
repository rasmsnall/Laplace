import { useEffect, useState } from "react";

import { CopyLink } from "./CopyLink";
import { go, shareLink } from "./route";

interface FlowReport {
  id: string;
  kind: string;
  owner: string | null;
  uptime: number | null;
  target: number | null;
  met: boolean | null;
  incidents: number;
  longest_outage_seconds: number;
  mean_recovery_seconds: number | null;
  runs_ok: number;
  runs_failed: number;
}

interface Report {
  from: string;
  to: string;
  flows: FlowReport[];
}

const LOCALE = "en-GB";

function currentMonth() {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;
}

function shiftMonth(month: string, by: number) {
  const [year, m] = month.split("-").map(Number);
  const date = new Date(year, m - 1 + by, 1);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}`;
}

function monthName(month: string) {
  const [year, m] = month.split("-").map(Number);
  return new Date(year, m - 1, 1).toLocaleDateString(LOCALE, { month: "long", year: "numeric" }).toLowerCase();
}

function percent(value: number | null) {
  return value === null ? "no data" : `${(value * 100).toFixed(value >= 0.999 && value < 1 ? 2 : 1)}%`;
}

function duration(seconds: number | null) {
  if (seconds === null || seconds === 0) return "none";
  if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
  if (seconds < 86400) return `${(seconds / 3600).toFixed(1)}h`;
  return `${(seconds / 86400).toFixed(1)}d`;
}

/** the month lives in the link, so a report can be sent to someone */
export function Reports({ month: linked }: { month: string | null }) {
  const month = linked && /^\d{4}-\d{2}$/.test(linked) && linked <= currentMonth() ? linked : currentMonth();
  const setMonth = (next: string) => go({ reports: "", month: next });
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setReport(null);
    fetch(`api/reports?month=${month}`)
      .then((response) => (response.ok ? response.json() : Promise.reject(new Error(`${response.status}`))))
      .then(setReport, (e: Error) => setError(e.message));
  }, [month]);

  const measured = report?.flows.filter((f) => f.uptime !== null) ?? [];
  const withTarget = report?.flows.filter((f) => f.met !== null) ?? [];
  const average = measured.length ? measured.reduce((sum, f) => sum + (f.uptime ?? 0), 0) / measured.length : null;
  const owners = [...new Set(report?.flows.map((f) => f.owner ?? "no owner") ?? [])].sort((a, b) =>
    a === "no owner" ? 1 : b === "no owner" ? -1 : a.localeCompare(b),
  );

  return (
    <section className="pt-12">
      <div className="mb-8 flex flex-wrap items-end justify-between gap-4">
        <h2 className="display text-[32px] leading-tight">Reports</h2>
        <div className="flex items-center gap-1 font-mono text-[13px]">
          <span className="mr-3 font-sans">
            <CopyLink link={shareLink({ reports: "", month })} />
          </span>
          <button onClick={() => setMonth(shiftMonth(month, -1))} aria-label="previous month" className="grid size-8 place-items-center rounded-[4px] border border-line text-fog-300 hover:text-fog-100">
            ‹
          </button>
          <span className="w-40 text-center text-fog-100">{monthName(month)}</span>
          <button
            onClick={() => setMonth(shiftMonth(month, 1))}
            disabled={month >= currentMonth()}
            aria-label="next month"
            className="grid size-8 place-items-center rounded-[4px] border border-line text-fog-300 hover:text-fog-100 disabled:opacity-30"
          >
            ›
          </button>
        </div>
      </div>

      {error && <p className="border-l-2 border-red-500 pl-3 font-mono text-sm text-red-300">{error}</p>}

      <div className="mb-10 grid gap-10 border-b border-line pb-8 sm:grid-cols-3">
        <Figure label="average uptime" value={percent(average)} />
        <Figure
          label="sla met"
          value={withTarget.length ? `${withTarget.filter((f) => f.met).length} of ${withTarget.length}` : "no targets"}
          tone={withTarget.some((f) => f.met === false) ? "text-red-400" : undefined}
        />
        <Figure label="incidents" value={String(report?.flows.reduce((sum, f) => sum + f.incidents, 0) ?? 0)} />
      </div>

      {report === null && !error && <p className="text-fog-500">loading</p>}
      {report &&
        owners.map((owner) => (
          <div key={owner} className="mb-8">
            <h3 className="mb-2 text-[14px] font-medium text-fog-300">{owner}</h3>
            <div className="overflow-x-auto rounded-lg border border-line">
              <table className="w-full min-w-[860px] table-fixed border-collapse text-left text-[13px]">
                <thead className="bg-ink-900 text-fog-300">
                  <tr>
                    {["flow", "uptime", "target", "incidents", "longest outage", "avg recovery", "runs ok / failed"].map((label, i) => (
                      <th key={label} className={`h-9 border-b border-line px-3 font-normal ${i === 0 ? "w-56" : ""} ${i > 1 ? "text-right" : ""}`}>
                        {label}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {report.flows
                    .filter((f) => (f.owner ?? "no owner") === owner)
                    .map((flow) => (
                      <tr key={flow.id} className="border-b border-line last:border-b-0">
                        <td className="h-10 truncate px-3 text-fog-100">
                          <a href={`#${new URLSearchParams({ flow: flow.id })}`} className="hover:underline">
                            {flow.id}
                          </a>{" "}
                          <span className="text-fog-500">· {flow.kind}</span>
                        </td>
                        <td className="px-3">
                          <div className="flex items-center gap-3">
                            <span className={`w-16 tabular-nums ${flow.met === false ? "text-red-300" : "text-fog-100"}`}>{percent(flow.uptime)}</span>
                            <span className="h-1 flex-1 bg-ink-800">
                              <span
                                className={`block h-1 ${flow.met === false ? "bg-red-500" : "bg-fog-500"}`}
                                style={{ width: `${(flow.uptime ?? 0) * 100}%` }}
                              />
                            </span>
                          </div>
                        </td>
                        <td className="px-3 text-right tabular-nums">
                          {flow.target === null ? (
                            <span className="text-fog-700">none</span>
                          ) : (
                            <span className={flow.met === false ? "text-red-300" : "text-fog-300"}>
                              {percent(flow.target)} {flow.met === false ? "missed" : flow.met ? "met" : ""}
                            </span>
                          )}
                        </td>
                        <td className="px-3 text-right text-fog-300 tabular-nums">{flow.incidents}</td>
                        <td className="px-3 text-right text-fog-300 tabular-nums">{duration(flow.longest_outage_seconds)}</td>
                        <td className="px-3 text-right text-fog-300 tabular-nums">{duration(flow.mean_recovery_seconds)}</td>
                        <td className="px-3 text-right tabular-nums">
                          <span className="text-fog-300">{flow.runs_ok}</span>
                          <span className="text-fog-700"> / </span>
                          <span className={flow.runs_failed ? "text-red-300" : "text-fog-500"}>{flow.runs_failed}</span>
                        </td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </div>
          </div>
        ))}
    </section>
  );
}

function Figure({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div>
      <p className="text-[14px] text-fog-500">{label}</p>
      <p className={`display mt-2 text-4xl tabular-nums ${tone ?? "text-fog-100"}`}>{value}</p>
    </div>
  );
}
