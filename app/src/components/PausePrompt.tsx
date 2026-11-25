import { type KeyboardEvent, useEffect, useState } from "react";
import { api, errorText, onPausePrompt, type PausePrompt as Prompt } from "../api";
import { formatDuration, plural } from "../logic";
import { REASON_MAX } from "./PauseList";

/**
 * Окно с вопросом «Зачем отходил?». Спрашивает об одной паузе: какой именно, решает Rust
 * (самая поздняя пауза сегодняшнего дня без причины). После сохранения приходит следующий
 * вопрос или `null`, и тогда окно прячется.
 *
 * Поле причины не получает фокус само: окно появляется, когда пользователь уже печатает
 * в другом приложении, и не должно перехватывать этот текст. Ввод начинается после клика.
 */
export function PausePrompt() {
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Вопрос приходит событием; первый запрашивается командой после подписки: событие могло
  // уйти, пока скрытое окно ещё загружалось.
  useEffect(() => {
    let alive = true;
    const apply = (next: Prompt | null) => {
      if (alive) setPrompt(next);
    };
    const unlisten = onPausePrompt(apply);
    unlisten
      .then(() => api.pausePrompt())
      .then(apply)
      .catch((e) => console.error("pause_prompt", e));
    return () => {
      alive = false;
      unlisten.then((f) => f());
    };
  }, []);

  // Набранный текст живёт, пока вопрос о той же паузе: она может вырасти, начало прежнее.
  const asked = prompt ? `${prompt.day} ${prompt.pause.start}` : null;
  useEffect(() => {
    setText("");
    setError(null);
  }, [asked]);

  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") api.snoozePausePrompt().catch((err) => console.error("snooze_pause_prompt", err));
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (!prompt) return <div className="prompt prompt-empty">Пауз без причины нет.</div>;
  const { pause, remaining, recent } = prompt;

  const save = async (value: string) => {
    const reason = value.trim();
    if (reason === "" || saving) return;
    setSaving(true);
    setError(null);
    try {
      await api.setPauseReason(prompt.day, pause, reason);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") save(text);
  };

  return (
    <div className="prompt">
      <h1>Зачем отходил?</h1>
      <p className="prompt-pause num">
        {pause.start}–{pause.end} · {formatDuration(pause.minutes)}
      </p>
      {pause.before && (
        <p className="prompt-before" title={pause.before}>
          <span className="muted">Перед паузой:</span> {pause.before}
        </p>
      )}
      <input
        className="prompt-input"
        placeholder="устал, кофе, поговорил с…"
        aria-label="Причина паузы"
        value={text}
        maxLength={REASON_MAX}
        disabled={saving}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
      />
      {recent.length > 0 && (
        <div className="prompt-recent" aria-label="Недавние причины">
          {recent.map((reason) => (
            <button key={reason} className="prompt-chip" title={reason} disabled={saving} onClick={() => save(reason)}>
              {reason}
            </button>
          ))}
        </div>
      )}
      {error && (
        <pre className="prompt-error" role="alert">
          {error}
        </pre>
      )}
      <div className="prompt-actions">
        <span className="prompt-remaining muted num">
          {remaining > 0 && `ещё ${remaining} ${plural(remaining, ["пауза", "паузы", "пауз"])} без причины`}
        </span>
        <button className="btn" onClick={() => api.snoozePausePrompt()}>
          Позже
        </button>
        <button className="btn primary" disabled={saving || text.trim() === ""} onClick={() => save(text)}>
          Сохранить
        </button>
      </div>
    </div>
  );
}
