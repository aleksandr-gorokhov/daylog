import { type KeyboardEvent, useMemo, useState } from "react";
import { errorText, type Pause } from "../api";
import { formatDuration, pauseTotals, pauseTotalsLabel } from "../logic";

/** Самая длинная причина, которую даёт ввести поле. */
export const REASON_MAX = 200;

function without<T>(set: ReadonlySet<T>, item: T): ReadonlySet<T> {
  const next = new Set(set);
  next.delete(item);
  return next;
}

/**
 * Паузы дня с причинами. Пауза без причины выделена и показана с полем ввода; записанную
 * причину можно исправить, нажав на неё. Черновики хранятся по началу паузы, поэтому
 * обновление дня их не сбрасывает; при смене дня компонент пересоздаётся (`key`).
 */
export function PauseList(props: {
  pauses: Pause[];
  /** Клик по времени паузы: показать её в журнале. */
  onPick: (pause: Pause) => void;
  /** Записать причину и дождаться обновления дня; при ошибке — отклонённое обещание. */
  onSave: (pause: Pause, reason: string) => Promise<void>;
}) {
  const { pauses, onPick, onSave } = props;
  const totals = useMemo(() => pauseTotals(pauses), [pauses]);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [editing, setEditing] = useState<ReadonlySet<string>>(new Set());
  const [saving, setSaving] = useState<ReadonlySet<string>>(new Set());
  const [errors, setErrors] = useState<Record<string, string>>({});

  const setDraft = (start: string, text: string) => setDrafts((d) => ({ ...d, [start]: text }));
  const drop = (start: string) => {
    setDrafts(({ [start]: _, ...rest }) => rest);
    setErrors(({ [start]: _, ...rest }) => rest);
    setEditing((s) => without(s, start));
  };

  const save = async (pause: Pause) => {
    const reason = (drafts[pause.start] ?? "").trim();
    if (reason === "" || saving.has(pause.start)) return;
    if (reason === pause.reason) return drop(pause.start);
    setSaving((s) => new Set(s).add(pause.start));
    try {
      await onSave(pause, reason);
      drop(pause.start);
    } catch (e) {
      setErrors((errs) => ({ ...errs, [pause.start]: errorText(e) }));
    } finally {
      setSaving((s) => without(s, pause.start));
    }
  };

  const onKeyDown = (pause: Pause) => (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") save(pause);
    if (e.key === "Escape") drop(pause.start);
  };

  return (
    <section className="pauses" aria-label="Паузы дня">
      <h2>Паузы</h2>
      <p className="pauses-total num">
        <span>{pauseTotalsLabel(totals)}</span>
        {totals.withoutReason > 0 && <span className="pauses-missing">{totals.withoutReason} без причины</span>}
      </p>
      <ul>
        {pauses.map((p) => {
          const span = `${p.start}–${p.end}`;
          const asking = p.reason === null || editing.has(p.start);
          return (
            <li key={p.start} className={`pause${p.reason === null ? " no-reason" : ""}`}>
              <button className="pause-time num" title="Показать в журнале" onClick={() => onPick(p)}>
                {span}
              </button>
              <span className="pause-dur num">{formatDuration(p.minutes)}</span>
              {asking ? (
                <input
                  className="pause-input"
                  placeholder="зачем отходил?"
                  aria-label={`Причина паузы ${span}`}
                  value={drafts[p.start] ?? ""}
                  maxLength={REASON_MAX}
                  disabled={saving.has(p.start)}
                  autoFocus={p.reason !== null}
                  spellCheck={false}
                  onChange={(e) => setDraft(p.start, e.target.value)}
                  onKeyDown={onKeyDown(p)}
                  onBlur={() => {
                    // правку начали и ничего не изменили — вернуть записанную причину
                    if (p.reason !== null && (drafts[p.start] ?? "").trim() === p.reason) drop(p.start);
                  }}
                />
              ) : (
                <button
                  className="pause-reason"
                  title="Исправить причину"
                  onClick={() => {
                    setDraft(p.start, p.reason ?? "");
                    setEditing((s) => new Set(s).add(p.start));
                  }}
                >
                  {p.reason}
                </button>
              )}
              {errors[p.start] && (
                <span className="pause-error" role="alert">
                  {errors[p.start]}
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
