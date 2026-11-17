import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  type CollectorError,
  type CollectorStatus,
  type Day,
  type DaySummary,
  errorText,
  onClassifyFinished,
  onReportFinished,
  toCollectorError,
} from "./api";
import { CategorySummary } from "./components/CategorySummary";
import { Banner, ReportButton } from "./components/common";
import { DayList } from "./components/DayList";
import { DayStrip } from "./components/DayStrip";
import { type Focus, MinutesTab } from "./components/MinutesTab";
import { type CollectorBusy, RecordControl } from "./components/RecordControl";
import { TimelineTab } from "./components/TimelineTab";
import { type CategoryKey, dayLong, filterMinutes, formatDuration, localDay, workMinutes } from "./logic";
import { usePolling } from "./usePolling";

const POLL_MS = 15_000;

type Tab = "timeline" | "minutes";

/** Сегодняшний день, если он есть, иначе самый поздний. */
function initialDay(days: DaySummary[]): string | null {
  const today = localDay();
  return days.find((d) => d.day === today)?.day ?? days[0]?.day ?? null;
}

function without<T>(set: ReadonlySet<T>, item: T): ReadonlySet<T> {
  const next = new Set(set);
  next.delete(item);
  return next;
}

// Возобновление записи запрашивается один раз за загрузку страницы (StrictMode монтирует дважды).
let resumeRequested = false;

