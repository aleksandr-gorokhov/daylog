import { memo, useEffect, useMemo, useRef } from "react";
import type { Minute } from "../api";
import {
  appLabel,
  categoryOf,
  type CategoryKey,
  categoryTitle,
  filterMinutes,
  formatDuration,
  type JournalItem,
  withGaps,
} from "../logic";
import { CategoryDot } from "./common";

export interface Focus {
  t: string;
  /** Растёт при каждом выборе минуты, чтобы повторный клик снова прокручивал к ней. */
  seq: number;
}

const Row = memo(function Row({ m, focused }: { m: Minute; focused: boolean }) {
  return (
    <div className={`row${focused ? " focused" : ""}`} data-t={m.t}>
      <time className="num">{m.t}</time>
      <span className="row-app" title={appLabel(m.app)}>
        <CategoryDot category={categoryOf(m)} />
        {appLabel(m.app)}
      </span>
      <span className={`row-act${m.activity === null ? " muted" : ""}`}>
        {m.activity ?? "ещё не описано"}
        {m.activity !== null && m.category === null && <span className="tag">не размечено</span>}
      </span>
    </div>
  );
});

export function MinutesTab(props: {
  day: string;
  minutes: Minute[];
  active: boolean;
  query: string;
  onQuery: (q: string) => void;
  categoryFilter: CategoryKey | null;
  onClearCategory: () => void;
  focus: Focus | null;
}) {
  const { day, minutes, active, query, onQuery, categoryFilter, onClearCategory, focus } = props;
  const filtered = useMemo(() => filterMinutes(minutes, query, categoryFilter), [minutes, query, categoryFilter]);
  const filtering = query.trim() !== "" || categoryFilter !== null;
  const items: JournalItem[] = useMemo(
    () => (filtering ? filtered.map((minute) => ({ kind: "minute", minute })) : withGaps(filtered)),
    [filtered, filtering],
  );
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    listRef.current?.scrollTo(0, 0);
  }, [day]);

  useEffect(() => {
    if (!focus || !active) return;
    listRef.current?.querySelector(`[data-t="${focus.t}"]`)?.scrollIntoView({ block: "center" });
  }, [focus, active]);

  return (
    <div className="minutes" hidden={!active}>
      <div className="toolbar">
        <input
          type="search"
          className="search"
          placeholder="Поиск по приложению и описанию"
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          spellCheck={false}
        />
        {categoryFilter !== null && (
          <span className="chip">
            <CategoryDot category={categoryFilter} />
            {categoryTitle(categoryFilter)}
            <button aria-label="Снять фильтр по категории" title="Снять фильтр" onClick={onClearCategory}>
              ×
            </button>
          </span>
        )}
        <span className="count num">
          {filtering ? `${filtered.length} из ${minutes.length}` : `${minutes.length} мин`}
        </span>
      </div>
      <div className="journal" ref={listRef}>
        {filtered.length === 0 ? (
          <div className="placeholder">ничего не найдено</div>
        ) : (
          items.map((item) =>
            item.kind === "gap" ? (
              <div key={`gap-${item.from}`} className="gap">
                <span>не за компьютером {formatDuration(item.minutes)}</span>
                <span className="num">
                  {item.from}–{item.to}
                </span>
              </div>
            ) : (
              <Row key={item.minute.t} m={item.minute} focused={focus?.t === item.minute.t} />
            ),
          )
        )}
      </div>
    </div>
  );
}
