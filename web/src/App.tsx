import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import { Connect } from "./Connect";
import { FlowActions, type Acknowledged } from "./FlowActions";
import { CopyLink } from "./CopyLink";
import { Reveal, RiseWords, TickField } from "./Motion";
import { Tile } from "./Tile";
import { go, shareLink, useRoute, type View } from "./route";
import { Pipelines } from "./Pipelines";
import { Reports } from "./Reports";
import { Settings } from "./Settings";
import { Icon, type IconName } from "./icons";

type State = "ok" | "warning" | "late" | "failed" | "pending" | "blocked" | "maintenance";

interface Flow {
  id: string;
  kind: string;
  every: string;
  source: string;
  /** a page a browser can open for the source: the api, the databricks job, the bucket */
  link: string | null;
  state: State;
  detail: string;
  last_ok: string | null;
  host_key_pending: boolean;
  after: string[];
  owner: string | null;
  acknowledged: Acknowledged | null;
  silenced_until: string | null;
  silence_note: string | null;
}

interface HistoryEntry {
  at: string;
  outcome: "start" | "ok" | "fail";
  detail: string;
  millis: number | null;
  metrics: Record<string, number> | null;
}

interface Mark {
  start: string;
  end: string | null;
  outcome: "ok" | "fail" | "running" | "unfinished";
  detail: string;
  millis: number | null;
  metrics: Record<string, number> | null;
}

interface Timeline {
  from: string;
  to: string;
  flows: Record<string, Mark[]>;
}

interface Tip {
  x: number;
  y: number;
  flow: string;
  mark: Mark;
}

/** dates stay english whatever language the browser is set to */
const LOCALE = "en-GB";
const REFRESH_MS = 15_000;
const PANE_OPEN = 520;
/** narrower than this the timeline is unreadable, so it closes instead */
const PANE_MIN = 320;
const MIN_LABEL_GAP = 48;
const BELOW_OPEN = 300;
/** a docked-below timeline lower than this closes, like the side pane */
const BELOW_MIN = 160;
const HANDLE = 12;
const NOW_LABEL_SPACE = 64;
const severity: Record<State, number> = { failed: 0, late: 1, warning: 2, blocked: 3, maintenance: 4, pending: 5, ok: 6 };

async function get<T>(url: string): Promise<T> {
  const response = await fetch(url);
  // the session ran out; loading the page again goes through sign-in
  if (response.status === 401) window.location.reload();
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  return response.json();
}

function useTimeline(hours: number, enabled: boolean) {
  const [timeline, setTimeline] = useState<Timeline | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    const load = () =>
      get<Timeline>(`api/timeline?hours=${hours}`)
        .then((data) => active && setTimeline(data))
        .catch(() => {});
    load();
    const timer = setInterval(load, REFRESH_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [hours, enabled]);

  return timeline;
}

type Role = "viewer" | "operator" | "editor" | "admin";
const RANK: Record<Role, number> = { viewer: 0, operator: 1, editor: 2, admin: 3 };

interface Me {
  email: string;
  role: Role;
  sso: boolean;
  /** why flows.toml on disk is not the one in use; only shown to editors */
  config_error: string | null;
}

/** who is signed in and what they may do; the server checks every action again */
function useMe() {
  const [me, setMe] = useState<Me | null>(null);
  useEffect(() => {
    get<Me>("api/me").then(setMe, () => {});
  }, []);
  return {
    me,
    may: (role: Role) => (me ? RANK[me.role] >= RANK[role] : false),
  };
}