export default function App() {
  const [days, setDays] = useState<DaySummary[] | null>(null);
  const [daysError, setDaysError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [day, setDay] = useState<Day | null>(null);
  const [dayError, setDayError] = useState<string | null>(null);

  const [tab, setTab] = useState<Tab>("minutes");
  const [query, setQuery] = useState("");
  const [categoryFilter, setCategoryFilter] = useState<CategoryKey | null>(null);
  const [focus, setFocus] = useState<Focus | null>(null);

  const [collector, setCollector] = useState<CollectorStatus | null>(null);
  const [collectorBusy, setCollectorBusyState] = useState<CollectorBusy>(null);
  const [collectorError, setCollectorError] = useState<{ op: "start" | "stop"; error: CollectorError } | null>(null);

  const [reportsRunning, setReportsRunning] = useState<ReadonlySet<string>>(new Set());
  const [reportErrors, setReportErrors] = useState<Record<string, string>>({});
  const [classifying, setClassifying] = useState<ReadonlySet<string>>(new Set());
  const [classifyErrors, setClassifyErrors] = useState<Record<string, string>>({});
  /** Дни, для которых доразметка уже запрашивалась с этой страницы. */
  const classifyRequested = useRef(new Set<string>());

  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const busyRef = useRef<CollectorBusy>(null);
  const setCollectorBusy = (b: CollectorBusy) => {
    busyRef.current = b;
    setCollectorBusyState(b);
  };

  const loadDays = useCallback(async () => {
    try {
      const list = await api.listDays();
      setDays(list);
      setDaysError(null);
      setSelected((cur) => cur ?? initialDay(list));
    } catch (e) {
      setDaysError(errorText(e));
      setDays((cur) => cur ?? []);
    }
  }, []);

  const loadDay = useCallback(async (d: string) => {
    try {
      const data = await api.getDay(d);
      if (selectedRef.current === d) {
        setDay(data);
        setDayError(null);
      }
    } catch (e) {
      if (selectedRef.current === d) setDayError(errorText(e));
    }
  }, []);

  const loadStatus = useCallback(async () => {
    try {
      const st = await api.collectorStatus();
      if (busyRef.current === null) setCollector(st);
    } catch (e) {
      console.error("collector_status", e);
    }
  }, []);

  // Первая загрузка: дни, состояние записи, идущие сборки отчётов, возобновление записи.
  useEffect(() => {
    loadDays();
    loadStatus();
    api
      .reportsInProgress()
      .then((list) => setReportsRunning(new Set(list)))
      .catch((e) => console.error("reports_in_progress", e));
    if (!resumeRequested) {
      resumeRequested = true;
      setCollectorBusy("start");
      api
        .resumeCollector()
        .then((st) => st && setCollector(st))
        .catch((e) => setCollectorError({ op: "start", error: toCollectorError(e) }))
        .finally(() => {
          setCollectorBusy(null);
          loadStatus();
        });
    }
  }, [loadDays, loadStatus]);

  useEffect(() => {
    setDay(null);
    setDayError(null);
    setFocus(null);
    if (selected) loadDay(selected);
  }, [selected, loadDay]);

  usePolling(() => {
    loadDays();
    if (selectedRef.current) loadDay(selectedRef.current);
    loadStatus();
  }, POLL_MS);

  useEffect(() => {
    const unlisten = onReportFinished((e) => {
      setReportsRunning((s) => without(s, e.day));
      setReportErrors(({ [e.day]: _, ...rest }) => (e.ok ? rest : { ...rest, [e.day]: e.error ?? "неизвестная ошибка" }));
      loadDays();
      if (selectedRef.current === e.day) loadDay(e.day);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [loadDays, loadDay]);

  // Доразметка: открытый день с описанными минутами без категории размечается сам.
  // Rust запускает её не больше одного раза за запуск приложения; здесь — не чаще одного
  // запроса на день с этой страницы, чтобы не дёргать команду при каждом опросе.
  useEffect(() => {
    if (!day || day.unclassified === 0 || classifyRequested.current.has(day.day)) return;
    const d = day.day;
    classifyRequested.current.add(d);
    api
      .classifyDay(d)
      .then((started) => {
        if (started !== "skipped") setClassifying((s) => new Set(s).add(d));
      })
      .catch((e) => setClassifyErrors((errs) => ({ ...errs, [d]: errorText(e) })));
  }, [day]);

  useEffect(() => {
    const unlisten = onClassifyFinished((e) => {
      setClassifying((s) => without(s, e.day));
      setClassifyErrors(({ [e.day]: _, ...rest }) => (e.ok ? rest : { ...rest, [e.day]: e.error ?? "неизвестная ошибка" }));
      loadDays();
      if (selectedRef.current === e.day) loadDay(e.day);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [loadDays, loadDay]);

  const generateReport = (d: string) => {
    setReportsRunning((s) => new Set(s).add(d));
    setReportErrors(({ [d]: _, ...rest }) => rest);
    api.generateReport(d).catch((e) => {
      setReportsRunning((s) => without(s, d));
      setReportErrors((errs) => ({ ...errs, [d]: errorText(e) }));
    });
  };

  const toggleRecording = async () => {
    const op = collector?.running ? "stop" : "start";
    setCollectorBusy(op);
    setCollectorError(null);
    try {
      setCollector(await (op === "start" ? api.startCollector() : api.stopCollector()));
    } catch (e) {
      setCollectorError({ op, error: toCollectorError(e) });
    } finally {
      setCollectorBusy(null);
      loadStatus();
    }
  };

  const pickMinute = (t: string) => {
    setTab("minutes");
    if (day && !filterMinutes(day.minutes, query, categoryFilter).some((m) => m.t === t)) {
      setQuery("");
      setCategoryFilter(null);
    }
    setFocus((f) => ({ t, seq: (f?.seq ?? 0) + 1 }));
  };

  const toggleCategory = (category: CategoryKey) => {
    setCategoryFilter((cur) => (cur === category ? null : category));
    setTab("minutes");
  };

  const today = localDay();
  const reportRunning = selected !== null && reportsRunning.has(selected);
  const reportError = selected !== null ? reportErrors[selected] : undefined;
  const classifyError = selected !== null ? classifyErrors[selected] : undefined;
  const minutes = day?.minutes ?? [];

  let content;
  if (days === null) {
    content = <div className="placeholder">Загрузка…</div>;
  } else if (days.length === 0) {
    content = (
      <div className="empty-state">
        <p>{daysError ? `Не удалось прочитать каталог данных: ${daysError}` : "Записей пока нет."}</p>
        <p className="muted">
          Нажмите «Начать запись»: daylog будет раз в минуту снимать экран, а здесь появятся минуты дня.
        </p>
      </div>
    );
  } else if (dayError) {
    content = <div className="placeholder">{dayError}</div>;
  } else if (!day) {
    content = <div className="placeholder">Загрузка…</div>;
  } else if (minutes.length === 0) {
    content = <div className="placeholder">За этот день минут нет.</div>;
  } else {
    content = (
      <>
        <DayStrip minutes={minutes} focusT={focus?.t ?? null} onPick={pickMinute} />
        <div className="body">
          <section className="content">
            <div className="tabs" role="tablist">
              <button role="tab" aria-selected={tab === "minutes"} onClick={() => setTab("minutes")}>
                Минуты
              </button>
              <button role="tab" aria-selected={tab === "timeline"} onClick={() => setTab("timeline")}>
                Таймлайн
              </button>
            </div>
            <MinutesTab
              day={day.day}
              minutes={minutes}
              active={tab === "minutes"}
              query={query}
              onQuery={setQuery}
              categoryFilter={categoryFilter}
              onClearCategory={() => setCategoryFilter(null)}
              focus={focus}
            />
            <TimelineTab
              report={day.report}
              active={tab === "timeline"}
              running={reportRunning}
              onGenerate={() => generateReport(day.day)}
            />
          </section>
          <CategorySummary
            minutes={minutes}
            selected={categoryFilter}
            classifying={classifying.has(day.day)}
            onToggle={toggleCategory}
          />
        </div>
      </>
    );
  }

  return (
    <div className="layout">
      <DayList days={days} selected={selected} today={today} onSelect={setSelected} />
      <main className="main">
        <header className="header">
          <div className="header-title">
            <h1>{selected ? dayLong(selected) : "daylog"}</h1>
            {day && minutes.length > 0 && (
              <div className="header-stats num">
                работа {formatDuration(workMinutes(minutes))} · за компьютером {formatDuration(day.active_minutes)} ·{" "}
                {minutes[0].t}–{minutes[minutes.length - 1].t}
              </div>
            )}
          </div>
          {day && minutes.length > 0 && (
            <ReportButton
              running={reportRunning}
              hasReport={day.report !== null}
              onClick={() => generateReport(day.day)}
            />
          )}
          <RecordControl status={collector} busy={collectorBusy} onToggle={toggleRecording} />
        </header>

        {collectorError &&
          (collectorError.error.kind === "permission" ? (
            <Banner
              title="Нужно разрешение на запись экрана."
              onClose={() => setCollectorError(null)}
              actions={
                <button className="btn" onClick={() => api.openScreenRecordingSettings()}>
                  Открыть настройки
                </button>
              }
            >
              <span>Включите daylog в разделе Screen & System Audio Recording; после выдачи разрешения перезапустите daylog.</span>
            </Banner>
          ) : (
            <Banner
              title={collectorError.op === "start" ? "Запись не запустилась." : "Запись не остановилась."}
              onClose={() => setCollectorError(null)}
            >
              <pre>{collectorError.error.message}</pre>
            </Banner>
          ))}
        {classifyError && (
          <Banner title="Разметка не удалась." onClose={() => setClassifyErrors(({ [selected!]: _, ...rest }) => rest)}>
            <pre>{classifyError}</pre>
          </Banner>
        )}
        {reportError && (
          <Banner title="Отчёт не собран." onClose={() => setReportErrors(({ [selected!]: _, ...rest }) => rest)}>
            <pre>{reportError}</pre>
          </Banner>
        )}

        {content}
      </main>
    </div>
  );
}
