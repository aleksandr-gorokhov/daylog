import type { DaySummary } from "../api";
import { dayShort, formatDuration } from "../logic";

export function DayList(props: {
  days: DaySummary[] | null;
  selected: string | null;
  today: string;
  onSelect: (day: string) => void;
}) {
  const { days, selected, today, onSelect } = props;
  return (
    <nav className="sidebar" aria-label="Дни">
      <div className="sidebar-title">daylog</div>
      {days?.map((d) => (
        <button
          key={d.day}
          className={`day-item${d.day === selected ? " selected" : ""}`}
          onClick={() => onSelect(d.day)}
          aria-current={d.day === selected ? "date" : undefined}
        >
          <span className="day-item-name">
            {dayShort(d.day)}
            {d.day === today && <span className="badge">сегодня</span>}
          </span>
          <span className="day-item-meta">
            <span className="num">
              работа {formatDuration(d.work_minutes)} из {formatDuration(d.active_minutes)}
            </span>
            {d.has_report && <i className="report-dot" title="отчёт собран" aria-label="отчёт собран" />}
          </span>
        </button>
      ))}
    </nav>
  );
}
