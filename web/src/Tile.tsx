import { Icon } from "./icons";
import { useCountUp } from "./Motion";

/** solid blocks like deepbook's stat row; ink and paper swap with the theme */
const tileTone = {
  gold: "bg-gold-500 text-black",
  ink: "bg-ink-950 text-fog-100",
  slate: "bg-slate-500 text-white",
  paper: "bg-fog-100 text-ink-950",
};

interface TileProps {
  /** numbers count up when they change; text such as "91.3%" is shown as it is */
  value: number | string;
  label: string;
  tone: keyof typeof tileTone;
  alert?: boolean;
  /** the tile's filter or sort is the one in use */
  active?: boolean;
  onClick: () => void;
}

/** a stat block that does something to the table below it when clicked */
export function Tile({ value, label, tone, alert, active, onClick }: TileProps) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      className={`corners group relative flex min-h-44 flex-col justify-between p-5 text-left outline-none focus-visible:ring-2 focus-visible:ring-gold-500 focus-visible:ring-inset ${tileTone[tone]}`}
    >
      <span className={`display text-[clamp(2.25rem,4.5vw,4rem)] leading-none tabular-nums ${alert ? "text-red-400" : ""}`}>
        {typeof value === "number" ? <Count value={value} /> : value}
      </span>
      <span className="flex items-center gap-1.5 text-[14px] font-medium">
        {label}
        <Icon
          name="arrow"
          className={`size-3.5 transition group-hover:translate-x-0 group-hover:opacity-100 group-focus-visible:opacity-100 ${
            active ? "translate-x-0 opacity-100" : "-translate-x-1 opacity-0"
          }`}
        />
      </span>
      {active && <span className="absolute inset-x-0 bottom-0 h-1 bg-current opacity-60" aria-hidden />}
    </button>
  );
}

function Count({ value }: { value: number }) {
  return <>{useCountUp(value)}</>;
}
