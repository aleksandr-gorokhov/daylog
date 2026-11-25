//! Причины пауз: запись причины через `daylog.py pause` и решение, когда спрашивать.
//!
//! Сами паузы и уже записанные причины читает `data.rs`. Файлы дней приложение не меняет:
//! строку в `pauses.jsonl` дописывает `daylog.py`.
//!
//! Окно с вопросом показывается, когда у сегодняшнего дня есть пауза без причины и
//! пользователь за компьютером. Здесь только состояние и правила (`Prompts`), без Tauri:
//! окно показывает и прячет `lib.rs` по результату `Prompts::refresh`.

use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::data::{self, Pause, IDLE_SEC};
use crate::procs::Runtime;
use crate::report::run_daylog;

/// Как часто фоновый поток пересчитывает вопрос.
pub const TICK: Duration = Duration::from_secs(2);
/// На сколько «Позже» откладывает вопрос.
pub const SNOOZE: Duration = Duration::from_secs(600);

/// Событие `pauses-changed`: в этом дне записана причина паузы.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PausesChanged {
    pub day: String,
}

/// Записывает причину паузы за отрезок `[start, end)` дня: `daylog.py pause <день> <начало>
/// <конец> <причина>`. Причина уходит одним аргументом как есть; время и причину проверяет
/// сама команда. Ошибка — конец её вывода.
pub fn write_reason(rt: &Runtime, day: &str, start: &str, end: &str, reason: &str) -> Result<(), String> {
    run_daylog(rt, "pause", day, &[start, end, reason])
}

/// Сколько секунд не было ввода с клавиатуры и мыши. То же значение, что `HIDIdleTime`,
/// по которому сборщик ставит пометку «нет ввода»; разрешений не требует.
pub fn idle_seconds() -> f64 {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
        }
        const HID_SYSTEM_STATE: i32 = 1; // kCGEventSourceStateHIDSystemState
        const ANY_INPUT_EVENT: u32 = u32::MAX; // kCGAnyInputEventType
        unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) }
    }
    #[cfg(not(target_os = "macos"))]
    0.0
}

/// Сегодняшний день по локальному времени, «ГГГГ-ММ-ДД».
pub fn today() -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        libc::localtime_r(&now, &mut tm);
    }
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

/// Вопрос окна: о какой паузе спрашивать.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PausePrompt {
    pub day: String,
    pub pause: Pause,
    /// Сколько ещё пауз этого дня без причины, кроме этой.
    pub remaining: usize,
    /// Недавние причины, от новых к старым.
    pub recent: Vec<String>,
}

/// «Позже»: когда нажато и какие паузы без причины тогда были (по началу паузы).
#[derive(Debug, Clone, PartialEq)]
pub struct Snooze {
    pub at: Instant,
    pub known: BTreeSet<String>,
}

/// Что делать с окном вопроса.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Window {
    Show,
    Hide,
    /// Ничего не менять: пользователя нет или вопрос отложен.
    Keep,
}

/// О какой паузе спрашивать: о самой поздней без причины. Второе значение — сколько
/// остальных пауз без причины.
pub fn question(pauses: &[Pause]) -> Option<(&Pause, usize)> {
    let without_reason = || pauses.iter().filter(|p| p.reason.is_none());
    let latest = without_reason().max_by(|a, b| a.start.cmp(&b.start))?;
    Some((latest, without_reason().count() - 1))
}

/// Показывать ли окно. `without_reason` — начала пауз сегодняшнего дня без причины.
///
/// Решение принимается по текущему состоянию, без отслеживания «ушёл → вернулся»:
/// - пауз без причины нет — окно прятать;
/// - вопрос отложен, 10 минут не прошли и новых пауз не появилось — ничего не менять;
/// - пользователь за компьютером (ввод был меньше `IDLE_SEC` назад) — показать;
/// - иначе ничего не менять: показанное окно дождётся пользователя, спрятанное не всплывёт.
pub fn decide(idle_sec: f64, without_reason: &[&str], snooze: Option<&Snooze>, now: Instant) -> Window {
    if without_reason.is_empty() {
        return Window::Hide;
    }
    if let Some(snooze) = snooze {
        let expired = now.saturating_duration_since(snooze.at) >= SNOOZE;
        let new_pause = without_reason.iter().any(|start| !snooze.known.contains(*start));
        if !expired && !new_pause {
            return Window::Keep;
        }
    }
    if idle_sec < IDLE_SEC as f64 {
        Window::Show
    } else {
        Window::Keep
    }
}