function useFlows() {
  const [flows, setFlows] = useState<Flow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [updated, setUpdated] = useState<Date | null>(null);
  const [reloads, setReloads] = useState(0);

  useEffect(() => {
    let active = true;
    const load = () =>
      get<Flow[]>("api/flows")
        .then((data) => {
          if (!active) return;
          setFlows(data);
          setError(null);
          setUpdated(new Date());
        })
        .catch((e: Error) => active && setError(e.message));
    load();
    const timer = setInterval(load, REFRESH_MS);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [reloads]);

  return { flows, error, updated, reload: () => setReloads((n) => n + 1) };
}

export function App() {
  const { flows, error, updated, reload } = useFlows();
  const route = useRoute();
  const { view, flow: open } = route;
  const { me, may } = useMe();
  const [kind, setKind] = useState("all");
  const [query, setQuery] = useState("");
  /** set by clicking a stat tile; narrows the table to that state */
  const [only, setOnly] = useState<{ label: string; test: (flow: Flow) => boolean } | null>(null);
  const [connecting, setConnecting] = useState(false);
  const closeConnect = useCallback(() => setConnecting(false), []);
  const [pane, setPane] = useState(0);
  const [dock, setDock] = useState<"side" | "below">("side");
  const [below, setBelow] = useState(0);
  const sideDock = dock === "side";
  const timelineShown = sideDock ? pane > 0 : below > 0;
  const columns = sideDock ? 10 : 9;
  const [hours, setHours] = useState(24);
  const [tip, setTip] = useState<Tip | null>(null);
  const timeline = useTimeline(hours, timelineShown);
  const range = timeline && { from: Date.parse(timeline.from), to: Date.parse(timeline.to) };

  const startDrag = (event: React.PointerEvent) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = pane;
    follow(event, (e) => setPane(paneWidth(startWidth + startX - e.clientX)));
  };

  const toggleTimeline = () => {
    if (sideDock) setPane((width) => (width ? 0 : paneWidth(PANE_OPEN)));
    else setBelow((height) => (height ? 0 : belowHeight(BELOW_OPEN)));
  };

  const moveTimeline = (next: "side" | "below") => {
    setDock(next);
    setPane(next === "side" ? paneWidth(PANE_OPEN) : 0);
    setBelow(next === "below" ? belowHeight(BELOW_OPEN) : 0);
  };

  const nudge = (event: React.KeyboardEvent) => {
    const grow = { ArrowLeft: 40, ArrowRight: -40 }[event.key];
    if (!grow) return;
    setPane((width) => paneWidth(width === 0 && grow > 0 ? PANE_MIN : width + grow));
  };

  const loaded = flows !== null;
  const all = flows ?? [];
  const kinds = ["all", ...new Set(all.map((f) => f.kind))];
  const visible = all
    .filter((f) => kind === "all" || f.kind === kind)
    .filter((f) => matches(f, query))
    .filter((f) => !only || only.test(f))
    .sort((a, b) => severity[a.state] - severity[b.state] || a.id.localeCompare(b.id));
  const count = (state: State) => all.filter((f) => f.state === state).length;
  const problems = all.filter((f) => needsAttention(f.state) && !f.acknowledged).length;
  const filters = (kind === "all" ? 0 : 1) + (query ? 1 : 0) + (only ? 1 : 0);
  const attention = all.filter((f) => needsAttention(f.state) && !f.acknowledged);

  // a flow opened from a pipeline, a report or a link is brought into view, and filters that
  // would hide it are cleared first
  useEffect(() => {
    if (!open || !loaded) return;
    if (!visible.some((f) => f.id === open) && all.some((f) => f.id === open)) {
      setKind("all");
      setQuery("");
      setOnly(null);
    }
    const frame = requestAnimationFrame(() =>
      document.getElementById(`flow-${open}`)?.scrollIntoView({ block: "center" }),
    );
    return () => cancelAnimationFrame(frame);
    // only when a different flow is opened, not on every refresh
  }, [open, loaded]);

  const showOnly = (label: string, test: (flow: Flow) => boolean) => {
    if (only?.label === label) return setOnly(null);
    setOnly({ label, test });
    const smooth = !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    document.getElementById("flows")?.scrollIntoView({ behavior: smooth ? "smooth" : "auto", block: "start" });
  };

  return (
    <div className="relative isolate min-h-screen">
      {view === "overview" && <TickField marks={attention.map((f) => ({ id: f.id, failed: f.state === "failed" }))} />}

      <header className="sticky top-0 z-40 bg-ink-950 pt-3 pb-1">
        <div className="mx-auto max-w-6xl px-5 sm:px-10">
        <div className="relative flex h-12 items-center gap-3 overflow-hidden rounded-md border border-line bg-ink-850 pr-1.5 pl-4">
          <h1 className="flex shrink-0 items-center gap-2.5">
            <Logo />
            <span className="display text-[17px]">laplace</span>
          </h1>
          <nav className="mr-auto ml-3 flex gap-0.5 text-[14px]">
            {(["overview", "reports", ...(may("admin") ? ["settings" as const] : [])] as View[]).map((page) => (
              <a
                key={page}
                href={page === "overview" ? "#" : `#${page}`}
                aria-current={view === page ? "page" : undefined}
                className={`rounded-[4px] px-2.5 py-1 font-medium ${view === page ? "text-fog-100" : "text-fog-500 hover:text-fog-100"}`}
              >
                {page}
              </a>
            ))}
          </nav>
          <Clock />
          <span className={`hidden items-center gap-2 text-[13px] font-medium whitespace-nowrap md:flex ${problems ? "text-gold-400" : "text-fog-300"}`}>
            <span className={`size-2 ${problems ? "bg-gold-500" : "bg-fog-500"}`} aria-hidden />
            {problems ? `${problems} need attention` : "all clear"}
          </span>
          {me?.sso && (
            <span className="hidden items-center gap-2 text-[12px] whitespace-nowrap text-fog-500 lg:flex">
              {me.email} <span className="text-fog-700">· {me.role}</span>
              <a href="auth/logout" className="text-fog-300 underline decoration-fog-700 underline-offset-4 hover:text-fog-100">
                sign out
              </a>
            </span>
          )}
          <ThemeToggle />
          {may("editor") && (
            <button onClick={() => setConnecting(true)} className="group flex h-9 shrink-0 items-stretch gap-0.5 text-[14px] font-medium text-black">
              <span className="flex items-center rounded-l-[4px] bg-gold-500 px-3 transition group-hover:bg-gold-400">connect</span>
              <span className="grid w-9 place-items-center rounded-r-[4px] bg-gold-500 transition group-hover:bg-gold-400">
                <Icon name="arrow" className="size-3.5 transition group-hover:translate-x-0.5" />
              </span>
            </button>
          )}
          {updated && <div key={updated.getTime()} className="refresh-bar absolute bottom-0 left-0 h-px bg-gold-500/70" />}
        </div>
        </div>
      </header>

      <main className="mx-auto max-w-6xl px-5 pb-28 sm:px-10">
        {view === "reports" ? (
          <Reports month={route.month} />
        ) : view === "settings" && may("admin") ? (
          <Settings tab={route.tab} />
        ) : (
        <>
        {error && <p className="mt-8 border-l-2 border-red-500 pl-3 font-mono text-sm text-red-300">{error}</p>}
        {me?.config_error && (
          <p className="mt-8 border-l-2 border-red-500 pl-3 font-mono text-[12px] whitespace-pre-wrap text-red-300">{me.config_error}</p>
        )}

        <section className="mt-16">
          <h2 className="display text-[clamp(2.5rem,6vw,4.5rem)] leading-[1.02]">
            <RiseWords text={problems ? `${problems} ${problems === 1 ? "flow needs" : "flows need"} attention` : "All clear"} />
          </h2>
          <p className="mt-4 text-[17px] text-fog-300">
            {all.length} flows watched. {count("failed")} failed, {count("late")} late, {count("warning")} warning.
          </p>
        </section>

        <section className="mt-10 grid grid-cols-2 border border-line lg:grid-cols-4">
          <Tile tone="gold" value={problems} label="Needs attention" active={only?.label === "needs attention"} onClick={() => showOnly("needs attention", (f) => needsAttention(f.state) && !f.acknowledged)} />
          <Tile tone="ink" value={count("failed")} label="Failed" alert={count("failed") > 0} active={only?.label === "failed"} onClick={() => showOnly("failed", (f) => f.state === "failed")} />
          <Tile tone="slate" value={count("late")} label="Late" active={only?.label === "late"} onClick={() => showOnly("late", (f) => f.state === "late")} />
          <Tile tone="paper" value={count("ok")} label="Ok" active={only?.label === "ok"} onClick={() => showOnly("ok", (f) => f.state === "ok")} />
        </section>
        <p className="mt-3 font-mono text-[12px] text-fog-500">
          {count("warning")} warning · {count("blocked")} blocked · {count("pending")} pending
          {count("maintenance") > 0 && ` · ${count("maintenance")} in maintenance`}
        </p>

        <Reveal>
          <Pipelines flows={all} />
        </Reveal>

        <Reveal>
        <section id="flows" className="scroll-mt-20 pt-16">
          <div className="mb-4 flex flex-wrap items-end justify-between gap-4">
            <h2 className="display text-[32px] leading-tight">Flows</h2>
            {updated && (
              <p className="flex items-center gap-2 font-mono text-[11px] text-fog-500">
                <span className="live-dot size-1.5 bg-gold-500" aria-hidden />
                live · updated {updated.toLocaleTimeString(LOCALE)}
              </p>
            )}
          </div>

          <div className="overflow-hidden rounded-lg border border-line bg-ink-950">
            <div className="flex h-11 items-center gap-1 overflow-x-auto border-b border-line px-2 text-[13px] whitespace-nowrap">
              <label className="relative shrink-0">
                <span className="sr-only">type</span>
                <select
                  value={kind}
                  onChange={(e) => setKind(e.target.value)}
                  className="h-7 appearance-none rounded-[4px] bg-transparent pr-7 pl-2 font-medium text-fog-100 outline-none hover:bg-ink-850 focus-visible:ring-1 focus-visible:ring-gold-500"
                >
                  {kinds.map((k) => (
                    <option key={k} value={k}>
                      {k === "all" ? "all types" : k}
                    </option>
                  ))}
                </select>
                <Icon name="chevron" className="pointer-events-none absolute top-1/2 right-2 size-3 -translate-y-1/2 text-fog-500" />
              </label>
              <span className="mx-1 h-4 w-px shrink-0 bg-line" aria-hidden />
              <ToolbarItem icon="rows">
                {visible.length}/{all.length} rows
              </ToolbarItem>
              <ToolbarItem icon="filter">{filters ? `${filters} filter${filters > 1 ? "s" : ""}` : "no filters"}</ToolbarItem>
              {only && (
                <button
                  onClick={() => setOnly(null)}
                  aria-label={`show all states, not only ${only.label}`}
                  className="flex h-7 shrink-0 items-center gap-1.5 rounded-[4px] bg-gold-500 px-2 font-medium text-black hover:bg-gold-400"
                >
                  {only.label}
                  <Icon name="close" className="size-3" />
                </button>
              )}
              <ToolbarItem icon="sort">status</ToolbarItem>
              <button
                onClick={toggleTimeline}
                aria-pressed={timelineShown}
                className={`flex h-7 shrink-0 items-center gap-1.5 rounded-[4px] px-2 transition ${
                  timelineShown ? "bg-ink-850 text-fog-100" : "text-fog-300 hover:bg-ink-850"
                }`}
              >
                <Icon name="gantt" className="size-3.5 text-fog-500" />
                timeline
              </button>
              {timelineShown && (
                <div className="flex shrink-0 rounded-[4px] border border-line">
                  {(["side", "below"] as const).map((place) => (
                    <button
                      key={place}
                      onClick={() => moveTimeline(place)}
                      aria-label={`timeline ${place === "side" ? "beside" : "below"} the table`}
                      aria-pressed={dock === place}
                      className={`grid h-6 w-7 place-items-center ${
                        dock === place ? "bg-fog-100 text-ink-950" : "text-fog-500 hover:bg-ink-850"
                      }`}
                    >
                      <Icon name={place === "side" ? "dockSide" : "dockBelow"} className="size-3.5" />
                    </button>
                  ))}
                </div>
              )}
              {timelineShown && (
                <div className="flex shrink-0 rounded-[4px] border border-line text-[12px]">
                  {[24, 168].map((h) => (
                    <button
                      key={h}
                      onClick={() => setHours(h)}
                      className={`px-2 py-0.5 ${h === hours ? "bg-fog-100 text-ink-950" : "text-fog-300 hover:bg-ink-850"}`}
                    >
                      {h === 24 ? "24h" : "7d"}
                    </button>
                  ))}
                </div>
              )}
              <label className="relative ml-auto shrink-0">
                <span className="sr-only">search flows</span>
                <Icon name="search" className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-fog-500" />
                <input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="search"
                  className="h-7 w-44 rounded-[4px] border border-line bg-ink-900 pr-2 pl-7 text-fog-100 outline-none placeholder:text-fog-700 focus-visible:border-gold-500"
                />
              </label>
            </div>

            <div className="overflow-x-auto">
              <table
                className="w-full table-fixed border-collapse text-left text-[13px]"
                style={{ minWidth: 1400 + (sideDock ? Math.max(pane, HANDLE) : 0) }}
              >
                <thead>
                  <tr className="text-fog-300">
                    <th className="sticky left-0 z-10 h-9 w-10 border-r border-b border-line bg-ink-900" aria-label="row" />
                    <Th icon="rows" className="sticky left-10 z-10 w-44 sm:w-60">flow</Th>
                    <Th icon="status" className="w-32">status</Th>
                    <Th icon="tag" className="w-32">type</Th>
                    <Th icon="user" className="w-28">owner</Th>
                    <Th icon="link" className="w-64">source</Th>
                    <Th icon="clock" className="w-28" align="right">last ok</Th>
                    <Th icon="repeat" className="w-56">schedule</Th>
                    <Th icon="alert">issue</Th>
                    {sideDock && (
                    <th
                      className="sticky right-0 z-20 h-9 border-b border-l border-line bg-ink-900 p-0"
                      style={{ width: Math.max(pane, HANDLE) }}
                    >
                      <div
                        role="separator"
                        aria-orientation="vertical"
                        aria-label="drag to resize the timeline"
                        aria-valuenow={Math.round(pane)}
                        tabIndex={0}
                        onPointerDown={startDrag}
                        onKeyDown={nudge}
                        className="absolute inset-y-0 left-0 z-10 w-3 -translate-x-1/2 cursor-col-resize touch-none after:absolute after:inset-y-2 after:left-1/2 after:w-px after:bg-fog-700 hover:after:bg-gold-500 focus-visible:after:bg-gold-500"
                      />
                      {pane > 0 && range && <Axis from={range.from} to={range.to} width={pane} />}
                    </th>
                    )}
                  </tr>
                </thead>
                <tbody>
                  {flows === null && (
                    <tr>
                      <td colSpan={columns} className="h-10 border-b border-line px-3 text-fog-500">
                        loading
                      </td>
                    </tr>
                  )}
                  {visible.map((flow, index) => (
                    <Fragment key={flow.id}>
                      <tr
                        id={`flow-${flow.id}`}
                        onClick={() => go(open === flow.id ? {} : { flow: flow.id })}
                        className="group cursor-pointer"
                        aria-expanded={open === flow.id}
                      >
                        <td className="sticky left-0 z-10 h-9 border-r border-b border-line bg-ink-950 text-center text-[12px] text-fog-500 tabular-nums group-hover:bg-ink-850">
                          {index + 1}
                        </td>
                        <Td className="sticky left-10 z-10 bg-ink-950">
                          <span className="flex items-center gap-2.5">
                            <span className="grid size-[18px] shrink-0 place-items-center rounded-[4px] border border-ink-600 bg-ink-800 text-fog-300">
                              <Icon name={kindIcon[flow.kind] ?? "rows"} className="size-3" />
                            </span>
                            <a
                              href={`#${new URLSearchParams(open === flow.id ? {} : { flow: flow.id })}`}
                              onClick={(e) => e.stopPropagation()}
                              aria-expanded={open === flow.id}
                              className="truncate font-medium text-fog-100 outline-none focus-visible:underline focus-visible:decoration-gold-500"
                            >
                              {flow.id}
                            </a>
                          </span>
                        </Td>
                        <Td>
                          <span className="inline-flex items-center gap-1.5">
                            <Chip className={stateChip[flow.state]}>{flow.state}</Chip>
                            {flow.acknowledged && (
                              <span title={`acknowledged by ${flow.acknowledged.by}`} className="text-fog-500">
                                <Icon name="check" className="size-3.5" />
                              </span>
                            )}
                            {flow.silenced_until && (
                              <span title={`silenced until ${new Date(flow.silenced_until).toLocaleString(LOCALE)}`} className="text-fog-500">
                                <Icon name="bellOff" className="size-3.5" />
                              </span>
                            )}
                          </span>
                        </Td>
                        <Td>
                          <Chip className="bg-ink-800 text-fog-300">{flow.kind}</Chip>
                        </Td>
                        <Td>{flow.owner ? <span className="text-fog-300">{flow.owner}</span> : <span className="text-fog-700">none</span>}</Td>
                        <Td title={flow.source}>
                          {flow.link ? (
                            <a
                              href={flow.link}
                              target="_blank"
                              rel="noopener noreferrer"
                              onClick={(e) => e.stopPropagation()}
                              className="inline-flex max-w-full items-center gap-1 text-fog-300 underline decoration-fog-700 underline-offset-4 hover:text-fog-100 hover:decoration-gold-500"
                            >
                              <span className="truncate">{flow.source}</span>
                              <Icon name="external" className="size-3 shrink-0 text-fog-500" />
                            </a>
                          ) : (
                            <span className="text-fog-300">{flow.source}</span>
                          )}
                        </Td>
                        <Td align="right" title={flow.last_ok ?? undefined}>
                          {flow.last_ok ? (
                            <span className="text-fog-100 tabular-nums">{age(flow.last_ok)} ago</span>
                          ) : (
                            <span className="text-fog-700">never</span>
                          )}
                        </Td>
                        <Td title={flow.every}>
                          <span className="text-fog-300">{flow.every}</span>
                        </Td>
                        <Td>
                          {issueTone[flow.state] ? (
                            <span className={issueTone[flow.state]}>{flow.detail}</span>
                          ) : (
                            <span className="inline-flex items-center gap-1.5 text-fog-700">
                              <Icon name="minus" className="size-3" />
                              no issue
                            </span>
                          )}
                        </Td>
                        {sideDock && (
                        <td
                          className="sticky right-0 z-10 h-9 border-b border-l border-line bg-ink-950 p-0 group-hover:bg-ink-850"
                          onClick={(e) => pane > 0 && e.stopPropagation()}
                        >
                          {pane > 0 && range && (
                            <Lane
                              marks={timeline?.flows[flow.id] ?? []}
                              from={range.from}
                              to={range.to}
                              width={pane}
                              onHover={(mark, x, y) => setTip(mark ? { x, y, flow: flow.id, mark } : null)}
                            />
                          )}
                        </td>
                        )}
                      </tr>
                      {open === flow.id && (
                        <tr>
                          <td colSpan={columns} className="border-b border-line bg-ink-900 px-4 py-3">
                            {flow.host_key_pending && may("operator") && <TrustHostKey flow={flow.id} />}
                            <div className="mb-3 flex items-center gap-3 text-[13px]" onClick={(e) => e.stopPropagation()}>
                              <CopyLink link={shareLink({ flow: flow.id })} />
                              {route.action && !may("operator") && (
                                <span className="text-fog-500">the alert asked to {route.action}; that needs operator rights</span>
                              )}
                            </div>
                            {may("operator") && (
                            <FlowActions
                              flow={flow.id}
                              needsAttention={needsAttention(flow.state)}
                              acknowledged={flow.acknowledged}
                              silencedUntil={flow.silenced_until}
                              silenceNote={flow.silence_note}
                              requested={route.action}
                              onRequestDone={() => go({ flow: flow.id, at: route.at }, true)}
                              onChange={reload}
                            />
                            )}
                            <History flow={flow.id} at={route.at} refreshed={updated?.getTime() ?? 0} />
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  ))}
                </tbody>
                <tfoot className="text-[12px] text-fog-500">
                  <tr>
                    <td className="sticky left-0 z-10 border-r border-line bg-ink-950" />
                    <td className="sticky left-10 z-10 h-9 border-r border-line bg-ink-950 px-3">
                      <span className="text-fog-100 tabular-nums">{visible.length}</span> count
                    </td>
                    <td className="border-r border-line px-3">
                      <span className={problems ? "text-gold-300" : "text-fog-100"}>{problems}</span> failing
                    </td>
                    <td className="border-r border-line" />
                    <td className="border-r border-line" />
                    <td className="border-r border-line" />
                    <td className="border-r border-line" />
                    <td className="border-r border-line" />
                    <td />
                    {sideDock && <td className="sticky right-0 z-10 border-l border-line bg-ink-950" />}
                  </tr>
                </tfoot>
              </table>
            </div>
          </div>

          {!sideDock && (
            <BelowPanel
              height={below}
              onResize={setBelow}
              flows={visible}
              timeline={timeline}
              onHover={(flow, mark, x, y) => setTip(mark ? { x, y, flow, mark } : null)}
            />
          )}
        </section>
        </Reveal>
        </>
        )}
      </main>
      {tip && timelineShown && <TipCard tip={tip} />}
      {connecting && <Connect onClose={closeConnect} />}
    </div>
  );
}

function belowHeight(wanted: number) {
  const height = Math.min(wanted, window.innerHeight * 0.75);
  return height < BELOW_MIN ? 0 : height;
}

function BelowPanel({
  height,
  onResize,
  flows,
  timeline,
  onHover,
}: {
  height: number;
  onResize: (height: number) => void;
  flows: Flow[];
  timeline: Timeline | null;
  onHover: (flow: string, mark: Mark | null, x: number, y: number) => void;
}) {
  const laneRef = useRef<HTMLDivElement>(null);
  const [laneWidth, setLaneWidth] = useState(0);
  const range = timeline && { from: Date.parse(timeline.from), to: Date.parse(timeline.to) };
  const open = height > 0;

  useEffect(() => {
    const lane = laneRef.current;
    if (!lane) return;
    const observer = new ResizeObserver(([entry]) => setLaneWidth(entry.contentRect.width));
    observer.observe(lane);
    return () => observer.disconnect();
  }, [open]);

  const startDrag = (event: React.PointerEvent) => {
    event.preventDefault();
    const startY = event.clientY;
    follow(event, (e) => onResize(belowHeight(height + startY - e.clientY)));
  };

  const nudge = (event: React.KeyboardEvent) => {
    const grow = { ArrowUp: 40, ArrowDown: -40 }[event.key];
    if (grow) onResize(belowHeight(height === 0 && grow > 0 ? BELOW_MIN : height + grow));
  };

  return (
    <div className="mt-2">
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="drag to resize the timeline"
        aria-valuenow={Math.round(height)}
        tabIndex={0}
        onPointerDown={startDrag}
        onKeyDown={nudge}
        className="group flex h-4 cursor-row-resize touch-none items-center justify-center"
      >
        <span className="h-1 w-12 rounded-full bg-ink-600 transition group-hover:bg-gold-500 group-focus-visible:bg-gold-500" />
      </div>
      {open && (
        <div className="overflow-hidden rounded-lg border border-line bg-ink-950" style={{ height }}>
          <div className="h-full overflow-y-auto">
            <div className="grid grid-cols-[140px_1fr] text-[13px] sm:grid-cols-[220px_1fr]">
              <div className="sticky top-0 z-10 flex h-9 items-center gap-1.5 border-r border-b border-line bg-ink-900 px-3 text-fog-300">
                <Icon name="gantt" className="size-3.5 text-fog-500" />
                timeline
              </div>
              <div ref={laneRef} className="sticky top-0 z-10 h-9 border-b border-line bg-ink-900">
                {range && laneWidth > 0 && <Axis from={range.from} to={range.to} width={laneWidth} />}
              </div>
              {flows.map((flow) => (
                <Fragment key={flow.id}>
                  <div className="flex h-9 items-center gap-2 truncate border-r border-b border-line px-3">
                    <Icon name={kindIcon[flow.kind] ?? "rows"} className="size-3 shrink-0 text-fog-500" />
                    <span className="truncate text-fog-100">{flow.id}</span>
                  </div>
                  <div className="h-9 border-b border-line">
                    {range && laneWidth > 0 && (
                      <Lane
                        marks={timeline?.flows[flow.id] ?? []}
                        from={range.from}
                        to={range.to}
                        width={laneWidth}
                        onHover={(mark, x, y) => onHover(flow.id, mark, x, y)}
                      />
                    )}
                  </div>
                </Fragment>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * follows the pointer until it is let go. capturing it means the release arrives even when it
 * happens outside the browser window, so a drag can never be left running.
 */
function follow(event: React.PointerEvent, move: (e: PointerEvent) => void) {
  const handle = event.currentTarget as HTMLElement;
  handle.setPointerCapture(event.pointerId);
  const stop = () => {
    handle.removeEventListener("pointermove", move);
    handle.removeEventListener("lostpointercapture", stop);
  };
  handle.addEventListener("pointermove", move);
  handle.addEventListener("lostpointercapture", stop);
}

function paneWidth(wanted: number) {
  const width = Math.min(wanted, window.innerWidth * 0.7);
  return width < PANE_MIN ? 0 : width;
}

function ticks(from: number, to: number) {
  const step = to - from > 2 * 86_400_000 ? 86_400_000 : 6 * 3_600_000;
  const offset = new Date(from).getTimezoneOffset() * 60_000;
  const first = Math.ceil((from - offset) / step) * step + offset;
  const result: number[] = [];
  for (let t = first; t < to; t += step) result.push(t);
  return { step, times: result };
}

function Axis({ from, to, width }: { from: number; to: number; width: number }) {
  const { step, times } = ticks(from, to);
  const x = (t: number) => ((t - from) / (to - from)) * width;
  const labelEvery = Math.ceil(MIN_LABEL_GAP / ((step / (to - from)) * width));
  const label = (t: number) => {
    const date = new Date(t);
    if (step >= 86_400_000 || (date.getHours() === 0 && date.getMinutes() === 0)) {
      return date.toLocaleDateString(LOCALE, { weekday: "short", day: "numeric" }).toLowerCase();
    }
    return date.toLocaleTimeString(LOCALE, { hour: "2-digit", minute: "2-digit" });
  };
  return (
    <svg width={width} height={36} className="block overflow-hidden font-mono text-[10px]">
      {times.map((t, i) => (
        <g key={t}>
          <line x1={x(t)} x2={x(t)} y1={24} y2={36} className="stroke-line" />
          {i % labelEvery === 0 && x(t) < width - NOW_LABEL_SPACE && (
            <text x={x(t) + 4} y={22} className="fill-fog-500">
              {label(t)}
            </text>
          )}
        </g>
      ))}
      <text x={width - 6} y={22} textAnchor="end" className="fill-gold-400">
        now
      </text>
    </svg>
  );
}

const barClass: Record<Mark["outcome"], string> = {
  ok: "fill-fog-500",
  fail: "fill-red-500",
  running: "fill-gold-500",
  unfinished: "fill-none stroke-gold-500 [stroke-dasharray:3_2]",
};

function Lane({
  marks,
  from,
  to,
  width,
  onHover,
}: {
  marks: Mark[];
  from: number;
  to: number;
  width: number;
  onHover: (mark: Mark | null, x: number, y: number) => void;
}) {
  const x = (t: number) => ((t - from) / (to - from)) * width;
  const hover = (mark: Mark) => ({
    onMouseEnter: (e: React.MouseEvent) => onHover(mark, e.clientX, e.clientY),
    onMouseMove: (e: React.MouseEvent) => onHover(mark, e.clientX, e.clientY),
    onMouseLeave: () => onHover(null, 0, 0),
  });

  return (
    <svg width={width} height={36} className="block overflow-hidden">
      {ticks(from, to).times.map((t) => (
        <line key={t} x1={x(t)} x2={x(t)} y1={0} y2={36} className="stroke-line" />
      ))}
      {marks.filter((mark) => (mark.end ? Date.parse(mark.end) : to) >= from).map((mark, i) => {
        const start = x(Date.parse(mark.start));
        const end = x(mark.end ? Date.parse(mark.end) : to);
        const isPoint = mark.end === mark.start;
        return isPoint ? (
          <g key={i} {...hover(mark)}>
            <rect x={start - 4} y={6} width={8} height={24} className="fill-transparent" />
            <rect
              x={start - 1}
              y={mark.outcome === "fail" ? 8 : 12}
              width={2}
              height={mark.outcome === "fail" ? 20 : 12}
              className={mark.outcome === "fail" ? "fill-red-500" : "fill-fog-700"}
            />
          </g>
        ) : (
          <rect
            key={i}
            x={Math.max(0, start)}
            y={11}
            width={Math.max(3, end - Math.max(0, start))}
            height={14}
            rx={2}
            className={barClass[mark.outcome]}
            {...hover(mark)}
          />
        );
      })}
      <line x1={width - 1} x2={width - 1} y1={0} y2={36} className="stroke-gold-500" />
    </svg>
  );
}

function TipCard({ tip }: { tip: Tip }) {
  const { mark } = tip;
  const isRun = mark.end !== mark.start;
  const left = tip.x > window.innerWidth - 320 ? tip.x - 316 : tip.x + 14;
  const rows: [string, string][] = [
    ["start", new Date(mark.start).toLocaleString(LOCALE)],
    ...(isRun ? [["end", mark.end ? new Date(mark.end).toLocaleString(LOCALE) : "still running"] as [string, string]] : []),
    ...(mark.millis !== null ? [[isRun ? "duration" : "latency", formatMillis(mark.millis)] as [string, string]] : []),
    ["detail", mark.detail || "none"],
    ...Object.entries(mark.metrics ?? {}).map(([name, value]) => [name, number.format(value)] as [string, string]),
  ];

  return (
    <div
      className="pointer-events-none fixed z-50 w-[300px] rounded-md border border-line bg-ink-900 p-3 font-mono text-[12px] shadow-2xl"
      style={{ left, top: tip.y + 14 }}
    >
      <div className="mb-2 flex items-center justify-between gap-2">
        <span className="truncate text-fog-100">{tip.flow}</span>
        <span className={mark.outcome === "fail" ? "text-red-300" : mark.outcome === "ok" ? "text-fog-300" : "text-gold-300"}>
          {mark.outcome}
        </span>
      </div>
      <dl className="grid grid-cols-[72px_1fr] gap-x-2 gap-y-1">
        {rows.map(([key, value]) => (
          <Fragment key={key}>
            <dt className="text-fog-500">{key}</dt>
            <dd className="break-words text-fog-300">{value}</dd>
          </Fragment>
        ))}
      </dl>
    </div>
  );
}

const number = new Intl.NumberFormat(LOCALE, { maximumFractionDigits: 2 });

function formatMetrics(metrics: Record<string, number>) {
  return Object.entries(metrics)
    .map(([name, value]) => `${name} ${number.format(value)}`)
    .join(" · ");
}

function formatMillis(millis: number) {
  if (millis < 1000) return `${millis} ms`;
  if (millis < 60_000) return `${(millis / 1000).toFixed(1)} s`;
  return `${Math.floor(millis / 60_000)}m ${Math.round((millis % 60_000) / 1000)}s`;
}

function matches(flow: Flow, query: string) {
  const needle = query.trim().toLowerCase();
  return !needle || [flow.id, flow.kind, flow.source].some((field) => field.toLowerCase().includes(needle));
}

function ToolbarItem({ icon, children }: { icon: IconName; children: ReactNode }) {
  return (
    <span className="flex h-7 shrink-0 items-center gap-1.5 rounded-[4px] px-2 text-fog-300">
      <Icon name={icon} className="size-3.5 text-fog-500" />
      {children}
    </span>
  );
}

function ThemeToggle() {
  const [theme, setTheme] = useState(() => document.documentElement.dataset.theme ?? "dark");
  const next = theme === "dark" ? "light" : "dark";

  const toggle = () => {
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem("theme", next);
    } catch {
      // private windows can refuse storage; the theme still applies for this visit
    }
    setTheme(next);
  };

  return (
    <button
      onClick={toggle}
      aria-label={`switch to ${next} mode`}
      className="grid size-8 place-items-center rounded-[4px] border border-ink-600 text-fog-300 transition hover:border-fog-700 hover:text-fog-100"
    >
      <Icon name={theme === "dark" ? "sun" : "moon"} className="size-4" />
    </button>
  );
}

function Clock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);
  return (
    <span className="hidden font-mono text-[11px] whitespace-nowrap text-fog-500 tabular-nums xl:inline">
      {now.toISOString().slice(0, 19).replace("T", " ")} utc
    </span>
  );
}

/** three bars of shrinking width: runs lined up on a timeline */
function Logo() {
  return (
    <svg width="18" height="16" viewBox="0 0 18 16" aria-hidden className="text-gold-500">
      <rect width="18" height="4" rx="1" fill="currentColor" />
      <rect y="6" width="13" height="4" rx="1" fill="currentColor" />
      <rect y="12" width="8" height="4" rx="1" fill="currentColor" />
    </svg>
  );
}

function TrustHostKey({ flow }: { flow: string }) {
  const [result, setResult] = useState<"idle" | "trusted" | "error">("idle");

  const trust = async () => {
    const response = await fetch(`api/flows/${encodeURIComponent(flow)}/trust-host-key`, { method: "POST" });
    setResult(response.ok ? "trusted" : "error");
  };

  return (
    <div className="mb-3 flex flex-wrap items-center gap-3 rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-[13px]">
      <span className="text-red-300">
        the server's host key changed. confirm with the partner that they replaced it before trusting the new one.
      </span>
      {result === "trusted" ? (
        <span className="text-fog-300">trusted, checking again</span>
      ) : (
        <button
          onClick={(e) => {
            e.stopPropagation();
            trust();
          }}
          className="rounded-[4px] border border-red-500/60 px-2.5 py-1 text-red-200 hover:bg-red-500/20"
        >
          trust new host key
        </button>
      )}
      {result === "error" && <span className="text-red-300">could not trust the key</span>}
    </div>
  );
}

function History({ flow, at, refreshed }: { flow: string; at: string | null; refreshed: number }) {
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);
  const marked = useRef<HTMLLIElement>(null);

  useEffect(() => {
    marked.current?.scrollIntoView({ block: "nearest" });
  }, [at, entries === null]);

  useEffect(() => {
    let active = true;
    get<HistoryEntry[]>(`api/flows/${encodeURIComponent(flow)}/history`).then(
      (data) => active && setEntries(data),
      () => active && setEntries((current) => current ?? []),
    );
    return () => {
      active = false;
    };
  }, [flow, refreshed]);

  if (entries === null) return <p className="text-xs text-fog-500">loading history</p>;
  if (entries.length === 0) return <p className="text-xs text-fog-500">nothing recorded yet</p>;

  return (
    <ol className="max-h-72 space-y-1 overflow-y-auto font-mono text-[12px]">
      {entries.map((entry, i) => (
        <li
          key={i}
          ref={sameMoment(entry.at, at) ? marked : undefined}
          className={`flex gap-4 ${sameMoment(entry.at, at) ? "-mx-1 bg-gold-500/10 px-1" : ""}`}
        >
          <a
            href={`#${new URLSearchParams({ flow, at: entry.at })}`}
            onClick={(e) => e.stopPropagation()}
            title="link to this run"
            className="shrink-0 text-fog-500 hover:text-fog-100 hover:underline"
          >
            {new Date(entry.at).toLocaleString(LOCALE)}
          </a>
          <span className={`w-10 shrink-0 ${outcomeColor[entry.outcome]}`}>{entry.outcome}</span>
          <span className="truncate text-fog-300">
            {entry.detail}
            {entry.millis !== null && <span className="text-fog-500"> · {entry.millis} ms</span>}
            {entry.metrics && <span className="text-fog-500"> · {formatMetrics(entry.metrics)}</span>}
          </span>
        </li>
      ))}
    </ol>
  );
}

function sameMoment(a: string, b: string | null) {
  return b !== null && Date.parse(a) === Date.parse(b);
}

const outcomeColor: Record<HistoryEntry["outcome"], string> = {
  fail: "text-red-400",
  ok: "text-gold-400",
  start: "text-fog-500",
};

function needsAttention(state: State) {
  return state === "failed" || state === "late" || state === "warning";
}

/** states that have something to say in the issue column */
const issueTone: Partial<Record<State, string>> = {
  failed: "text-red-300",
  late: "text-gold-300",
  warning: "text-gold-300",
  blocked: "text-fog-500",
  maintenance: "text-fog-500",
};

const stateChip: Record<State, string> = {
  failed: "bg-red-500/15 text-red-300",
  late: "bg-gold-500/15 text-gold-300",
  ok: "bg-ink-800 text-fog-300",
  warning: "border border-gold-500/60 text-gold-300",
  blocked: "border border-dashed border-fog-700 text-fog-500",
  maintenance: "border border-dashed border-gold-500/40 text-fog-300",
  pending: "border border-ink-600 text-fog-500",
};

const kindIcon: Record<string, IconName> = {
  sftp: "transfer",
  storage: "bucket",
  heartbeat: "pulse",
  http: "globe",
  inbound: "inbound",
  databricks: "layers",
  delta: "table",
};

interface CellProps {
  align?: "left" | "right";
  className?: string;
  children?: ReactNode;
}

function Th({ icon, align = "left", className = "", children }: CellProps & { icon: IconName }) {
  return (
    <th className={`h-9 border-r border-b border-line bg-ink-900 px-3 font-normal last:border-r-0 ${className}`}>
      <span className={`flex items-center gap-1.5 ${align === "right" ? "justify-end" : ""}`}>
        <Icon name={icon} className="size-3.5 text-fog-500" />
        {children}
      </span>
    </th>
  );
}

function Td({ align = "left", className = "", title, children }: CellProps & { title?: string }) {
  return (
    <td
      title={title}
      className={`h-9 truncate border-r border-b border-line px-3 last:border-r-0 group-hover:bg-ink-850 ${
        align === "right" ? "text-right" : ""
      } ${className}`}
    >
      {children}
    </td>
  );
}

function Chip({ className, children }: { className: string; children: string }) {
  return <span className={`inline-block rounded-[4px] px-1.5 py-1 text-[12px] leading-none ${className}`}>{children}</span>;
}


function age(iso: string): string {
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 60) return `${Math.floor(seconds)}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86400)}d`;
}
