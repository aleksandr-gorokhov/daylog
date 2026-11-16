import { describe, expect, it } from "vitest";
import {
  type Category,
  categoryBreakdown,
  categoryTitle,
  dayLong,
  dayShort,
  filterMinutes,
  formatDuration,
  formatGeneratedAt,
  formatShare,
  fromMin,
  hourRange,
  intervalMinutes,
  localDay,
  type Minute,
  stripRuns,
  toMin,
  withGaps,
  workMinutes,
} from "./logic";

const m = (t: string, app = "Arc", activity: string | null = "читал", category: Category | null = "work"): Minute => ({
  t,
  app,
  activity,
  category: activity === null ? null : category,
});

/** Подряд идущие минуты начиная с `from`. */
function range(from: string, count: number, category: Category | null, activity: string | null = "занят"): Minute[] {
  return Array.from({ length: count }, (_, i) => m(fromMin(toMin(from) + i), "Arc", activity, category));
}

describe("время", () => {
  it("переводит ЧЧ:ММ туда и обратно", () => {
    expect(toMin("09:28")).toBe(568);
    expect(fromMin(568)).toBe("09:28");
    expect(fromMin(0)).toBe("00:00");
  });

  it("форматирует длительность", () => {
    expect(formatDuration(205)).toBe("3 ч 25 мин");
    expect(formatDuration(120)).toBe("2 ч 0 мин");
    expect(formatDuration(80)).toBe("1 ч 20 мин");
    expect(formatDuration(45)).toBe("45 мин");
    expect(formatDuration(0)).toBe("0 мин");
  });

  it("длительность интервала", () => {
    expect(intervalMinutes("09:29", "09:33")).toBe(4);
    expect(intervalMinutes("23:50", "00:10")).toBe(20);
  });

  it("даты по-русски", () => {
    expect(dayShort("2026-11-16")).toBe("16 ноября, пн");
    expect(dayLong("2026-11-16")).toBe("Понедельник, 16 ноября 2026");
    expect(localDay(new Date(2026, 0, 5, 23, 59))).toBe("2026-01-05");
    expect(formatGeneratedAt("2026-11-16T15:02:11")).toBe("16 ноября в 15:02");
  });

  it("доли", () => {
    expect(formatShare(0.6)).toBe("60%");
    expect(formatShare(1 / 224)).toBe("<1%");
    expect(formatShare(0)).toBe("0%");
    expect(formatShare(0.006)).toBe("1%");
  });
});

describe("пропуски", () => {
  it("разделитель от 5 пропущенных минут", () => {
    const items = withGaps([m("09:38"), m("09:39"), m("09:47"), m("09:50"), m("09:56")]);
    expect(items.filter((i) => i.kind === "gap")).toEqual([
      { kind: "gap", from: "09:40", to: "09:47", minutes: 7 },
      { kind: "gap", from: "09:51", to: "09:56", minutes: 5 },
    ]);
    expect(items.map((i) => (i.kind === "gap" ? "gap" : i.minute.t))).toEqual([
      "09:38", "09:39", "gap", "09:47", "09:50", "gap", "09:56",
    ]);
  });

  it("4 пропущенные минуты — без разделителя", () => {
    expect(withGaps([m("10:00"), m("10:05")]).every((i) => i.kind === "minute")).toBe(true);
  });
});

describe("фильтр", () => {
  const minutes = [
    m("10:00", "Arc", "читал PD-1428 в Jira", "work"),
    m("10:01", "Warp", "правил pd-1428/biome.json", "work"),
    m("10:02", "Telegram", "личная переписка", "nonwork"),
    m("10:03", "Arc", null),
    m("10:04", "Arc", "старая минута", null),
  ];

  it("подстрока без учёта регистра в описании и приложении", () => {
    expect(filterMinutes(minutes, "pd-1428", null).map((x) => x.t)).toEqual(["10:00", "10:01"]);
    expect(filterMinutes(minutes, "  TELEGRAM ", null).map((x) => x.t)).toEqual(["10:02"]);
    expect(filterMinutes(minutes, "ничего такого", null)).toEqual([]);
  });

  it("по категории и вместе с поиском", () => {
    expect(filterMinutes(minutes, "", "work").map((x) => x.t)).toEqual(["10:00", "10:01"]);
    expect(filterMinutes(minutes, "", "none").map((x) => x.t)).toEqual(["10:03", "10:04"]);
    expect(filterMinutes(minutes, "jira", "work").map((x) => x.t)).toEqual(["10:00"]);
    expect(filterMinutes(minutes, "", null)).toHaveLength(5);
  });
});

describe("итоги по категориям", () => {
  it("120 рабочих / 60 нерабочих / 20 «ничего не делал»", () => {
    const minutes = [...range("09:00", 120, "work"), ...range("11:00", 60, "nonwork"), ...range("12:00", 20, "idle")];
    const rows = categoryBreakdown(minutes);
    expect(rows.map((r) => [categoryTitle(r.key), formatDuration(r.minutes), formatShare(r.share)])).toEqual([
      ["Рабочее", "2 ч 0 мин", "60%"],
      ["Нерабочее", "1 ч 0 мин", "30%"],
      ["Ничего не делал", "20 мин", "10%"],
    ]);
    expect(workMinutes(minutes)).toBe(120);
  });

  it("«не размечено» — неописанные и описанные без категории, сумма равна числу минут", () => {
    const minutes = [
      ...range("09:00", 3, "work"),
      ...range("09:03", 2, null),
      ...range("09:05", 1, null, null),
      ...range("09:10", 4, "nonwork"),
    ];
    const rows = categoryBreakdown(minutes);
    expect(rows.map((r) => [r.key, r.minutes])).toEqual([
      ["work", 3],
      ["nonwork", 4],
      ["idle", 0],
      ["none", 3],
    ]);
    expect(rows.reduce((s, r) => s + r.minutes, 0)).toBe(minutes.length);
    expect(categoryBreakdown([]).map((r) => r.key)).toEqual(["work", "nonwork", "idle"]);
  });
});

describe("полоса дня", () => {
  it("диапазон часов", () => {
    expect(hourRange([m("09:28"), m("14:58")])).toEqual({ start: 540, end: 900 });
    expect(hourRange([m("14:00")])).toEqual({ start: 840, end: 900 });
    expect(hourRange([])).toBeNull();
  });

  it("склеивает подряд идущие минуты одной категории", () => {
    const runs = stripRuns([
      m("09:00", "Arc"),
      m("09:01", "Warp"), // другое приложение, та же категория — тот же отрезок
      m("09:02", "Telegram", "личное", "nonwork"),
      m("09:03", "Arc", "старое", null),
      m("09:04", "Arc", null),
      m("09:06", "Arc", null),
    ]);
    expect(runs).toEqual([
      { start: 540, length: 2, category: "work", described: true },
      { start: 542, length: 1, category: "nonwork", described: true },
      { start: 543, length: 1, category: "none", described: true },
      { start: 544, length: 1, category: "none", described: false },
      { start: 546, length: 1, category: "none", described: false },
    ]);
  });
});
