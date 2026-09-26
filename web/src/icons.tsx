const iconPaths = {
  table: "M3 5h18v14H3zM3 10h18M3 15h18M9 5v14",
  user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21a8 8 0 0 1 16 0",
  bellOff: "M9 18h6M6 8a6 6 0 0 1 10-4.5M18 8v5l2 3H8M3 3l18 18",
  plug: "M9 2v6M15 2v6M6 8h12v4a6 6 0 0 1-12 0zM12 18v4",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  check: "M5 12l5 5 9-10",
  close: "M6 6l12 12M18 6 6 18",
  rows: "M4 6h16M4 12h16M4 18h16",
  status: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  tag: "M3 12V3h9l9 9-9 9zM7.5 7.5h.01",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7v5l3 2",
  repeat: "M17 2l4 4-4 4M3 11V9a3 3 0 0 1 3-3h15M7 22l-4-4 4-4M21 13v2a3 3 0 0 1-3 3H3",
  alert: "M12 3 2 20h20zM12 10v4M12 17h.01",
  chevron: "M6 9l6 6 6-6",
  sort: "M7 4v16M3 16l4 4 4-4M17 20V4M13 8l4-4 4 4",
  minus: "M5 12h14",
  transfer: "M4 8h15l-4-4M20 16H5l4 4",
  bucket: "M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6",
  pulse: "M3 12h4l2-6 4 12 2-6h6",
  globe: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c2.5 2.5 3.5 5.5 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-5.5-3.5-9s1-6.5 3.5-9z",
  inbound: "M3 12h13M12 8l4 4-4 4M20 4v16",
  layers: "M12 3 3 8l9 5 9-5zM3 13l9 5 9-5",
  link: "M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1",
  dockSide: "M3 4h18v16H3zM15 4v16",
  dockBelow: "M3 4h18v16H3zM3 14h18",
  gantt: "M4 5h9M8 10h12M6 15h8M10 20h8",
  filter: "M3 5h18l-7 8v6l-4 2v-8z",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM21 21l-5-5",
  sun: "M12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4",
  moon: "M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z",
};

export type IconName = keyof typeof iconPaths;

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden
    >
      <path d={iconPaths[name]} />
    </svg>
  );
}
