// Чистые функции экрана: время, пропуски, фильтр, итоги по категориям, паузы.

export type Category = "work" | "nonwork" | "idle";

export interface Minute {
  t: string;
  app: string;
  /** null — минута снята, но ещё не описана. */
  activity: string | null;
  /** null — минута не описана или описана без категории. */
  category: Category | null;
}

/** Пауза дня: отрезок из минут «нет ввода» и незаписанных минут, без созвонов. */
export interface Pause {
  /** Первая минута паузы. */
  start: string;
  /** Минута после последней; «24:00», если пауза идёт до полуночи. */
  end: string;
  minutes: number;
  /** null — пауза без причины. */
  reason: string | null;
  /** Описание последней описанной минуты перед паузой; null — такой минуты нет. */
  before: string | null;
}

/** «ЧЧ:ММ» → минуты от начала суток. */
export function toMin(t: string): number {
  return Number(t.slice(0, 2)) * 60 + Number(t.slice(3, 5));
}

/** Минуты от начала суток → «ЧЧ:ММ». */
export function fromMin(m: number): string {
  const h = Math.floor(m / 60) % 24;
  return `${String(h).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
}

/** 205 → «3 ч 25 мин», 45 → «45 мин». */
export function formatDuration(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return h > 0 ? `${h} ч ${m} мин` : `${m} мин`;
}

/** 0.6 → «60%»; ненулевая доля меньше 0,5% → «<1%». */
export function formatShare(share: number): string {
  if (share > 0 && share < 0.005) return "<1%";
  return `${Math.round(share * 100)}%`;
}

/** Форма слова по числу: `plural(2, ["пауза", "паузы", "пауз"])` → «паузы». */
export function plural(n: number, forms: readonly [string, string, string]): string {
  const tens = Math.abs(n) % 100;
  const ones = tens % 10;
  if (tens >= 11 && tens <= 14) return forms[2];
  if (ones === 1) return forms[0];
  if (ones >= 2 && ones <= 4) return forms[1];
  return forms[2];
}

export function appLabel(app: string): string {
  return app || "неизвестно";
}

export type JournalItem =
  | { kind: "minute"; minute: Minute }
  | { kind: "gap"; from: string; to: string; minutes: number };

/**
 * Строки журнала с разделителями «не за компьютером» там, где между соседними минутами
 * пропущено `minGap` минут или больше. `from`–`to` — пропущенный отрезок, конец не включён.
 */
export function withGaps(minutes: Minute[], minGap = 5): JournalItem[] {
  const items: JournalItem[] = [];
  minutes.forEach((minute, i) => {
    if (i > 0) {
      const prev = toMin(minutes[i - 1].t);
      const missing = toMin(minute.t) - prev - 1;
      if (missing >= minGap) {
        items.push({ kind: "gap", from: fromMin(prev + 1), to: minute.t, minutes: missing });
      }
    }
    items.push({ kind: "minute", minute });
  });
  return items;
}

/** Ключ категории для экрана: `none` — минута не описана или описана без категории. */
export type CategoryKey = Category | "none";

export const CATEGORY_ORDER: readonly CategoryKey[] = ["work", "nonwork", "idle", "none"];

export const CATEGORY_LABEL: Record<CategoryKey, string> = {
  work: "рабочее",
  nonwork: "нерабочее",
  idle: "ничего не делал",
  none: "не размечено",
};

/** «рабочее» → «Рабочее». */
export function categoryTitle(key: CategoryKey): string {
  const label = CATEGORY_LABEL[key];
  return label[0].toUpperCase() + label.slice(1);
}

export function categoryOf(m: Minute): CategoryKey {
  return m.category ?? "none";
}

/** Подстрока без учёта регистра в приложении и описании; `category` — выбранная категория. */
export function filterMinutes(minutes: Minute[], query: string, category: CategoryKey | null): Minute[] {
  const q = query.trim().toLocaleLowerCase("ru");
  return minutes.filter(
    (m) =>
      (category === null || categoryOf(m) === category) &&
      (q === "" ||
        m.app.toLocaleLowerCase("ru").includes(q) ||
        (m.activity ?? "").toLocaleLowerCase("ru").includes(q)),
  );
}

export interface CategoryShare {
  key: CategoryKey;
  minutes: number;
  share: number;
}

/**
 * Итоги дня: «рабочее», «нерабочее», «ничего не делал» всегда, «не размечено» — если такие
 * минуты есть; доля — от всех минут дня, сумма строк равна числу минут.
 */
export function categoryBreakdown(minutes: Minute[]): CategoryShare[] {
  const counts = new Map<CategoryKey, number>(CATEGORY_ORDER.map((k) => [k, 0]));
  for (const m of minutes) counts.set(categoryOf(m), counts.get(categoryOf(m))! + 1);
  const total = minutes.length;
  return CATEGORY_ORDER.filter((k) => k !== "none" || counts.get(k)! > 0).map((key) => ({
    key,
    minutes: counts.get(key)!,
    share: total ? counts.get(key)! / total : 0,
  }));
}

/** Рабочие минуты дня. */
export function workMinutes(minutes: Minute[]): number {
  return minutes.filter((m) => m.category === "work").length;
}

export interface PauseTotals {
  count: number;
  /** Суммарная длительность пауз в минутах. */
  minutes: number;
  withoutReason: number;
}

/** Итоги над списком пауз. */
export function pauseTotals(pauses: Pause[]): PauseTotals {
  return {
    count: pauses.length,
    minutes: pauses.reduce((sum, p) => sum + p.minutes, 0),
    withoutReason: pauses.filter((p) => p.reason === null).length,
  };
}

/** «7 пауз · 57 мин»; пустой список — «пауз нет». */
export function pauseTotalsLabel(totals: PauseTotals): string {
  if (totals.count === 0) return "пауз нет";
  return `${totals.count} ${plural(totals.count, ["пауза", "паузы", "пауз"])} · ${formatDuration(totals.minutes)}`;
}

/**
 * Минута журнала, к которой ведёт клик по паузе: первая записанная не раньше её начала.
 * У паузы, которая начинается с пропуска, это первая минута после него. `minutes` — по возрастанию.
 */
export function pauseTarget(minutes: Minute[], pause: Pause): string | null {
  return minutes.find((m) => m.t >= pause.start)?.t ?? null;
}

/** Диапазон полосы дня: от часа первой минуты до часа после последней, в минутах суток. */
export function hourRange(minutes: Minute[]): { start: number; end: number } | null {
  if (minutes.length === 0) return null;
  const ts = minutes.map((m) => toMin(m.t));
  const start = Math.floor(Math.min(...ts) / 60) * 60;
  const end = (Math.floor(Math.max(...ts) / 60) + 1) * 60;
  return { start, end };
}

export interface Run {
  start: number;
  length: number;
  category: CategoryKey;
  described: boolean;
}

/** Подряд идущие минуты одной категории с одинаковым признаком «описано» — один отрезок полосы. */
export function stripRuns(minutes: Minute[]): Run[] {
  const runs: Run[] = [];
  for (const m of minutes) {
    const t = toMin(m.t);
    const category = categoryOf(m);
    const described = m.activity !== null;
    const last = runs[runs.length - 1];
    if (last && last.start + last.length === t && last.category === category && last.described === described) {
      last.length += 1;
    } else {
      runs.push({ start: t, length: 1, category, described });
    }
  }
  return runs;
}

/** Длительность интервала «09:29»–«09:33» в минутах. */
export function intervalMinutes(start: string, end: string): number {
  const d = toMin(end) - toMin(start);
  return d >= 0 ? d : d + 24 * 60;
}

const WEEKDAYS_SHORT = ["вс", "пн", "вт", "ср", "чт", "пт", "сб"];
const WEEKDAYS = ["воскресенье", "понедельник", "вторник", "среда", "четверг", "пятница", "суббота"];
const MONTHS = [
  "января", "февраля", "марта", "апреля", "мая", "июня",
  "июля", "августа", "сентября", "октября", "ноября", "декабря",
];

function parseDay(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d);
}

/** «2026-11-16» → «16 ноября, пн». */
export function dayShort(day: string): string {
  const d = parseDay(day);
  return `${d.getDate()} ${MONTHS[d.getMonth()]}, ${WEEKDAYS_SHORT[d.getDay()]}`;
}

/** «2026-11-16» → «Понедельник, 16 ноября 2026». */
export function dayLong(day: string): string {
  const d = parseDay(day);
  const w = WEEKDAYS[d.getDay()];
  return `${w[0].toUpperCase()}${w.slice(1)}, ${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()}`;
}

/** Локальная дата в виде «ГГГГ-ММ-ДД». */
export function localDay(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

/** «2026-11-16T15:02:11» → «16 ноября в 15:02». */
export function formatGeneratedAt(iso: string): string {
  const m = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/.exec(iso);
  if (!m) return iso;
  const d = parseDay(m[1]);
  return `${d.getDate()} ${MONTHS[d.getMonth()]} в ${m[2]}`;
}