/// Что изменилось за один пересчёт; `None` в поле — менять нечего.
#[derive(Debug, Default, PartialEq)]
pub struct Update {
    /// Новый вопрос для окна или `Some(None)`, если вопроса больше нет.
    pub prompt: Option<Option<PausePrompt>>,
    /// Показать (`true`) или спрятать (`false`) окно.
    pub window: Option<bool>,
    /// Новое число пауз без причины для значка в Dock.
    pub badge: Option<usize>,
}

/// Время изменения и размер `captures.jsonl`, `minutes.jsonl` и `pauses.jsonl` дня:
/// паузы перечитываются, только когда что-то из этого изменилось.
type Stamp = [Option<(Option<SystemTime>, u64)>; 3];

fn stamp(day_dir: &Path) -> Stamp {
    ["captures.jsonl", "minutes.jsonl", "pauses.jsonl"]
        .map(|name| fs::metadata(day_dir.join(name)).ok().map(|m| (m.modified().ok(), m.len())))
}

#[derive(Default)]
struct Inner {
    /// День и отпечаток файлов, по которым прочитаны `pauses` и `recent`.
    loaded: Option<(String, Stamp)>,
    pauses: Vec<Pause>,
    recent: Vec<String>,
    /// Вопрос, который сейчас знает окно.
    prompt: Option<PausePrompt>,
    snooze: Option<Snooze>,
    visible: bool,
    badge: usize,
}

/// Состояние окна с вопросом. Живёт в памяти процесса: после перезапуска приложение
/// спрашивает сразу, отложенный вопрос не запоминается.
#[derive(Default)]
pub struct Prompts(Mutex<Inner>);

impl Prompts {
    /// Пересчитывает вопрос. `today` — сегодняшний день, `idle_sec` — секунды без ввода.
    /// Возвращает, что изменилось; вызывающий отправляет вопрос окну, показывает или
    /// прячет его и обновляет значок.
    pub fn refresh(&self, data_dir: &Path, today: &str, idle_sec: f64, now: Instant) -> Update {
        let mut s = self.0.lock().unwrap();
        let fresh = (today.to_owned(), stamp(&data_dir.join(today)));
        if s.loaded.as_ref() != Some(&fresh) {
            match data::day_pauses(data_dir, today) {
                Ok(pauses) => s.pauses = pauses,
                Err(e) => {
                    // прежние паузы остаются, чтение повторится на следующем пересчёте
                    eprintln!("daylog: паузы за {today} не прочитаны: {e}");
                    return Update::default();
                }
            }
            s.recent = data::recent_reasons(data_dir, today);
            if s.loaded.as_ref().is_some_and(|(day, _)| day != today) {
                s.snooze = None; // отложены были вчерашние паузы
            }
            s.loaded = Some(fresh);
        }

        let prompt = question(&s.pauses).map(|(pause, remaining)| PausePrompt {
            day: today.to_owned(),
            pause: pause.clone(),
            remaining,
            recent: s.recent.clone(),
        });
        let starts: Vec<&str> = s.pauses.iter().filter(|p| p.reason.is_none()).map(|p| p.start.as_str()).collect();
        let window = decide(idle_sec, &starts, s.snooze.as_ref(), now);
        let badge = starts.len();

        let mut update = Update::default();
        if prompt != s.prompt {
            s.prompt = prompt.clone();
            update.prompt = Some(prompt);
        }
        if window != Window::Keep {
            s.snooze = None;
            let show = window == Window::Show;
            if show != s.visible {
                s.visible = show;
                update.window = Some(show);
            }
        }
        if badge != s.badge {
            s.badge = badge;
            update.badge = Some(badge);
        }
        update
    }

    /// «Позже»: прячет окно и откладывает вопрос об известных сейчас паузах без причины.
    pub fn snooze(&self, now: Instant) -> Update {
        let mut s = self.0.lock().unwrap();
        let known = s.pauses.iter().filter(|p| p.reason.is_none()).map(|p| p.start.clone()).collect();
        s.snooze = Some(Snooze { at: now, known });
        Update { window: std::mem::take(&mut s.visible).then_some(false), ..Update::default() }
    }

