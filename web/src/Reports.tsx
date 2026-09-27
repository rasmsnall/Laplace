import { Fragment, useEffect, useState } from "react";

import { CopyLink } from "./CopyLink";
import { Icon } from "./icons";
import { Reveal } from "./Motion";
import { go, shareLink } from "./route";
import { Tile } from "./Tile";

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
  /** uptime of each day of the month, null before the flow reported or for days to come */
  days: (number | null)[];
}

interface Report {
  from: string;
  to: string;
  flows: FlowReport[];
}

type SortKey = "flow" | "uptime" | "target" | "incidents" | "outage" | "recovery" | "runs";

const COLUMNS: { key: SortKey; label: string }[] = [
  { key: "flow", label: "flow" },
  { key: "uptime", label: "uptime" },
  { key: "target", label: "target" },
  { key: "incidents", label: "incidents" },
  { key: "outage", label: "longest outage" },
  { key: "recovery", label: "avg recovery" },
  { key: "runs", label: "runs ok / failed" },
];

/** what each column sorts by; flows without data go last either way */
const SORT: Record<SortKey, (flow: FlowReport) => number | string> = {
  flow: (f) => f.id,
  uptime: (f) => f.uptime ?? 2,
  target: (f) => (f.met === false ? 0 : f.met ? 1 : 2),
  incidents: (f) => f.incidents,
  outage: (f) => f.longest_outage_seconds,
  recovery: (f) => f.mean_recovery_seconds ?? -1,
  runs: (f) => f.runs_failed,
};

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
  const [sort, setSort] = useState<{ key: SortKey; descending: boolean }>({ key: "flow", descending: false });
  const [onlyMissed, setOnlyMissed] = useState(false);
  const [open, setOpen] = useState<Set<string>>(new Set());

  useEffect(() => {
    setReport(null);
    setError(null);
    fetch(`api/reports?month=${month}`)
      .then((response) => (response.ok ? response.json() : Promise.reject(new Error(`${response.status}`))))
      .then(setReport, (e: Error) => setError(e.message));
  }, [month]);

  // left and right arrows step through months, unless someone is typing
  useEffect(() => {
    const step = (event: KeyboardEvent) => {
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) return;
      if (event.key === "ArrowLeft") setMonth(shiftMonth(month, -1));
      if (event.key === "ArrowRight" && month < currentMonth()) setMonth(shiftMonth(month, 1));
    };
    window.addEventListener("keydown", step);
    return () => window.removeEventListener("keydown", step);
  });

  const flows = report?.flows ?? [];
  const measured = flows.filter((f) => f.uptime !== null);
  const missed = flows.filter((f) => f.met === false);
  const average = measured.length ? measured.reduce((sum, f) => sum + (f.uptime ?? 0), 0) / measured.length : null;
  const incidents = flows.reduce((sum, f) => sum + f.incidents, 0);
  const longest = Math.max(0, ...flows.map((f) => f.longest_outage_seconds));

  const sortBy = (key: SortKey, descending: boolean) =>
    setSort((current) => (current.key === key ? { key, descending: !current.descending } : { key, descending }));
  const ordered = [...flows]
    .filter((f) => !onlyMissed || f.met === false)
    .sort((a, b) => {
      const [x, y] = [SORT[sort.key](a), SORT[sort.key](b)];
      const order = typeof x === "string" ? x.localeCompare(y as string) : x - (y as number);
      return sort.descending ? -order : order;
    });
  const owners = [...new Set(ordered.map((f) => f.owner ?? "no owner"))].sort((a, b) =>
    a === "no owner" ? 1 : b === "no owner" ? -1 : a.localeCompare(b),
  );
  const toggle = (id: string) =>
    setOpen((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });

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
          <span className="w-40 text-center text-fog-100" aria-live="polite">
            {monthName(month)}
          </span>
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

      {error && <p className="mb-6 border-l-2 border-red-500 pl-3 font-mono text-sm text-red-300">{error}</p>}

      <div className="grid grid-cols-2 border border-line lg:grid-cols-4">
        <Tile tone="gold" value={missed.length} label="SLA missed" active={onlyMissed} onClick={() => setOnlyMissed((on) => !on)} />
        <Tile tone="ink" value={percent(average)} label="Average uptime" active={sort.key === "uptime"} onClick={() => sortBy("uptime", false)} />
        <Tile tone="slate" value={incidents} label="Incidents" active={sort.key === "incidents"} onClick={() => sortBy("incidents", true)} />
        <Tile tone="paper" value={duration(longest)} label="Longest outage" active={sort.key === "outage"} onClick={() => sortBy("outage", true)} />
      </div>
      <p className="mt-3 mb-10 font-mono text-[12px] text-fog-500">
        click a tile to filter or sort · click a flow for its days · ← → change month
      </p>

      {report === null && !error && <p className="text-fog-500">loading</p>}
      {report && ordered.length === 0 && <p className="text-fog-500">no flows missed their target this month</p>}
      {report &&
        owners.map((owner) => (
          <Reveal key={owner} className="mb-8">
            <h3 className="mb-2 text-[14px] font-medium text-fog-300">{owner}</h3>
            <div className="overflow-x-auto rounded-md border border-line">
              <table className="w-full min-w-[860px] table-fixed border-collapse text-left text-[13px]">
                <thead className="bg-ink-900 text-fog-300">
                  <tr>
                    {COLUMNS.map(({ key, label }, i) => (
                      <th
                        key={key}
                        aria-sort={sort.key === key ? (sort.descending ? "descending" : "ascending") : undefined}
                        className={`h-9 border-b border-line p-0 font-normal whitespace-nowrap ${i === 0 ? "w-60" : i === 1 ? "w-56" : ""}`}
                      >
                        <button
                          onClick={() => sortBy(key, key !== "flow" && key !== "uptime" && key !== "target")}
                          className={`flex h-9 w-full items-center gap-1 px-3 hover:text-fog-100 ${i > 1 ? "justify-end" : ""} ${sort.key === key ? "text-fog-100" : ""}`}
                        >
                          {label}
                          <Icon
                            name="chevron"
                            className={`size-3 transition ${sort.key === key ? "opacity-100" : "opacity-0"} ${sort.key === key && !sort.descending ? "rotate-180" : ""}`}
                          />
                        </button>
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {ordered
                    .filter((f) => (f.owner ?? "no owner") === owner)
                    .map((flow) => (
                      <Fragment key={flow.id}>
                        <tr
                          onClick={() => toggle(flow.id)}
                          aria-expanded={open.has(flow.id)}
                          className="cursor-pointer border-b border-line last:border-b-0 hover:bg-ink-850"
                        >
                          <td className="h-10 truncate px-3 text-fog-100">
                            <span className="flex items-center gap-2">
                              <Icon name="chevron" className={`size-3 shrink-0 text-fog-500 transition ${open.has(flow.id) ? "" : "-rotate-90"}`} />
                              <a href={`#${new URLSearchParams({ flow: flow.id })}`} onClick={(e) => e.stopPropagation()} className="truncate hover:underline">
                                {flow.id}
                              </a>
                              <span className="shrink-0 text-fog-500">· {flow.kind}</span>
                            </span>
                          </td>
                          <td className="px-3">
                            <div className="flex items-center gap-3">
                              <span className={`w-16 tabular-nums ${flow.met === false ? "text-red-300" : "text-fog-100"}`}>{percent(flow.uptime)}</span>
                              <UptimeBar uptime={flow.uptime} target={flow.target} missed={flow.met === false} />
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
                        {open.has(flow.id) && (
                          <tr className="border-b border-line bg-ink-900">
                            <td colSpan={COLUMNS.length} className="px-4 py-4">
                              <Days days={flow.days} month={month} target={flow.target} />
                            </td>
                          </tr>
                        )}
                      </Fragment>
                    ))}
                </tbody>
              </table>
            </div>
          </Reveal>
        ))}
    </section>
  );
}

/** grows in from zero; a tick marks the flow's target */
function UptimeBar({ uptime, target, missed }: { uptime: number | null; target: number | null; missed: boolean }) {
  const [grown, setGrown] = useState(false);
  useEffect(() => {
    const frame = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(frame);
  }, []);

  return (
    <span className="relative h-1.5 flex-1 bg-ink-800" title={target === null ? undefined : `target ${percent(target)}`}>
      <span
        className={`block h-1.5 transition-[width] duration-700 ease-out motion-reduce:transition-none ${missed ? "bg-red-500" : "bg-fog-500"}`}
        style={{ width: grown ? `${(uptime ?? 0) * 100}%` : 0 }}
      />
      {target !== null && <span className="absolute -top-1 h-3.5 w-px bg-gold-500" style={{ left: `${target * 100}%` }} aria-hidden />}
    </span>
  );
}

/** a colour for a day: grey when it was all up, gold when it dipped, red below the target or 95% */
function dayTone(uptime: number | null, target: number | null) {
  if (uptime === null) return "border border-line bg-transparent";
  if (uptime >= 0.9995) return "bg-fog-500";
  if (uptime >= (target ?? 0.95)) return "bg-gold-500";
  return uptime >= 0.5 ? "bg-red-500/70" : "bg-red-500";
}

/** the month as a row of day blocks; pointing at one says how that day went */
function Days({ days, month, target }: { days: (number | null)[]; month: string; target: number | null }) {
  const [year, m] = month.split("-").map(Number);
  const label = (day: number) => new Date(year, m - 1, day).toLocaleDateString(LOCALE, { weekday: "short", day: "numeric", month: "short" }).toLowerCase();
  const [pointed, setPointed] = useState<number | null>(null);
  const latest = days.reduce<number>((last, uptime, i) => (uptime === null ? last : i), -1);
  const shown = pointed ?? latest;

  return (
    <div>
      <div className="flex flex-wrap gap-1" onMouseLeave={() => setPointed(null)}>
        {days.map((uptime, i) => (
          <span
            key={i}
            tabIndex={0}
            role="img"
            aria-label={`${label(i + 1)}: ${uptime === null ? "no data" : `${percent(uptime)} up`}`}
            onMouseEnter={() => setPointed(i)}
            onFocus={() => setPointed(i)}
            className={`size-6 rounded-[3px] outline-none transition hover:scale-110 focus-visible:ring-2 focus-visible:ring-gold-500 ${dayTone(uptime, target)} ${
              pointed === i ? "ring-2 ring-fog-100" : ""
            }`}
          />
        ))}
      </div>
      <p className="mt-3 h-4 font-mono text-[12px] text-fog-300">
        {shown >= 0 && (
          <>
            {label(shown + 1)} · {days[shown] === null ? "no data" : `${percent(days[shown])} up`}
          </>
        )}
      </p>
    </div>
  );
}
