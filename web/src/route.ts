import { useEffect, useState } from "react";

export type View = "overview" | "reports" | "settings";
export type Action = "acknowledge" | "silence";

/** everything a link can point at: #flow=x&at=...&do=acknowledge, #reports&month=2026-09, #settings */
export interface Route {
  view: View;
  flow: string | null;
  at: string | null;
  action: Action | null;
  month: string | null;
  /** which settings tab is open */
  tab: string | null;
}

function read(): Route {
  const params = new URLSearchParams(window.location.hash.slice(1));
  const action = params.get("do");
  return {
    view: params.has("reports") ? "reports" : params.has("settings") ? "settings" : "overview",
    flow: params.get("flow"),
    at: params.get("at"),
    action: action === "acknowledge" || action === "silence" ? action : null,
    month: params.get("month"),
    tab: params.get("tab"),
  };
}

export function useRoute() {
  const [route, setRoute] = useState(read);
  useEffect(() => {
    const onChange = () => setRoute(read());
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);
  return route;
}

function hash(params: Record<string, string | null | undefined>) {
  const parts = Object.entries(params)
    .filter(([, value]) => value !== null && value !== undefined)
    .map(([key, value]) => (value === "" ? key : `${key}=${encodeURIComponent(value!)}`));
  return parts.join("&");
}

export function go(params: Record<string, string | null | undefined>, replace = false) {
  const next = `#${hash(params)}`;
  if (replace) {
    history.replaceState(null, "", next);
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  } else {
    window.location.hash = next;
  }
}

/** shared links use the query string, which survives the sign-in redirect; a fragment would not */
export function shareLink(params: Record<string, string | null | undefined>) {
  return `${window.location.origin}${window.location.pathname}?${hash(params)}`;
}

/** turns a shared ?flow=x link into the #flow=x the app routes on */
export function adoptSharedLink() {
  if (!window.location.search) return;
  history.replaceState(null, "", `${window.location.pathname}#${window.location.search.slice(1)}`);
}
