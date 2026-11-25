// Типы и обёртки над командами Rust (src-tauri/src/lib.rs) и их событиями.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Category, Minute, Pause } from "./logic";

export type { Category, Minute, Pause };

export interface DaySummary {
  day: string;
  active_minutes: number;
  work_minutes: number;
}

export interface Interval {
  start: string;
  end: string;
  activity: string;
  /** null — «не за компьютером» или отчёт собран до появления категорий. */
  category: Category | null;
}

export interface Report {
  generated_at: string;
  minutes_count: number;
  intervals: Interval[];
  stale: boolean;
}

export interface Day {
  day: string;
  active_minutes: number;
  /** Описанные минуты без категории: такой день размечается сам при открытии. */
  unclassified: number;
  minutes: Minute[];
  /** Паузы дня по возрастанию времени, без созвонов. */
  pauses: Pause[];
  report: Report | null;
}

export interface CollectorStatus {
  running: boolean;
  pid: number | null;
}

export interface CollectorError {
  kind: "permission" | "failed";
  message: string;
}

/** События `report-finished` и `classify-finished`. */
export interface JobFinished {
  day: string;
  ok: boolean;
  error: string | null;
}

/** Вопрос окна `pause-prompt`: о какой паузе сегодняшнего дня спрашивать. */
export interface PausePrompt {
  day: string;
  pause: Pause;
  /** Сколько ещё пауз этого дня без причины, кроме этой. */
  remaining: number;
  /** Недавние причины, от новых к старым. */
  recent: string[];
}

/** Событие `pauses-changed`: в этом дне записана причина паузы. */
export interface PausesChanged {
  day: string;
}

/** started — запущена; running — уже идёт; skipped — уже запускалась в этом запуске приложения. */
export type ClassifyStart = "started" | "running" | "skipped";

export const api = {
  listDays: () => invoke<DaySummary[]>("list_days"),
  getDay: (day: string) => invoke<Day>("get_day", { day }),
  collectorStatus: () => invoke<CollectorStatus>("collector_status"),
  startCollector: () => invoke<CollectorStatus>("start_collector"),
  stopCollector: () => invoke<CollectorStatus>("stop_collector"),
  /** Возобновить запись при запуске, если при выходе она шла; `null` — возобновлять нечего. */
  resumeCollector: () => invoke<CollectorStatus | null>("resume_collector"),
  openScreenRecordingSettings: () => invoke<void>("open_screen_recording_settings"),
  generateReport: (day: string) => invoke<void>("generate_report", { day }),
  reportsInProgress: () => invoke<string[]>("reports_in_progress"),
  classifyDay: (day: string) => invoke<ClassifyStart>("classify_day", { day }),
  /** Причина паузы за отрезок `start`–`end` дня; записывает `daylog.py pause`. */
  setPauseReason: (day: string, pause: Pick<Pause, "start" | "end">, reason: string) =>
    invoke<void>("set_pause_reason", { day, start: pause.start, end: pause.end, reason }),
  /** Вопрос, который сейчас должно показывать окно с вопросом; `null` — спрашивать не о чем. */
  pausePrompt: () => invoke<PausePrompt | null>("pause_prompt"),
  /** «Позже»: спрятать окно с вопросом и отложить его. */
  snoozePausePrompt: () => invoke<void>("snooze_pause_prompt"),
};

export function onReportFinished(handler: (e: JobFinished) => void): Promise<UnlistenFn> {
  return listen<JobFinished>("report-finished", (event) => handler(event.payload));
}

export function onClassifyFinished(handler: (e: JobFinished) => void): Promise<UnlistenFn> {
  return listen<JobFinished>("classify-finished", (event) => handler(event.payload));
}

export function onPausesChanged(handler: (e: PausesChanged) => void): Promise<UnlistenFn> {
  return listen<PausesChanged>("pauses-changed", (event) => handler(event.payload));
}

/** Вопрос изменился: новая пауза или `null`, если пауз без причины не осталось. */
export function onPausePrompt(handler: (prompt: PausePrompt | null) => void): Promise<UnlistenFn> {
  return listen<PausePrompt | null>("pause-prompt", (event) => handler(event.payload));
}

/** Ошибка команды как текст: команды возвращают строку или `{kind, message}`. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

export function toCollectorError(e: unknown): CollectorError {
  if (e && typeof e === "object" && "kind" in e && "message" in e) {
    const { kind, message } = e as { kind: unknown; message: unknown };
    if (kind === "permission" || kind === "failed") return { kind, message: String(message) };
  }
  return { kind: "failed", message: errorText(e) };
}
