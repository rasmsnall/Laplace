import { useState, type ReactNode } from "react";

import type { Action } from "./route";

export interface Acknowledged {
  note: string;
  by: string;
  at: string;
}

interface Props {
  flow: string;
  needsAttention: boolean;
  acknowledged: Acknowledged | null;
  silencedUntil: string | null;
  silenceNote: string | null;
  /** asked for by a button in an alert; waits here for a click */
  requested: Action | null;
  onRequestDone: () => void;
  onChange: () => void;
}

const SILENCE_FOR = [
  { label: "1h", hours: 1 },
  { label: "4h", hours: 4 },
  { label: "1 day", hours: 24 },
  { label: "1 week", hours: 168 },
];

async function send(url: string, method: "POST" | "DELETE", body?: unknown) {
  const response = await fetch(url, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok) throw new Error(`${response.status}`);
}

/** acknowledging says someone is on it; silencing stops alerts for a while */
export function FlowActions({
  flow,
  needsAttention,
  acknowledged,
  silencedUntil,
  silenceNote,
  requested,
  onRequestDone,
  onChange,
}: Props) {
  const [note, setNote] = useState("");
  const [error, setError] = useState(false);
  const base = `api/flows/${encodeURIComponent(flow)}`;

  const act = async (url: string, method: "POST" | "DELETE", body?: unknown) => {
    try {
      await send(url, method, body);
      setNote("");
      setError(false);
      onChange();
      onRequestDone();
    } catch {
      setError(true);
    }
  };

  const pending =
    requested === "acknowledge" ? (needsAttention && !acknowledged ? requested : null) : requested === "silence" && !silencedUntil ? requested : null;

  return (
    <div className="mb-3 space-y-2 text-[13px]" onClick={(e) => e.stopPropagation()}>
      {pending && (
        <div className="flex flex-wrap items-center gap-3 rounded-md border border-gold-500/60 px-3 py-1.5">
          <span className="text-fog-300">
            from an alert: {pending === "acknowledge" ? "acknowledge" : "silence for 1h"} <span className="text-fog-100">{flow}</span>?
          </span>
          <SmallButton
            onClick={() =>
              pending === "acknowledge"
                ? act(`${base}/acknowledge`, "POST", { note })
                : act(`${base}/silence`, "POST", { hours: 1, note })
            }
          >
            {pending === "acknowledge" ? "acknowledge" : "silence 1h"}
          </SmallButton>
          <SmallButton onClick={onRequestDone}>not now</SmallButton>
        </div>
      )}
      {acknowledged && (
        <Line>
          <span className="text-fog-300">
            acknowledged by <span className="text-fog-100">{acknowledged.by}</span>{" "}
            {new Date(acknowledged.at).toLocaleString("en-GB")}
            {acknowledged.note && <span className="text-fog-500">: {acknowledged.note}</span>}
          </span>
          <SmallButton onClick={() => act(`${base}/acknowledge`, "DELETE")}>clear</SmallButton>
        </Line>
      )}
      {silencedUntil && (
        <Line>
          <span className="text-fog-300">
            silenced until <span className="text-fog-100">{new Date(silencedUntil).toLocaleString("en-GB")}</span>
            {silenceNote && <span className="text-fog-500">: {silenceNote}</span>}
          </span>
          <SmallButton onClick={() => act(`${base}/silence`, "DELETE")}>lift</SmallButton>
        </Line>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <input
          value={note}
          onChange={(e) => setNote(e.target.value)}
          placeholder="note, e.g. partner delayed until 14:00"
          aria-label="note"
          className="h-7 w-72 rounded-[4px] border border-line bg-ink-950 px-2 text-fog-100 outline-none placeholder:text-fog-700 focus-visible:border-gold-500"
        />
        {needsAttention && !acknowledged && pending !== "acknowledge" && (
          <SmallButton onClick={() => act(`${base}/acknowledge`, "POST", { note })}>acknowledge</SmallButton>
        )}
        <span className="text-fog-500">silence</span>
        {SILENCE_FOR.map(({ label, hours }) => (
          <SmallButton key={label} onClick={() => act(`${base}/silence`, "POST", { hours, note })}>
            {label}
          </SmallButton>
        ))}
        {error && <span className="text-red-300">that did not work</span>}
      </div>
    </div>
  );
}

function Line({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap items-center gap-3 rounded-md border border-line px-3 py-1.5">{children}</div>;
}

function SmallButton({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button onClick={onClick} className="h-7 rounded-[4px] border border-line px-2.5 text-fog-300 hover:border-fog-700 hover:text-fog-100">
      {children}
    </button>
  );
}
