import type { Report } from "../api";
import { formatDuration, formatGeneratedAt, intervalMinutes } from "../logic";
import { CategoryDot, ReportButton } from "./common";

export function TimelineTab(props: {
  report: Report | null;
  active: boolean;
  running: boolean;
  onGenerate: () => void;
}) {
  const { report, active, running, onGenerate } = props;
  if (!report) {
    return (
      <div className="timeline empty-state" hidden={!active}>
        <p>Отчёт за этот день ещё не собран.</p>
        <p className="muted">LLM склеит минуты в интервалы, как `daylog.py report`.</p>
        <ReportButton running={running} hasReport={false} onClick={onGenerate} />
      </div>
    );
  }
  return (
    <div className="timeline" hidden={!active}>
      <div className="report-meta">
        Собран {formatGeneratedAt(report.generated_at)} · интервалов: {report.intervals.length}
      </div>
      {report.stale && <div className="stale">Есть новые минуты после сборки.</div>}
      <ol className="intervals">
        {report.intervals.map((i, n) => (
          <li key={n} className={/^не за компьютером/i.test(i.activity) ? "away" : undefined}>
            <time className="num">
              {i.start}–{i.end}
            </time>
            <span className="interval-dur num">{formatDuration(intervalMinutes(i.start, i.end))}</span>
            <span className="interval-act">
              {i.category ? <CategoryDot category={i.category} /> : <i className="dot dot-empty" aria-hidden />}
              <span>{i.activity}</span>
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}
