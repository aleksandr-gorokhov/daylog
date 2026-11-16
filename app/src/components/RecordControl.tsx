import type { CollectorStatus } from "../api";

export type CollectorBusy = "start" | "stop" | null;

export function RecordControl(props: { status: CollectorStatus | null; busy: CollectorBusy; onToggle: () => void }) {
  const { status, busy, onToggle } = props;
  const running = status?.running ?? false;
  const label =
    busy === "start" ? "Запускаю…" : busy === "stop" ? "Останавливаю…" : running ? "■ Остановить" : "▶ Начать запись";
  return (
    <div className="record">
      <span
        className={`record-indicator${running ? " on" : ""}`}
        title={running && status?.pid ? `сборщик, PID ${status.pid}` : undefined}
      >
        <i aria-hidden />
        {running ? "запись идёт" : "запись не идёт"}
      </span>
      <button className={`btn${running ? "" : " primary"}`} disabled={busy !== null || status === null} onClick={onToggle}>
        {label}
      </button>
    </div>
  );
}