    /// Вопрос, который сейчас должен показывать окно.
    pub fn current(&self) -> Option<PausePrompt> {
        self.0.lock().unwrap().prompt.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::resolve_python;
    use std::fs;
    use std::path::Path;

    /// Подставной daylog.py: записывает полученные аргументы в `args.json` в DAYLOG_DIR.
    const STUB: &str = r#"
import json, os, sys
with open(os.path.join(os.environ["DAYLOG_DIR"], "args.json"), "w") as f:
    json.dump(sys.argv[1:], f, ensure_ascii=False)
if sys.argv[2] == "2000-01-01":
    print("что-то в stdout")
    print("daylog: нет данных за 2000-01-01", file=sys.stderr)
    sys.exit(1)
"#;

    fn runtime(dir: &Path) -> Runtime {
        let script = dir.join("stub.py");
        fs::write(&script, STUB).unwrap();
        Runtime {
            data_dir: dir.to_owned(),
            python: resolve_python(None, |p| p.is_file()),
            script,
            path_var: std::env::var("PATH").unwrap_or_default(),
        }
    }

    fn args(dir: &Path) -> Vec<String> {
        serde_json::from_str(&fs::read_to_string(dir.join("args.json")).unwrap()).unwrap()
    }

    #[test]
    fn reason_reaches_the_command_as_one_argument() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path());
        for reason in [
            "поговорил с Игорем",
            "--help",
            "-x \"кавычки\" и 'апострофы'",
            "$(touch взлом); `id` && rm -rf ~ | * ? \\ %s",
            "  пробелы   по   краям  ",
        ] {
            assert_eq!(write_reason(&rt, "2026-11-25", "14:22", "14:30", reason), Ok(()));
            assert_eq!(args(tmp.path()), ["pause", "2026-11-25", "14:22", "14:30", reason]);
        }
        assert!(!tmp.path().join("взлом").exists());
    }

    #[test]
    fn command_failure_returns_the_end_of_its_output() {
        let tmp = tempfile::tempdir().unwrap();
        let err = write_reason(&runtime(tmp.path()), "2000-01-01", "14:22", "14:30", "кофе").unwrap_err();
        assert!(err.contains("daylog.py pause завершился с кодом 1"), "{err}");
        assert!(err.contains("нет данных за 2000-01-01"), "{err}");
    }

    #[test]
    fn bad_day_does_not_start_the_command() {
        let tmp = tempfile::tempdir().unwrap();
        let err = write_reason(&runtime(tmp.path()), "../x", "14:22", "14:30", "кофе").unwrap_err();
        assert!(err.contains("некорректный день"), "{err}");
        assert!(!tmp.path().join("args.json").exists());
    }

    /// С настоящим `daylog.py`: строка дописана, остальные файлы дня не тронуты.
    #[test]
    fn real_script_appends_one_line() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rt = runtime(tmp.path());
        rt.script = crate::procs::resolve_script(None);
        let day = tmp.path().join("2026-11-25");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("captures.jsonl"), "{\"t\": \"14:21\", \"app\": \"Arc\"}\n").unwrap();

        assert_eq!(write_reason(&rt, "2026-11-25", "14:22", "14:30", "кофе\nи  разговор"), Ok(()));
        let text = fs::read_to_string(day.join("pauses.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 1);
        let row: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(row["start"], "14:22");
        assert_eq!(row["end"], "14:30");
        assert_eq!(row["reason"], "кофе и разговор");
        assert_eq!(fs::read_to_string(day.join("captures.jsonl")).unwrap(), "{\"t\": \"14:21\", \"app\": \"Arc\"}\n");

        let err = write_reason(&rt, "2026-11-25", "14:22", "14:30", "   ").unwrap_err();
        assert!(err.contains("причина паузы пуста"), "{err}");
        assert!(write_reason(&rt, "2026-11-24", "14:22", "14:30", "кофе").unwrap_err().contains("нет данных"));
        assert_eq!(fs::read_to_string(day.join("pauses.jsonl")).unwrap(), text);
        assert!(!tmp.path().join("2026-11-24").exists());
    }

    // ---------- когда спрашивать ----------

    fn pause(start: &str, end: &str, reason: Option<&str>) -> Pause {
        Pause { start: start.into(), end: end.into(), minutes: 10, reason: reason.map(str::to_owned), before: None }
    }

    fn snoozed(at: Instant, known: &[&str]) -> Snooze {
        Snooze { at, known: known.iter().map(|s| s.to_string()).collect() }
    }

    const HERE: f64 = 3.0; // ввод был три секунды назад
    const AWAY: f64 = 400.0;

    #[test]
    fn latest_pause_without_reason_is_asked_first() {
        let pauses = [
            pause("11:46", "11:56", None),
            pause("12:20", "12:45", Some("обед")),
            pause("13:12", "13:20", None),
            pause("14:22", "14:30", None),
            pause("15:00", "15:10", Some("кофе")),
        ];
        let (asked, remaining) = question(&pauses).unwrap();
        assert_eq!((asked.start.as_str(), remaining), ("14:22", 2));

        assert_eq!(question(&pauses[..2]).map(|(p, n)| (p.start.as_str(), n)), Some(("11:46", 0)));
        assert_eq!(question(&[pause("12:20", "12:45", Some("обед"))]), None);
        assert_eq!(question(&[]), None);
    }

    #[test]
    fn window_follows_presence_and_pauses() {
        let now = Instant::now();
        assert_eq!(decide(HERE, &["14:22"], None, now), Window::Show);
        // пользователь не за компьютером: окно не всплывает, но и не прячется
        assert_eq!(decide(AWAY, &["14:22"], None, now), Window::Keep);
        assert_eq!(decide(IDLE_SEC as f64, &["14:22"], None, now), Window::Keep, "ровно 5 минут без ввода");
        assert_eq!(decide(IDLE_SEC as f64 - 0.5, &["14:22"], None, now), Window::Show);
        // пауз без причины нет: окно прячется, есть пользователь или нет
        assert_eq!(decide(HERE, &[], None, now), Window::Hide);
        assert_eq!(decide(AWAY, &[], Some(&snoozed(now, &["14:22"])), now), Window::Hide);
    }

    #[test]
    fn snooze_holds_for_ten_minutes() {
        let at = Instant::now();
        let snooze = snoozed(at, &["13:12", "14:22"]);
        let after = |secs: u64| at + Duration::from_secs(secs);
        assert_eq!(decide(HERE, &["13:12", "14:22"], Some(&snooze), after(1)), Window::Keep);
        assert_eq!(decide(HERE, &["13:12", "14:22"], Some(&snooze), after(599)), Window::Keep);
        assert_eq!(decide(HERE, &["13:12", "14:22"], Some(&snooze), after(600)), Window::Show);
        // десять минут прошли, а пользователя нет: окно ждёт его возвращения
        assert_eq!(decide(AWAY, &["13:12", "14:22"], Some(&snooze), after(900)), Window::Keep);
        // часть причин записана в списке главного окна: отложенное остаётся отложенным
        assert_eq!(decide(HERE, &["13:12"], Some(&snooze), after(60)), Window::Keep);
    }

    #[test]
    fn new_pause_ends_the_snooze() {
        let at = Instant::now();
        let snooze = snoozed(at, &["14:22"]);
        let soon = at + Duration::from_secs(60);
        // появилась пауза с новым началом: спрашиваем, как только пользователь вернулся
        assert_eq!(decide(AWAY, &["14:22", "14:32"], Some(&snooze), soon), Window::Keep);
        assert_eq!(decide(HERE, &["14:22", "14:32"], Some(&snooze), soon), Window::Show);
        // отложенная пауза выросла, начало прежнее: это не новая пауза
        assert_eq!(decide(HERE, &["14:22"], Some(&snooze), soon), Window::Keep);
    }

    #[test]
    fn today_matches_the_system_date() {
        let out = std::process::Command::new("date").arg("+%Y-%m-%d").output().unwrap();
        assert_eq!(today(), String::from_utf8_lossy(&out.stdout).trim());
        assert!(data::is_valid_day(&today()));
    }

    /// Простой по `ioreg`, как его читает сборщик; `None`, если `HIDIdleTime` недоступен.
    fn ioreg_idle_seconds() -> Option<f64> {
        let out = std::process::Command::new("ioreg").args(["-c", "IOHIDSystem"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let rest = &text[text.find("\"HIDIdleTime\" = ")? + 16..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        Some(digits.parse::<f64>().ok()? / 1e9)
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn idle_seconds_matches_the_collector_source() {
        // Значения читаются с разницей в миллисекунды; ввод между чтениями сбивает сравнение,
        // поэтому до трёх попыток.
        let mut seen = Vec::new();
        for _ in 0..3 {
            let Some(collector) = ioreg_idle_seconds() else { return };
            let app = idle_seconds();
            assert!(app >= 0.0, "{app}");
            if (app - collector).abs() < 1.0 {
                return;
            }
            seen.push((app, collector));
        }
        panic!("простой у приложения и у сборщика расходится больше чем на секунду: {seen:?}");
    }

    // ---------- пересчёт вопроса по файлам дня ----------

    fn row(t: &str, idle: bool) -> String {
        let flag = if idle { ", \"idle\": true" } else { "" };
        format!("{{\"t\": \"{t}\", \"app\": \"Arc\", \"frames\": []{flag}}}\n")
    }

    /// Минуты часа `hour`: `10:00`, `10:01`, …; `idle` — номера минут «нет ввода».
    fn captures(hour: u8, minutes: std::ops::RangeInclusive<u8>, idle: &[std::ops::RangeInclusive<u8>]) -> String {
        minutes.map(|m| row(&format!("{hour:02}:{m:02}"), idle.iter().any(|r| r.contains(&m)))).collect()
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    fn reason(start: &str, end: &str, text: &str) -> String {
        format!("{{\"start\": \"{start}\", \"end\": \"{end}\", \"reason\": \"{text}\", \"at\": \"x\"}}\n")
    }

    /// Каталог данных с днём 2026-11-25: паузы 10:01–10:06 и 10:10–10:16, перед первой описана 10:00.
    fn data_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let day = tmp.path().join("2026-11-25");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("captures.jsonl"), captures(10, 0..=16, &[1..=5, 10..=15])).unwrap();
        fs::write(day.join("minutes.jsonl"), "{\"t\": \"10:00\", \"app\": \"Arc\", \"activity\": \"смотрел PR\", \"category\": \"work\"}\n").unwrap();
        tmp
    }

    fn asked(update: &Update) -> Option<(&str, &str, usize)> {
        let prompt = update.prompt.as_ref().expect("вопрос изменился").as_ref()?;
        Some((prompt.pause.start.as_str(), prompt.pause.end.as_str(), prompt.remaining))
    }

    #[test]
    fn question_follows_the_reasons_file() {
        let tmp = data_dir();
        let day = tmp.path().join("2026-11-25");
        let prompts = Prompts::default();
        let now = Instant::now();
        let refresh = || prompts.refresh(tmp.path(), "2026-11-25", HERE, now);
        assert_eq!(prompts.current(), None);

        // вопрос о самой поздней паузе, окно показано, на значке 2
        let update = refresh();
        assert_eq!(asked(&update), Some(("10:10", "10:16", 1)));
        assert_eq!((update.window, update.badge), (Some(true), Some(2)));
        let prompt = prompts.current().unwrap();
        assert_eq!((prompt.day.as_str(), prompt.pause.before.as_deref()), ("2026-11-25", Some("смотрел PR")));
        assert_eq!(prompt.recent, Vec::<String>::new());

        // ничего не изменилось — менять нечего
        assert_eq!(refresh(), Update::default());

        // причина записана: вопрос о следующей паузе, окно остаётся показанным
        append(&day.join("pauses.jsonl"), &reason("10:10", "10:16", "кофе"));
        let update = refresh();
        assert_eq!(asked(&update), Some(("10:01", "10:06", 0)));
        assert_eq!((update.window, update.badge), (None, Some(1)));
        assert_eq!(prompts.current().unwrap().recent, ["кофе"]);

        // записаны все причины: вопроса нет, окно прячется, число со значка пропадает
        append(&day.join("pauses.jsonl"), &reason("10:01", "10:06", "устал"));
        let update = refresh();
        assert_eq!(asked(&update), None);
        assert_eq!((update.window, update.badge), (Some(false), Some(0)));
        assert_eq!(prompts.current(), None);
        assert_eq!(refresh(), Update::default());
    }

    #[test]
    fn question_moves_to_the_new_day() {
        let tmp = data_dir();
        let prompts = Prompts::default();
        let now = Instant::now();
        assert_eq!(asked(&prompts.refresh(tmp.path(), "2026-11-25", HERE, now)), Some(("10:10", "10:16", 1)));

        // наступил новый день: его каталога ещё нет — вчерашние паузы не спрашиваются
        let update = prompts.refresh(tmp.path(), "2026-11-26", HERE, now);
        assert_eq!(asked(&update), None);
        assert_eq!((update.window, update.badge), (Some(false), Some(0)));

        // в новом дне появилась пауза
        let day = tmp.path().join("2026-11-26");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("captures.jsonl"), captures(9, 0..=8, &[1..=7])).unwrap();
        let update = prompts.refresh(tmp.path(), "2026-11-26", HERE, now);
        assert_eq!(asked(&update), Some(("09:01", "09:08", 0)));
        assert_eq!(prompts.current().unwrap().day, "2026-11-26");
        assert_eq!((update.window, update.badge), (Some(true), Some(1)));
    }

    #[test]
    fn window_waits_for_the_user() {
        let tmp = data_dir();
        let prompts = Prompts::default();
        let now = Instant::now();
        // пауза в данных есть, но пользователь ещё не вернулся: вопрос готов, окно не показано
        let update = prompts.refresh(tmp.path(), "2026-11-25", AWAY, now);
        assert_eq!(asked(&update), Some(("10:10", "10:16", 1)));
        assert_eq!((update.window, update.badge), (None, Some(2)));
        // появился ввод
        let update = prompts.refresh(tmp.path(), "2026-11-25", HERE, now);
        assert_eq!(update, Update { window: Some(true), ..Update::default() });
        // снова отошёл: показанное окно остаётся на экране
        assert_eq!(prompts.refresh(tmp.path(), "2026-11-25", AWAY, now), Update::default());
    }

    #[test]
    fn snooze_hides_until_time_passes_or_a_new_pause_ends() {
        let tmp = data_dir();
        let day = tmp.path().join("2026-11-25");
        let prompts = Prompts::default();
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let refresh = |idle: f64, now: Instant| prompts.refresh(tmp.path(), "2026-11-25", idle, now);
        assert_eq!(refresh(HERE, t0).window, Some(true));

        // «Позже»: окно спрятано; повторное «Позже» прятать уже нечего
        assert_eq!(prompts.snooze(at(0)), Update { window: Some(false), ..Update::default() });
        assert_eq!(prompts.snooze(at(0)), Update::default());
        assert_eq!(refresh(HERE, at(300)), Update::default());
        // через 10 минут окно возвращается
        assert_eq!(refresh(HERE, at(600)), Update { window: Some(true), ..Update::default() });

        // снова «Позже», и через минуту заканчивается новая пауза 10:18–10:24
        assert_eq!(prompts.snooze(at(700)).window, Some(false));
        append(&day.join("captures.jsonl"), &captures(10, 17..=24, &[18..=23]));
        let update = refresh(AWAY, at(760));
        assert_eq!(asked(&update), Some(("10:18", "10:24", 2)));
        assert_eq!((update.window, update.badge), (None, Some(3)), "пользователь ещё не вернулся");
        assert_eq!(refresh(HERE, at(770)), Update { window: Some(true), ..Update::default() });

        // «Позже», затем все причины записаны в списке главного окна: окно больше не появится
        assert_eq!(prompts.snooze(at(800)).window, Some(false));
        for (start, end) in [("10:01", "10:06"), ("10:10", "10:16"), ("10:18", "10:24")] {
            append(&day.join("pauses.jsonl"), &reason(start, end, "кофе"));
        }
        let update = refresh(HERE, at(810));
        assert_eq!(asked(&update), None);
        assert_eq!((update.window, update.badge), (None, Some(0)), "окно и так спрятано");
        assert_eq!(refresh(HERE, at(5000)), Update::default());
    }

    #[test]
    fn missing_day_or_data_dir_means_no_question() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts = Prompts::default();
        let now = Instant::now();
        assert_eq!(prompts.refresh(tmp.path(), "2026-11-25", HERE, now), Update::default());
        assert_eq!(prompts.refresh(&tmp.path().join("нет"), "2026-11-25", HERE, now), Update::default());
        assert_eq!(prompts.current(), None);
    }
}
