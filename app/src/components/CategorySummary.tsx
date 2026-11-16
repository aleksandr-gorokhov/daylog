import { useMemo } from "react";
import type { Minute } from "../api";
import { categoryBreakdown, type CategoryKey, categoryTitle, formatDuration, formatShare } from "../logic";
import { CategoryDot } from "./common";

/** Итоги дня по категориям; клик по строке включает и снимает фильтр журнала. */
export function CategorySummary(props: {
  minutes: Minute[];
  selected: CategoryKey | null;
  classifying: boolean;
  onToggle: (category: CategoryKey) => void;
}) {
  const { minutes, selected, classifying, onToggle } = props;
  const rows = useMemo(() => categoryBreakdown(minutes), [minutes]);
  return (
    <aside className="summary" aria-label="Итоги дня">
      <h2>Итоги дня</h2>
      <ul>
        {rows.map((r) => (
          <li key={r.key}>
            <button
              className={`summary-row${selected === r.key ? " active" : ""}`}
              onClick={() => onToggle(r.key)}
              title={selected === r.key ? "Снять фильтр" : "Показать минуты этой категории"}
            >
              <span className="summary-name">
                <CategoryDot category={r.key} />
                <span>{categoryTitle(r.key)}</span>
              </span>
              <span className="summary-dur num">{formatDuration(r.minutes)}</span>
              <span className="summary-share num">{formatShare(r.share)}</span>
              <span className="summary-bar">
                <span className={`cat-${r.key}`} style={{ width: `${r.share * 100}%` }} />
              </span>
              {r.key === "none" && classifying && <span className="summary-note">размечаю…</span>}
            </button>
          </li>
        ))}
      </ul>
      {classifying && !rows.some((r) => r.key === "none") && <p className="summary-note">размечаю…</p>}
    </aside>
  );
}
