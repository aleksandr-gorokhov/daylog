import { type MouseEvent, useMemo, useRef, useState } from "react";
import type { Minute } from "../api";
import { appLabel, CATEGORY_LABEL, categoryOf, hourRange, stripRuns, toMin } from "../logic";

const pct = (n: number) => `${n}%`;

/** Полоса дня: по отрезку на минуту, цвет по категории, пропуски пустые. */
export function DayStrip(props: { minutes: Minute[]; focusT: string | null; onPick: (t: string) => void }) {
  const { minutes, focusT, onPick } = props;
  const range = hourRange(minutes);
  const runs = useMemo(() => stripRuns(minutes), [minutes]);
  const byMin = useMemo(() => new Map(minutes.map((m) => [toMin(m.t), m])), [minutes]);
  const ref = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<{ minute: Minute; x: number; width: number } | null>(null);

  if (!range) return null;
  const total = range.end - range.start;
  const at = (offset: number) => pct(((offset - range.start) / total) * 100);

  const minuteAt = (e: MouseEvent) => {
    const rect = ref.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const minute = byMin.get(range.start + Math.floor((x / rect.width) * total));
    return { minute, x, width: rect.width };
  };

  const hours: number[] = [];
  const step = total / 60 > 10 ? 2 : 1;
  for (let h = range.start / 60; h <= range.end / 60; h += step) hours.push(h);

  return (
    <div className="strip-wrap">
      <div
        className="strip"
        ref={ref}
        role="img"
        aria-label="Полоса дня"
        onMouseMove={(e) => {
          const { minute, x, width } = minuteAt(e);
          setHover(minute ? { minute, x, width } : null);
        }}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          const { minute } = minuteAt(e);
          if (minute) onPick(minute.t);
        }}
      >
        {runs.map((r) => (
          <div
            key={r.start}
            className={`seg cat-${r.category}${r.described ? "" : " undescribed"}`}
            style={{ left: at(r.start), width: pct((r.length / total) * 100) }}
          />
        ))}
        {focusT && <div className="seg-focus" style={{ left: at(toMin(focusT)), width: pct(100 / total) }} />}
        {hover && (
          <div
            className="tooltip"
            style={{ left: Math.min(Math.max(hover.x, 150), Math.max(hover.width - 150, 150)) }}
          >
            <span className="num">{hover.minute.t}</span> ·{" "}
            {hover.minute.activity !== null && `${CATEGORY_LABEL[categoryOf(hover.minute)]} · `}
            {appLabel(hover.minute.app)} ·{" "}
            {hover.minute.activity ?? <span className="muted">ещё не описано</span>}
          </div>
        )}
      </div>
      <div className="hours">
        {hours.map((h) => (
          <span key={h} className="num" style={{ left: at(h * 60) }}>
            {h % 24}:00
          </span>
        ))}
      </div>
    </div>
  );
}
