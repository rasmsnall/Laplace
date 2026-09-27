import { useEffect, useRef, useState, type ReactNode } from "react";

const REDUCE = "(prefers-reduced-motion: reduce)";
const stillness = () => window.matchMedia(REDUCE).matches;

const GAP_X = 26;
const GAP_Y = 38;
const TICK = 18;
const REACH = 160;

export interface FieldMark {
  id: string;
  failed: boolean;
}

/** stable place in the field for a flow, so its tick does not jump around between refreshes */
function cellOf(id: string, cells: number) {
  let hash = 2166136261;
  for (const char of id) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  return (hash >>> 0) % cells;
}

/**
 * deepbook's drifting field of ticks, behind the headline. each flow that needs attention
 * is one tick that pulses in gold, or red when it failed; the cursor lights up the ticks near it.
 */
export function TickField({ marks }: { marks: FieldMark[] }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const current = useRef(marks);
  current.current = marks;

  useEffect(() => {
    const element = canvas.current;
    const context = element?.getContext("2d");
    if (!element || !context) return;

    let colors = readColors();
    let pointer = { x: -1e4, y: -1e4 };
    let frame = 0;
    let last = 0;

    const size = () => {
      const ratio = window.devicePixelRatio || 1;
      element.width = element.clientWidth * ratio;
      element.height = element.clientHeight * ratio;
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
    };

    const draw = (time: number) => {
      const width = element.clientWidth;
      const height = element.clientHeight;
      const columns = Math.ceil(width / GAP_X);
      const rows = Math.ceil(height / GAP_Y);
      const lit = new Map<number, FieldMark>();
      for (const mark of current.current) lit.set(cellOf(mark.id, columns * rows), mark);

      context.clearRect(0, 0, width, height);
      context.lineWidth = 1.5;
      context.lineCap = "round";
      for (let row = 0; row < rows; row++) {
        for (let column = 0; column < columns; column++) {
          const x = column * GAP_X + (row % 2) * (GAP_X / 2) + 6;
          const wave = (Math.sin(column * 0.32 + row * 0.55 + time * 0.0007) + 1) / 2;
          const y = row * GAP_Y + 8 + wave * 8;
          const near = Math.max(0, 1 - Math.hypot(pointer.x - x, pointer.y - y) / REACH);
          const mark = lit.get(row * columns + column);

          let length = TICK * (0.5 + 0.5 * wave) + near * 12;
          let alpha = 0.28 + 0.42 * wave + near * 0.6;
          context.strokeStyle = colors.tick;
          if (mark) {
            const pulse = (Math.sin(time * 0.004 + column) + 1) / 2;
            length = TICK * 1.5 + pulse * 6;
            alpha = 0.65 + 0.35 * pulse;
            context.strokeStyle = mark.failed ? colors.red : colors.gold;
          }
          context.globalAlpha = Math.min(alpha, 1);
          context.beginPath();
          context.moveTo(x, y);
          context.lineTo(x, y + length);
          context.stroke();
        }
      }
      context.globalAlpha = 1;
    };

    // about 30 frames a second is plenty for a drift, and kinder to a screen left open all day;
    // browsers stop animation frames in hidden tabs, so it rests there by itself
    const loop = (time: number) => {
      frame = requestAnimationFrame(loop);
      if (time - last < 33) return;
      last = time;
      draw(time);
    };

    const still = stillness();
    const redrawStill = () => still && draw(0);
    const move = (event: PointerEvent) => {
      const rect = element.getBoundingClientRect();
      pointer = { x: event.clientX - rect.left, y: event.clientY - rect.top };
    };
    const leave = () => (pointer = { x: -1e4, y: -1e4 });
    const resized = new ResizeObserver(() => {
      size();
      redrawStill();
    });
    const themed = new MutationObserver(() => {
      colors = readColors();
      redrawStill();
    });

    size();
    resized.observe(element);
    themed.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    if (still) {
      draw(0);
    } else {
      window.addEventListener("pointermove", move);
      document.documentElement.addEventListener("pointerleave", leave);
      frame = requestAnimationFrame(loop);
    }
    return () => {
      cancelAnimationFrame(frame);
      resized.disconnect();
      themed.disconnect();
      window.removeEventListener("pointermove", move);
      document.documentElement.removeEventListener("pointerleave", leave);
    };
  }, []);

  return <canvas ref={canvas} aria-hidden className="field-mask pointer-events-none absolute inset-x-0 top-0 -z-10 h-[560px] w-full" />;
}

function readColors() {
  const style = getComputedStyle(document.documentElement);
  return {
    tick: style.getPropertyValue("--field-tick").trim(),
    gold: style.getPropertyValue("--field-gold").trim(),
    red: style.getPropertyValue("--field-red").trim(),
  };
}

/** counts from the last value shown to the new one, like deepbook's figures */
export function useCountUp(value: number) {
  const [shown, setShown] = useState(0);
  const from = useRef(0);

  useEffect(() => {
    if (stillness()) {
      from.current = value;
      setShown(value);
      return;
    }
    const start = performance.now();
    const origin = from.current;
    let frame = 0;
    const step = (now: number) => {
      const progress = Math.min(1, (now - start) / 700);
      const eased = 1 - (1 - progress) ** 3;
      const next = Math.round(origin + (value - origin) * eased);
      from.current = next;
      setShown(next);
      if (progress < 1) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [value]);

  return shown;
}

/** each word rises out of its own clipped line; a new text plays it again */
export function RiseWords({ text }: { text: string }) {
  const words = text.split(" ");
  return (
    <>
      <span className="sr-only">{text}</span>
      {words.map((word, i) => (
        <span key={`${text}-${i}`} aria-hidden>
          <span className="inline-block overflow-hidden pb-[0.08em] align-bottom">
            <span className="rise inline-block" style={{ animationDelay: `${i * 70}ms` }}>
              {word}
            </span>
          </span>
          {i < words.length - 1 && " "}
        </span>
      ))}
    </>
  );
}

/** fades and lifts its content in the first time it scrolls into view */
export function Reveal({ children, className = "" }: { children: ReactNode; className?: string }) {
  const element = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState(false);

  useEffect(() => {
    const target = element.current;
    if (!target) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setShown(true);
          observer.disconnect();
        }
      },
      { rootMargin: "0px 0px -8% 0px" },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, []);

  return (
    <div ref={element} className={`reveal ${shown ? "in" : ""} ${className}`}>
      {children}
    </div>
  );
}
