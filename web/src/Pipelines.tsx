import { Fragment } from "react";

export interface PipelineFlow {
  id: string;
  state: string;
  detail: string;
  after: string[];
}

const mark: Record<string, string> = {
  failed: "bg-red-500",
  late: "bg-gold-400",
  warning: "border border-gold-400",
  blocked: "border border-fog-700 bg-[repeating-linear-gradient(135deg,var(--color-fog-700)_0_1px,transparent_1px_3px)]",
  pending: "border border-fog-700",
  ok: "bg-fog-700",
};

/** flows linked by `after`, grouped into separate pipelines and laid out step by step */
function pipelines(flows: PipelineFlow[]) {
  const byId = new Map(flows.map((flow) => [flow.id, flow]));
  const linked = flows.filter((flow) => flow.after.length > 0 || flows.some((other) => other.after.includes(flow.id)));

  const group = new Map<string, string>();
  const root = (id: string): string => {
    const parent = group.get(id) ?? id;
    return parent === id ? id : root(parent);
  };
  for (const flow of linked) {
    for (const upstream of flow.after) group.set(root(flow.id), root(upstream));
  }

  const depth = new Map<string, number>();
  const depthOf = (id: string): number => {
    if (!depth.has(id)) {
      const upstream = byId.get(id)?.after.filter((a) => byId.has(a)) ?? [];
      depth.set(id, upstream.length ? 1 + Math.max(...upstream.map(depthOf)) : 0);
    }
    return depth.get(id)!;
  };

  const groups = new Map<string, PipelineFlow[][]>();
  for (const flow of linked) {
    const steps = groups.get(root(flow.id)) ?? [];
    (steps[depthOf(flow.id)] ??= []).push(flow);
    groups.set(root(flow.id), steps);
  }
  return [...groups.values()];
}

export function Pipelines({ flows }: { flows: PipelineFlow[] }) {
  const all = pipelines(flows);
  if (all.length === 0) return null;

  return (
    <section className="pt-16">
      <h2 className="wide mb-4 text-2xl font-medium tracking-[0.2em] uppercase">pipelines</h2>
      <div className="space-y-3">
        {all.map((steps) => (
          <div key={steps[0][0].id} className="overflow-x-auto rounded-lg border border-line bg-ink-950 p-4">
            <div className="flex min-w-max items-center gap-3">
              {steps.map((step, i) => (
                <Fragment key={i}>
                  {i > 0 && (
                    <svg width="32" height="8" viewBox="0 0 32 8" className="shrink-0 text-fog-700" aria-hidden>
                      <path d="M0 4h28M24 1l4 3-4 3" fill="none" stroke="currentColor" strokeWidth="1.25" />
                    </svg>
                  )}
                  <div className="flex flex-col gap-2">
                    {step.map((flow) => (
                      <div
                        key={flow.id}
                        title={flow.detail}
                        className={`w-52 rounded-md border px-3 py-2 ${
                          flow.state === "failed" ? "border-red-500/60" : flow.state === "late" ? "border-gold-500/60" : "border-line"
                        }`}
                      >
                        <div className="flex items-center gap-2 text-[13px]">
                          <span className={`size-2 shrink-0 ${mark[flow.state] ?? "bg-fog-700"}`} aria-hidden />
                          <span className="truncate text-fog-100">{flow.id}</span>
                        </div>
                        <div className="mt-1 truncate font-mono text-[11px] text-fog-500">
                          {flow.state}
                          {flow.state !== "ok" && flow.state !== "pending" ? ` · ${flow.detail}` : ""}
                        </div>
                      </div>
                    ))}
                  </div>
                </Fragment>
              ))}
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}
