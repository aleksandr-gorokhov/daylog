//! Чтение каталога данных daylog. Только чтение и без зависимостей от Tauri.
//!
//! Каталог данных: `$DAYLOG_DIR`, иначе `~/.local/share/daylog`; внутри по каталогу на день
//! `YYYY-MM-DD/` с `captures.jsonl`, `minutes.jsonl`, (после сборки) `report.json` и
//! (после первой записанной причины паузы) `pauses.jsonl`.
//!
//! Паузы дня нигде не хранятся: они вычисляются при чтении из минут «нет ввода» и пропусков.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Каталог данных по значениям `DAYLOG_DIR` и `HOME`.
pub fn data_dir_from(daylog_dir: Option<String>, home: Option<String>) -> PathBuf {
    match daylog_dir.filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.unwrap_or_default()).join(".local/share/daylog"),
    }
}

pub fn data_dir_from_env() -> PathBuf {
    data_dir_from(std::env::var("DAYLOG_DIR").ok(), std::env::var("HOME").ok())
}

/// `^\d{4}-\d{2}-\d{2}$`, только ASCII-цифры.
pub fn is_valid_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

pub fn check_day(day: &str) -> Result<(), String> {
    if is_valid_day(day) {
        Ok(())
    } else {
        Err(format!("некорректный день: {day:?}, нужен вид ГГГГ-ММ-ДД"))
    }
}

/// `ЧЧ:ММ`.
fn is_valid_minute(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && [0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit())
        && t[..2] < *"24"
        && t[3..] < *"60"
}

/// «ЧЧ:ММ» → номер минуты от полуночи. Только для времени, прошедшего проверку.
fn minute_no(t: &str) -> u16 {
    let b = t.as_bytes();
    let digit = |i: usize| u16::from(b[i] - b'0');
    (digit(0) * 10 + digit(1)) * 60 + digit(3) * 10 + digit(4)
}

/// Номер минуты → «ЧЧ:ММ»; 1440 — «24:00», конец паузы, которая идёт до полуночи.
fn minute_str(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Конец отрезка: «ЧЧ:ММ» или «24:00».
fn is_valid_end(t: &str) -> bool {
    is_valid_minute(t) || t == "24:00"
}

/// Строки JSONL; пустые и битые строки (например, недописанная последняя) пропускаются.
fn read_jsonl<T: DeserializeOwned>(path: &Path) -> io::Result<Vec<T>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Сколько секунд без ввода делают минуты паузой: `IDLE_SEC` в `daylog.py`, менять вместе.
pub const IDLE_SEC: u64 = 300;

/// Пропуск в записях без единой минуты «нет ввода» — пауза, только если он не короче.
const MIN_GAP_PAUSE: u16 = 5;

#[derive(Deserialize)]
struct CaptureRow {
    t: String,
    #[serde(default)]
    app: String,
    /// `true`, если на снимке без ввода набралось `IDLE_SEC`; так же помечены строки без `idle_sec`.
    #[serde(default)]
    idle: Option<serde_json::Value>,
    /// Секунды с последнего ввода на момент снимка; в старых строках поля нет.
    #[serde(default)]
    idle_sec: Option<serde_json::Value>,
}

impl CaptureRow {
    fn idle(&self) -> bool {
        self.idle.as_ref().and_then(serde_json::Value::as_bool) == Some(true)
    }

    fn idle_sec(&self) -> u64 {
        self.idle_sec.as_ref().and_then(serde_json::Value::as_f64).filter(|s| *s >= 0.0).map_or(0, |s| s as u64)
    }
}

/// Номера минут «нет ввода» по спеке `minute-capture`, тем же правилом, что `pending()`
/// в `daylog.py`: минута строки с `"idle": true` и минуты, покрытые строкой с
/// `idle_sec >= IDLE_SEC`, — она сама и `idle_sec / 60` минут перед ней, не раньше 00:00.
/// Минута помечена, если её покрывает хотя бы одна строка: у минуты бывает две строки.
fn no_input_minutes(captures: &[CaptureRow]) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    for c in captures {
        let m = minute_no(&c.t);
        if c.idle() {
            out.insert(m);
        }
        let sec = c.idle_sec();
        if sec >= IDLE_SEC {
            let back = (sec / 60).min(u64::from(m)) as u16;
            out.extend(m - back..=m);
        }
    }
    out
}

/// Описание, которое `daylog.py` пишет, если LLM не вернул минуту. Такие минуты не размечаются.
pub const NO_DESCRIPTION: &str = "(нет описания)";

/// Категория минуты или интервала.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Work,
    Nonwork,
    Idle,
}

impl Category {
    /// Значение поля `category` из JSONL; нет поля, не строка или неизвестное значение — `None`.
    fn parse(v: Option<&serde_json::Value>) -> Option<Category> {
        match v.and_then(serde_json::Value::as_str) {
            Some("work") => Some(Category::Work),
            Some("nonwork") => Some(Category::Nonwork),
            Some("idle") => Some(Category::Idle),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct MinuteRow {
    t: String,
    #[serde(default)]
    app: String,
    #[serde(default)]
    activity: String,
    #[serde(default)]
    category: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct IntervalRow {
    start: String,
    end: String,
    activity: String,
    #[serde(default)]
    category: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Interval {
    pub start: String,
    pub end: String,
    pub activity: String,
    /// `None` — интервал «не за компьютером» или отчёт собран до появления категорий.
    pub category: Option<Category>,
}

#[derive(Deserialize)]
struct ReportFile {
    generated_at: String,
    minutes_count: u64,
    intervals: Vec<IntervalRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Minute {
    pub t: String,
    pub app: String,
    /// `None` — минута снята, но ещё не описана.
    pub activity: Option<String>,
    /// `None` — минута не описана или описана без категории.
    pub category: Option<Category>,
}

impl Minute {
    /// Описана, но без категории: такую минуту размечает `daylog.py classify`.
    fn needs_category(&self) -> bool {
        self.category.is_none() && self.activity.as_deref().is_some_and(|a| !a.is_empty() && a != NO_DESCRIPTION)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub generated_at: String,
    pub minutes_count: u64,
    pub intervals: Vec<Interval>,
    /// Описанных минут сейчас больше, чем было при сборке.
    pub stale: bool,
}

/// Пауза дня: непрерывный отрезок из минут «нет ввода» и незаписанных минут.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pause {
    /// Первая минута паузы.
    pub start: String,
    /// Минута после последней; «24:00», если пауза идёт до полуночи.
    pub end: String,
    pub minutes: usize,
    /// Причина из `pauses.jsonl`; `None` — пауза без причины.
    pub reason: Option<String>,
    /// Описание последней описанной минуты перед паузой.
    pub before: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Day {
    pub day: String,
    /// Время за компьютером: уникальные минуты в `captures.jsonl`.
    pub active_minutes: usize,
    /// Описанные минуты без категории (кроме «(нет описания)»): их нужно доразметить.
    pub unclassified: usize,
    pub minutes: Vec<Minute>,
    /// Паузы дня по возрастанию времени, без созвонов.
    pub pauses: Vec<Pause>,
    pub report: Option<Report>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DaySummary {
    pub day: String,
    pub active_minutes: usize,
    /// Минуты с категорией «рабочее».
    pub work_minutes: usize,
}

#[derive(Deserialize)]
struct PauseRow {
    start: String,
    end: String,
    reason: String,
}

/// Причина из `pauses.jsonl`: отрезок `[start, end)` в номерах минут и текст.
struct PauseReason {
    start: u16,
    end: u16,
    reason: String,
}

/// Причины дня в порядке записи. Строки с некорректным временем, с началом не раньше
/// конца и с пустой причиной пропускаются. Причина относится к паузе, с отрезком которой
/// пересекается её отрезок; из нескольких действует последняя (см. `pauses`).
fn read_reasons(dir: &Path) -> io::Result<Vec<PauseReason>> {
    Ok(read_jsonl::<PauseRow>(&dir.join("pauses.jsonl"))?
        .into_iter()
        .filter(|r| is_valid_minute(&r.start) && is_valid_end(&r.end))
        .map(|r| PauseReason { start: minute_no(&r.start), end: minute_no(&r.end), reason: r.reason.trim().to_owned() })
        .filter(|r| r.start < r.end && !r.reason.is_empty())
        .collect())
}

fn read_captures(dir: &Path) -> io::Result<Vec<CaptureRow>> {
    Ok(read_jsonl::<CaptureRow>(&dir.join("captures.jsonl"))?
        .into_iter()
        .filter(|r| is_valid_minute(&r.t))
        .collect())
}

fn read_minutes(dir: &Path) -> io::Result<Vec<MinuteRow>> {
    Ok(read_jsonl::<MinuteRow>(&dir.join("minutes.jsonl"))?
        .into_iter()
        .filter(|r| is_valid_minute(&r.t))
        .collect())
}

fn active_minutes(captures: &[CaptureRow]) -> usize {
    captures.iter().map(|c| c.t.as_str()).collect::<BTreeSet<_>>().len()
}

/// Объединение `captures.jsonl` и `minutes.jsonl` по `t`, по возрастанию времени; дубли — последняя строка.
fn merge(captures: &[CaptureRow], described: &[MinuteRow]) -> Vec<Minute> {
    let mut by_t: BTreeMap<String, Minute> = BTreeMap::new();
    for c in captures {
        by_t.insert(c.t.clone(), Minute { t: c.t.clone(), app: c.app.clone(), activity: None, category: None });
    }
    for m in described {
        let app = if m.app.is_empty() {
            by_t.get(&m.t).map(|x| x.app.clone()).unwrap_or_default()
        } else {
            m.app.clone()
        };
        let category = Category::parse(m.category.as_ref());
        by_t.insert(m.t.clone(), Minute { t: m.t.clone(), app, activity: Some(m.activity.clone()), category });
    }
    by_t.into_values().collect()
}

/// Описание минуты, если оно настоящее: не пустое и не «(нет описания)».
fn real_activity(m: &Minute) -> Option<&str> {
    m.activity.as_deref().filter(|a| !a.is_empty() && *a != NO_DESCRIPTION)
}

/// Паузы дня. `minutes` — записанные минуты по возрастанию времени (результат `merge`).
///
/// Каждая минута от первой до последней записанной — обычная, «нет ввода» или не записана.
/// Пауза — наибольший непрерывный отрезок из минут второго и третьего вида, в котором есть
/// минута «нет ввода» или который не короче `MIN_GAP_PAUSE`. Отрезок — созвон, а не пауза,
/// если среди его минут «нет ввода» с категорией не меньше половины «рабочее» и «нерабочее».
fn pauses(captures: &[CaptureRow], minutes: &[Minute], reasons: &[PauseReason]) -> Vec<Pause> {
    struct Run {
        start: u16,
        end: u16,
        no_input: usize,
        calls: usize,
        idle: usize,
    }

    let no_input = no_input_minutes(captures);
    let by_no: BTreeMap<u16, &Minute> = minutes.iter().map(|m| (minute_no(&m.t), m)).collect();
    let (Some(&first), Some(&last)) = (by_no.keys().next(), by_no.keys().next_back()) else {
        return Vec::new();
    };

    let mut runs: Vec<Run> = Vec::new();
    let mut open: Option<Run> = None;
    for m in first..=last {
        let recorded = by_no.get(&m);
        if recorded.is_some() && !no_input.contains(&m) {
            runs.extend(open.take());
            continue;
        }
        let run = open.get_or_insert(Run { start: m, end: m, no_input: 0, calls: 0, idle: 0 });
        run.end = m + 1;
        if let Some(minute) = recorded {
            run.no_input += 1;
            match minute.category {
                Some(Category::Idle) => run.idle += 1,
                Some(_) => run.calls += 1,
                None => {}
            }
        }
    }
    runs.extend(open);

    runs.into_iter()
        .filter(|r| r.no_input > 0 || r.end - r.start >= MIN_GAP_PAUSE)
        .filter(|r| r.calls == 0 || r.calls * 2 < r.calls + r.idle)
        .map(|r| Pause {
            start: minute_str(r.start),
            end: minute_str(r.end),
            minutes: usize::from(r.end - r.start),
            reason: reasons.iter().rev().find(|x| x.start < r.end && r.start < x.end).map(|x| x.reason.clone()),
            before: minutes
                .iter()
                .rev()
                .filter(|m| minute_no(&m.t) < r.start)
                .find_map(real_activity)
                .map(str::to_owned),
        })
        .collect()
}

/// Дни в каталоге данных, новые сверху. Нет каталога — пустой список.
pub fn list_days(data_dir: &Path) -> io::Result<Vec<DaySummary>> {
    let entries = match fs::read_dir(data_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut days = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if !is_valid_day(&name) || !entry.path().is_dir() {
            continue;
        }
        let dir = entry.path();
        let captures = read_captures(&dir)?;
        let minutes = merge(&captures, &read_minutes(&dir)?);
        days.push(DaySummary {
            active_minutes: active_minutes(&captures),
            work_minutes: minutes.iter().filter(|m| m.category == Some(Category::Work)).count(),
            day: name,
        });
    }
    days.sort_by(|a, b| b.day.cmp(&a.day));
    Ok(days)
}

/// Минуты дня: объединение `captures.jsonl` и `minutes.jsonl` по `t`, по возрастанию времени.
pub fn get_day(data_dir: &Path, day: &str) -> Result<Day, String> {
    check_day(day)?;
    let dir = data_dir.join(day);
    if !dir.is_dir() {
        return Err(format!("нет данных за {day}"));
    }
    let err = |e: io::Error| format!("не удалось прочитать {day}: {e}");
    let captures = read_captures(&dir).map_err(err)?;
    let described = read_minutes(&dir).map_err(err)?;
    let minutes = merge(&captures, &described);
    let pauses = pauses(&captures, &minutes, &read_reasons(&dir).map_err(err)?);

    let report = read_report(&dir, described.len() as u64);
    Ok(Day {
        day: day.to_owned(),
        active_minutes: active_minutes(&captures),
        unclassified: minutes.iter().filter(|m| m.needs_category()).count(),
        minutes,
        pauses,
        report,
    })
}

/// Паузы дня для окна с вопросом. Каталога дня ещё нет — пауз нет.
pub fn day_pauses(data_dir: &Path, day: &str) -> Result<Vec<Pause>, String> {
    check_day(day)?;
    if !data_dir.join(day).is_dir() {
        return Ok(Vec::new());
    }
    get_day(data_dir, day).map(|d| d.pauses)
}

/// `report.json` дня; нет файла или он не читается — отчёта нет.
fn read_report(dir: &Path, described_now: u64) -> Option<Report> {
    let text = fs::read_to_string(dir.join("report.json")).ok()?;
    let file: ReportFile = serde_json::from_str(&text).ok()?;
    Some(Report {
        stale: described_now > file.minutes_count,
        generated_at: file.generated_at,
        minutes_count: file.minutes_count,
        intervals: file
            .intervals
            .into_iter()
            .map(|i| Interval {
                category: Category::parse(i.category.as_ref()),
                start: i.start,
                end: i.end,
                activity: i.activity,
            })
            .collect(),
    })
}

/// Число дней от 1970-01-01 до даты григорианского календаря (алгоритм Хиннанта).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// Обратное к `days_from_civil`: (год, месяц, день).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// День за `days` дней до `day`; `None`, если `day` не вида `ГГГГ-ММ-ДД`.
fn day_minus(day: &str, days: i64) -> Option<String> {
    if !is_valid_day(day) {
        return None;
    }
    let num = |range: std::ops::Range<usize>| day[range].parse::<i64>().ok();
    let (y, m, d) = civil_from_days(days_from_civil(num(0..4)?, num(5..7)?, num(8..10)?) - days);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// За сколько календарных дней, считая сегодняшний, берутся недавние причины.
pub const RECENT_DAYS: i64 = 14;
/// Сколько недавних причин предлагается.
pub const RECENT_LIMIT: usize = 6;

/// Последние разные причины пауз из `pauses.jsonl` дня `today` и `RECENT_DAYS - 1` дней
/// перед ним, от новых к старым, не больше `RECENT_LIMIT`. Причины сравниваются как строки
/// после обрезки пробелов. Нет каталога данных или некорректный день — пустой список.
pub fn recent_reasons(data_dir: &Path, today: &str) -> Vec<String> {
    let (Some(from), Ok(entries)) = (day_minus(today, RECENT_DAYS - 1), fs::read_dir(data_dir)) else {
        return Vec::new();
    };
    let mut days: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|name| is_valid_day(name) && name.as_str() >= from.as_str() && name.as_str() <= today)
        .collect();
    days.sort_by(|a, b| b.cmp(a));

    let mut out: Vec<String> = Vec::new();
    for day in days {
        for r in read_reasons(&data_dir.join(day)).unwrap_or_default().into_iter().rev() {
            if out.len() < RECENT_LIMIT && !out.contains(&r.reason) {
                out.push(r.reason);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    fn cap(t: &str, app: &str) -> String {
        format!("{{\"t\": \"{t}\", \"app\": \"{app}\", \"frames\": [\"x.jpg\"]}}\n")
    }

    fn min(t: &str, app: &str, activity: &str) -> String {
        format!("{{\"t\": \"{t}\", \"app\": \"{app}\", \"activity\": \"{activity}\"}}\n")
    }

    #[test]
    fn data_dir_prefers_env() {
        assert_eq!(data_dir_from(Some("/x".into()), Some("/h".into())), PathBuf::from("/x"));
        assert_eq!(
            data_dir_from(None, Some("/h".into())),
            PathBuf::from("/h/.local/share/daylog")
        );
        assert_eq!(
            data_dir_from(Some(String::new()), Some("/h".into())),
            PathBuf::from("/h/.local/share/daylog")
        );
    }

    #[test]
    fn day_name_validation() {
        assert!(is_valid_day("2026-11-16"));
        for bad in ["2026-1-16", "../x", "2026-11-16/..", "notes", "２０２６-11-16", "2026_11_16", ""] {
            assert!(!is_valid_day(bad), "{bad}");
        }
    }

    #[test]
    fn merges_sorts_and_marks_undescribed() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &[cap("14:10", "Arc"), cap("14:29", "Warp"), cap("14:44", "Slack")].concat());
        // 14:29 записана раньше 14:10
        write(&d, "minutes.jsonl", &[min("14:29", "Warp", "терминал"), min("14:10", "Arc", "читал PR")].concat());

        let day = get_day(tmp.path(), "2026-11-16").unwrap();
        let ts: Vec<_> = day.minutes.iter().map(|m| m.t.as_str()).collect();
        assert_eq!(ts, ["14:10", "14:29", "14:44"]);
        assert_eq!(day.minutes[0].activity.as_deref(), Some("читал PR"));
        assert_eq!(day.minutes[2], Minute { t: "14:44".into(), app: "Slack".into(), activity: None, category: None });
        assert_eq!(day.active_minutes, 3);
        assert_eq!(day.report, None);
    }

    #[test]
    fn duplicates_last_wins_and_app_from_minutes() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &[cap("09:00", "Arc"), cap("09:00", "Warp")].concat());
        write(&d, "minutes.jsonl", &[min("09:00", "Arc", "первое"), min("09:00", "Zed", "второе")].concat());

        let day = get_day(tmp.path(), "2026-11-16").unwrap();
        assert_eq!(
            day.minutes,
            vec![Minute { t: "09:00".into(), app: "Zed".into(), activity: Some("второе".into()), category: None }]
        );
        assert_eq!(day.active_minutes, 1);
    }

    fn cmin(t: &str, activity: &str, category: &str) -> String {
        format!("{{\"t\": \"{t}\", \"app\": \"Arc\", \"activity\": \"{activity}\", \"category\": {category}}}\n")
    }

    #[test]
    fn minute_categories_and_unclassified() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        let caps: String = ["10:00", "10:01", "10:02", "10:03", "10:04", "10:05", "10:06", "10:07"]
            .iter()
            .map(|t| cap(t, "Arc"))
            .collect();
        write(&d, "captures.jsonl", &caps);
        let mins = [
            cmin("10:00", "PR", "\"work\""),
            cmin("10:01", "telegram", "\"nonwork\""),
            cmin("10:02", "экран блокировки", "\"idle\""),
            min("10:03", "Arc", "старая строка без категории"),
            cmin("10:04", "что-то", "\"bogus\""),
            cmin("10:05", "что-то", "5"),
            min("10:06", "Arc", NO_DESCRIPTION),
            // 10:07 снята, но не описана
        ]
        .concat();
        write(&d, "minutes.jsonl", &mins);

        let day = get_day(tmp.path(), "2026-11-16").unwrap();
        let cats: Vec<_> = day.minutes.iter().map(|m| m.category).collect();
        use Category::*;
        assert_eq!(cats, [Some(Work), Some(Nonwork), Some(Idle), None, None, None, None, None]);
        // без категории: старая строка, неизвестное значение, не строка; «(нет описания)» и неописанная — нет
        assert_eq!(day.unclassified, 3);
        assert_eq!(day.minutes.len(), 8, "строка с category = 5 не отброшена");

        let json = serde_json::to_value(&day.minutes[0]).unwrap();
        assert_eq!(json["category"], "work");
        assert_eq!(serde_json::to_value(&day.minutes[3]).unwrap()["category"], serde_json::Value::Null);
    }

    #[test]
    fn work_minutes_in_list_days() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &[cap("10:00", "Arc"), cap("10:01", "Arc"), cap("10:02", "Arc")].concat());
        // дубль 10:00: побеждает последняя строка, поэтому рабочая минута одна
        write(
            &d,
            "minutes.jsonl",
            &[cmin("10:00", "a", "\"work\""), cmin("10:00", "b", "\"nonwork\""), cmin("10:01", "c", "\"work\"")].concat(),
        );
        let days = list_days(tmp.path()).unwrap();
        assert_eq!(days[0].work_minutes, 1);
        assert_eq!(days[0].active_minutes, 3);
    }

    #[test]
    fn interval_categories() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &cap("09:00", "Arc"));
        let report = r#"{"day": "2026-11-16", "generated_at": "2026-11-16T15:02:11", "minutes_count": 0, "intervals": [
            {"start": "09:00", "end": "09:30", "activity": "PR", "category": "work"},
            {"start": "09:30", "end": "09:40", "activity": "не за компьютером", "category": null},
            {"start": "09:40", "end": "09:50", "activity": "старый отчёт"},
            {"start": "09:50", "end": "10:00", "activity": "?", "category": "away"}]}"#;
        write(&d, "report.json", report);
        let cats: Vec<_> =
            get_day(tmp.path(), "2026-11-16").unwrap().report.unwrap().intervals.iter().map(|i| i.category).collect();
        assert_eq!(cats, [Some(Category::Work), None, None, None]);
    }

    #[test]
    fn broken_line_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &[cap("09:00", "Arc"), cap("09:01", "Arc")].concat());
        write(&d, "minutes.jsonl", &format!("{}{{\"t\": \"09:01\", \"app\": \"Ar", min("09:00", "Arc", "читал")));

        let day = get_day(tmp.path(), "2026-11-16").unwrap();
        assert_eq!(day.minutes.len(), 2);
        assert_eq!(day.minutes[0].activity.as_deref(), Some("читал"));
        assert_eq!(day.minutes[1].activity, None);
    }

    #[test]
    fn day_without_minutes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &cap("09:00", "Arc"));

        let day = get_day(tmp.path(), "2026-11-16").unwrap();
        assert_eq!(day.minutes, vec![Minute { t: "09:00".into(), app: "Arc".into(), activity: None, category: None }]);
    }

    #[test]
    fn report_stale_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &[cap("09:00", "Arc"), cap("09:01", "Arc")].concat());
        write(&d, "minutes.jsonl", &min("09:00", "Arc", "читал"));
        let report = r#"{"day": "2026-11-16", "generated_at": "2026-11-16T15:02:11", "minutes_count": 1,
            "intervals": [{"start": "09:00", "end": "09:01", "activity": "читал"}]}"#;
        write(&d, "report.json", report);

        let r = get_day(tmp.path(), "2026-11-16").unwrap().report.unwrap();
        assert!(!r.stale);
        assert_eq!(r.generated_at, "2026-11-16T15:02:11");
        assert_eq!(r.intervals.len(), 1);

        fs::write(d.join("minutes.jsonl"), [min("09:00", "Arc", "читал"), min("09:01", "Arc", "писал")].concat()).unwrap();
        assert!(get_day(tmp.path(), "2026-11-16").unwrap().report.unwrap().stale);
    }

    #[test]
    fn report_md_only_means_no_report() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &cap("09:00", "Arc"));
        write(&d, "report.md", "# 2026-11-16\n");
        assert_eq!(get_day(tmp.path(), "2026-11-16").unwrap().report, None);
    }

    #[test]
    fn list_days_filters_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("2026-11-15"), "captures.jsonl", &[cap("09:00", "Arc"), cap("09:00", "Arc"), cap("09:01", "Arc")].concat());
        write(&tmp.path().join("2026-11-16"), "report.json", "{}");
        write(&tmp.path().join("notes"), "x", "");
        write(&tmp.path().join("2026-1-1"), "x", "");
        write(tmp.path(), "2026-11-17", "файл, а не каталог");
        write(tmp.path(), "collector.lock", "123");

        let days = list_days(tmp.path()).unwrap();
        assert_eq!(
            days,
            vec![
                DaySummary { day: "2026-11-16".into(), active_minutes: 0, work_minutes: 0 },
                DaySummary { day: "2026-11-15".into(), active_minutes: 2, work_minutes: 0 },
            ]
        );
    }

    #[test]
    fn missing_data_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(list_days(&tmp.path().join("нет")).unwrap(), vec![]);
    }

    #[test]
    fn bad_day_is_rejected_before_disk() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("x"), "captures.jsonl", &cap("09:00", "Arc"));
        assert!(get_day(&tmp.path().join("2026-11-16"), "../x").is_err());
        assert!(get_day(tmp.path(), "2026-11-16").is_err());
    }

    #[test]
    fn reading_does_not_touch_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("2026-11-16");
        write(&d, "captures.jsonl", &cap("09:00", "Arc"));
        let before: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        list_days(tmp.path()).unwrap();
        get_day(tmp.path(), "2026-11-16").unwrap();
        let after: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(before, after);
    }

    // ---------- минуты «нет ввода» ----------

    /// Строка нового формата: `idle_sec` и, от порога, `"idle": true`.
    fn capi(t: &str, idle_sec: u64) -> String {
        let idle = if idle_sec >= IDLE_SEC { ", \"idle\": true" } else { "" };
        format!("{{\"t\": \"{t}\", \"app\": \"Arc\", \"frames\": [\"x.jpg\"], \"idle_sec\": {idle_sec}{idle}}}\n")
    }

    /// Строка старого формата: `"idle": true` без `idle_sec`.
    fn cap_idle(t: &str) -> String {
        format!("{{\"t\": \"{t}\", \"app\": \"Arc\", \"frames\": [\"x.jpg\"], \"idle\": true}}\n")
    }

    /// Строки подряд идущих минут от `from` до `to` включительно; `row` получает время и номер по порядку.
    fn seq(from: &str, to: &str, row: impl Fn(&str, u64) -> String) -> String {
        (minute_no(from)..=minute_no(to)).enumerate().map(|(i, m)| row(&minute_str(m), i as u64)).collect()
    }

    fn no_input(captures: &str) -> Vec<String> {
        let rows: Vec<CaptureRow> = captures.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        no_input_minutes(&rows).into_iter().map(minute_str).collect()
    }

    #[test]
    fn no_input_covers_minutes_since_last_input() {
        // ввод в 13:26:30; на снимке 13:32 без ввода 330 с, эта строка покрывает 13:27–13:32
        let captures = [capi("13:26", 0), seq("13:27", "13:33", |t, i| capi(t, 30 + 60 * i))].concat();
        assert_eq!(no_input(&captures), ["13:27", "13:28", "13:29", "13:30", "13:31", "13:32", "13:33"]);
    }

    #[test]
    fn no_input_needs_threshold_on_a_capture() {
        // ввод в 13:26:30 и снова в 13:30:10: порог не набран ни на одном снимке
        let captures = [seq("13:27", "13:30", |t, i| capi(t, 30 + 60 * i)), capi("13:31", 50)].concat();
        assert_eq!(no_input(&captures), Vec::<String>::new());
    }

    #[test]
    fn old_row_marks_only_itself() {
        let captures = [cap("14:20", "Arc"), cap("14:21", "Arc"), cap_idle("14:22"), cap("14:23", "Arc")].concat();
        assert_eq!(no_input(&captures), ["14:22"]);
    }

    #[test]
    fn duplicate_rows_of_a_minute_are_united() {
        let before = seq("13:27", "13:31", |t, i| capi(t, 30 + 60 * i));
        let expected = ["13:27", "13:28", "13:29", "13:30", "13:31", "13:32"];
        // у минуты 13:32 две строки, порог набран только в одной: порядок строк не важен
        assert_eq!(no_input(&[before.clone(), capi("13:32", 330), capi("13:32", 2)].concat()), expected);
        assert_eq!(no_input(&[before, capi("13:32", 2), capi("13:32", 330)].concat()), expected);
    }

    #[test]
    fn no_input_stops_at_midnight_and_ignores_garbage() {
        assert_eq!(no_input(&capi("00:02", 330)), ["00:00", "00:01", "00:02"]);
        let garbage = "{\"t\": \"10:00\", \"idle_sec\": \"много\", \"idle\": \"да\"}\n{\"t\": \"10:01\", \"idle_sec\": -400}\n";
        assert_eq!(no_input(garbage), Vec::<String>::new());
        assert_eq!(no_input("{\"t\": \"10:05\", \"idle_sec\": 300.0}\n"), ["10:00", "10:01", "10:02", "10:03", "10:04", "10:05"]);
    }

    // ---------- паузы дня ----------

    const DAY: &str = "2026-11-25";

    /// День во временном каталоге; пустая строка — файла нет.
    fn day_of(captures: &str, minutes: &str, reasons: &str) -> Day {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join(DAY);
        fs::create_dir_all(&d).unwrap();
        for (name, text) in [("captures.jsonl", captures), ("minutes.jsonl", minutes), ("pauses.jsonl", reasons)] {
            if !text.is_empty() {
                write(&d, name, text);
            }
        }
        get_day(tmp.path(), DAY).unwrap()
    }

    /// Паузы дня в виде «начало–конец минуты».
    fn spans(day: &Day) -> Vec<String> {
        day.pauses.iter().map(|p| format!("{}–{} {}", p.start, p.end, p.minutes)).collect()
    }

    fn pauses_of(captures: &str, minutes: &str) -> Vec<String> {
        spans(&day_of(captures, minutes, ""))
    }

    fn plain(t: &str, _: u64) -> String {
        cap(t, "Arc")
    }

    fn flagged(t: &str, _: u64) -> String {
        cap_idle(t)
    }

    #[test]
    fn pause_starts_at_last_input() {
        // последний ввод в 13:26:30, следующий в 13:59:10
        let captures =
            [capi("13:26", 0), seq("13:27", "13:59", |t, i| capi(t, 30 + 60 * i)), capi("14:00", 50)].concat();
        assert_eq!(pauses_of(&captures, ""), ["13:27–14:00 33"]);
    }

    #[test]
    fn away_for_less_than_threshold_is_not_a_pause() {
        // последний ввод в 13:26:30, следующий в 13:30:10
        let captures = [capi("13:26", 0), seq("13:27", "13:30", |t, i| capi(t, 30 + 60 * i)), capi("13:31", 50)].concat();
        assert_eq!(pauses_of(&captures, ""), Vec::<String>::new());
    }

    #[test]
    fn sleep_inside_a_pause_keeps_it_one() {
        // 18 ноября: три куска минут «нет ввода» и два пропуска между ними
        let captures = [
            seq("14:40", "14:43", plain),
            seq("14:44", "15:08", flagged),
            seq("15:13", "15:14", flagged),
            seq("15:32", "15:43", flagged),
            cap("15:44", "Arc"),
        ]
        .concat();
        assert_eq!(pauses_of(&captures, ""), ["14:44–15:44 60"]);
    }

    #[test]
    fn gap_without_no_input_minutes() {
        let gap = |to: &str| pauses_of(&[cap("12:00", "Arc"), cap(to, "Arc")].concat(), "");
        assert_eq!(gap("12:16"), ["12:01–12:16 15"]);
        assert_eq!(gap("12:06"), ["12:01–12:06 5"], "пропуск ровно в 5 минут");
        assert_eq!(gap("12:05"), Vec::<String>::new(), "пропуск в 4 минуты");
        assert_eq!(gap("12:04"), Vec::<String>::new(), "пропуск в 3 минуты");
    }

    #[test]
    fn short_gap_next_to_no_input_minutes_joins_the_pause() {
        let captures = [cap("11:11", "Arc"), seq("11:12", "11:20", flagged), seq("11:22", "11:28", flagged), cap("11:29", "Arc")].concat();
        assert_eq!(pauses_of(&captures, ""), ["11:12–11:29 17"]);
    }

    #[test]
    fn pause_at_the_end_of_records_and_at_midnight() {
        let captures = [cap("17:19", "Arc"), seq("17:20", "17:45", flagged)].concat();
        assert_eq!(pauses_of(&captures, ""), ["17:20–17:46 26"]);
        let captures = [cap("23:49", "Arc"), seq("23:50", "23:59", flagged)].concat();
        assert_eq!(pauses_of(&captures, ""), ["23:50–24:00 10"]);
    }

    #[test]
    fn day_without_idle_sec_uses_row_flags() {
        let captures = [cap("14:21", "Arc"), seq("14:22", "14:29", flagged), cap("14:30", "Arc")].concat();
        assert_eq!(pauses_of(&captures, ""), ["14:22–14:30 8"]);
    }

    #[test]
    fn minutes_without_captures_count_as_recorded() {
        // описанные минуты без строк в captures.jsonl: пропуск между ними — пауза, как в журнале
        let minutes = [cmin("12:00", "PR", "\"work\""), cmin("12:16", "PR", "\"work\"")].concat();
        assert_eq!(pauses_of("", &minutes), ["12:01–12:16 15"]);
        assert_eq!(pauses_of("", ""), Vec::<String>::new());
    }

    /// Минуты «нет ввода» 10:11–10:17 между обычными 10:10 и 10:18.
    fn stretch() -> String {
        [cap("10:10", "Arc"), seq("10:11", "10:17", flagged), cap("10:18", "Arc")].concat()
    }

    /// Описания минут 10:11–10:17 с категориями по порядку.
    fn described(categories: [&str; 7]) -> String {
        seq("10:11", "10:17", |t, i| cmin(t, "что-то", &format!("\"{}\"", categories[i as usize])))
    }

    #[test]
    fn call_is_not_a_pause() {
        assert_eq!(pauses_of(&stretch(), &described(["work"; 7])), Vec::<String>::new());
        assert_eq!(pauses_of(&stretch(), &described(["nonwork"; 7])), Vec::<String>::new(), "личный звонок");
    }

    #[test]
    fn undescribed_stretch_is_a_pause() {
        assert_eq!(pauses_of(&stretch(), ""), ["10:11–10:18 7"]);
        // описаны без категории или как «(нет описания)»: категории нет ни у одной минуты
        let no_category = seq("10:11", "10:17", |t, _| min(t, "Arc", NO_DESCRIPTION));
        assert_eq!(pauses_of(&stretch(), &no_category), ["10:11–10:18 7"]);
    }

    #[test]
    fn half_rule_survives_one_wrong_minute() {
        let one_wrong_in_pause = ["idle", "idle", "idle", "work", "idle", "idle", "idle"];
        assert_eq!(pauses_of(&stretch(), &described(one_wrong_in_pause)), ["10:11–10:18 7"]);
        let one_wrong_in_call = ["work", "work", "work", "idle", "work", "work", "work"];
        assert_eq!(pauses_of(&stretch(), &described(one_wrong_in_call)), Vec::<String>::new());

        // ровно половина минут с категорией — созвон: «не меньше половины»
        let minutes = [
            cmin("10:11", "созвон", "\"work\""),
            cmin("10:12", "созвон", "\"work\""),
            cmin("10:13", "пауза", "\"idle\""),
            cmin("10:14", "пауза", "\"idle\""),
        ]
        .concat();
        assert_eq!(pauses_of(&stretch(), &minutes), Vec::<String>::new());
        // описана одна минута из семи, и она — пауза
        assert_eq!(pauses_of(&stretch(), &cmin("10:11", "пауза", "\"idle\"")), ["10:11–10:18 7"]);
    }

    #[test]
    fn before_is_the_last_described_minute() {
        let captures = [seq("14:18", "14:21", plain), seq("14:22", "14:29", flagged), cap("14:30", "Arc")].concat();
        // 14:19 описана, 14:20 — «(нет описания)», 14:21 снята, но не описана
        let minutes = [
            cmin("14:18", "читал Slack", "\"work\""),
            cmin("14:19", "смотрел PR #86991 на GitHub", "\"work\""),
            min("14:20", "Arc", NO_DESCRIPTION),
            cmin("14:30", "вернулся", "\"work\""),
        ]
        .concat();
        let day = day_of(&captures, &minutes, "");
        assert_eq!(spans(&day), ["14:22–14:30 8"]);
        assert_eq!(day.pauses[0].before.as_deref(), Some("смотрел PR #86991 на GitHub"));
        assert_eq!(day.pauses[0].reason, None);

        // перед паузой ничего не описано
        let day = day_of(&captures, &cmin("14:30", "вернулся", "\"work\""), "");
        assert_eq!(day.pauses[0].before, None);
    }

    #[test]
    fn pause_json_shape() {
        let day = day_of(&[cap("14:21", "Arc"), seq("14:22", "14:29", flagged), cap("14:30", "Arc")].concat(), "", "");
        let json = serde_json::to_value(&day).unwrap();
        assert_eq!(
            json["pauses"],
            serde_json::json!([{"start": "14:22", "end": "14:30", "minutes": 8, "reason": null, "before": null}])
        );
    }

    // ---------- причины пауз ----------

    fn reason(start: &str, end: &str, text: &str) -> String {
        format!("{{\"start\": \"{start}\", \"end\": \"{end}\", \"reason\": \"{text}\", \"at\": \"2026-11-25T15:00:00\"}}\n")
    }

    /// Пауза 14:22–14:30 и, если задано `second`, вторая пауза 14:36–14:50.
    fn two_pauses() -> String {
        [
            cap("14:21", "Arc"),
            seq("14:22", "14:29", flagged),
            seq("14:30", "14:35", plain),
            seq("14:36", "14:49", flagged),
            cap("14:50", "Arc"),
        ]
        .concat()
    }

    fn reasons_of(captures: &str, reasons: &str) -> Vec<(String, Option<String>)> {
        day_of(captures, "", reasons).pauses.into_iter().map(|p| (format!("{}–{}", p.start, p.end), p.reason)).collect()
    }

    fn some(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn reason_belongs_to_the_overlapping_pause_only() {
        let got = reasons_of(&two_pauses(), &reason("14:22", "14:30", "поговорил с Игорем"));
        // соседняя пауза 14:36–14:50 остаётся без причины
        assert_eq!(got, [("14:22–14:30".to_owned(), some("поговорил с Игорем")), ("14:36–14:50".to_owned(), None)]);
    }

    #[test]
    fn last_reason_wins() {
        let reasons = [reason("14:22", "14:30", "кофе"), reason("14:22", "14:30", "кофе и разговор с Игорем")].concat();
        assert_eq!(reasons_of(&two_pauses(), &reasons)[0].1, some("кофе и разговор с Игорем"));
        // порядок в файле, а не время в строке: более поздняя строка действует и для части отрезка
        let reasons = [reason("14:22", "14:30", "первая"), reason("14:25", "14:26", "вторая")].concat();
        assert_eq!(reasons_of(&two_pauses(), &reasons)[0].1, some("вторая"));
    }

    #[test]
    fn reason_survives_a_grown_pause() {
        // причина записана для 14:44–15:09; после сна Mac пауза стала 14:44–15:44
        let captures = [
            cap("14:43", "Arc"),
            seq("14:44", "15:08", flagged),
            seq("15:13", "15:14", flagged),
            seq("15:32", "15:43", flagged),
            cap("15:44", "Arc"),
        ]
        .concat();
        let got = reasons_of(&captures, &reason("14:44", "15:09", "обед"));
        assert_eq!(got, [("14:44–15:44".to_owned(), some("обед"))]);
    }

    #[test]
    fn reason_touching_a_pause_edge_does_not_overlap() {
        // отрезок причины кончается там, где пауза начинается, и наоборот: конец не включён
        let reasons = [reason("14:10", "14:22", "до"), reason("14:30", "14:36", "между")].concat();
        let got = reasons_of(&two_pauses(), &reasons);
        assert_eq!(got.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(), [None, None]);
    }

    #[test]
    fn broken_and_invalid_reason_rows_are_skipped() {
        let reasons = [
            reason("14:22", "14:30", "кофе"),
            reason("14:36", "25:00", "конец не время"),
            reason("9:5", "14:50", "начало не время"),
            reason("14:49", "14:36", "начало позже конца"),
            reason("14:36", "14:50", "   "),
            "{\"start\": \"14:36\", \"end\": \"14:50\", \"reason\": 5}\n".to_owned(),
            "{\"start\": \"14:36\", \"end\": \"14:50\"}\n".to_owned(),
            "{\"start\": \"14:36\", \"end\": \"14:5".to_owned(), // недописанная последняя строка
        ]
        .concat();
        let got = reasons_of(&two_pauses(), &reasons);
        assert_eq!(got, [("14:22–14:30".to_owned(), some("кофе")), ("14:36–14:50".to_owned(), None)]);
    }

    #[test]
    fn no_reasons_file_means_no_reasons() {
        let got = reasons_of(&two_pauses(), "");
        assert_eq!(got.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(), [None, None]);
    }

    #[test]
    fn reason_until_midnight_and_trimmed() {
        let captures = [cap("23:49", "Arc"), seq("23:50", "23:59", flagged)].concat();
        assert_eq!(reasons_of(&captures, &reason("23:50", "24:00", "  спать  ")), [("23:50–24:00".to_owned(), some("спать"))]);
    }

    #[test]
    fn call_described_later_is_not_a_pause_even_with_a_reason() {
        let day = day_of(&stretch(), "", &reason("10:11", "10:18", "созвон"));
        assert_eq!(day.pauses.len(), 1);
        assert_eq!(day.pauses[0].reason, some("созвон"));
        let day = day_of(&stretch(), &described(["work"; 7]), &reason("10:11", "10:18", "созвон"));
        assert_eq!(day.pauses, vec![]);
    }

    // ---------- недавние причины ----------

    #[test]
    fn day_arithmetic() {
        assert_eq!(day_minus("2026-11-25", 13).as_deref(), Some("2026-11-12"));
        assert_eq!(day_minus("2026-01-05", 13).as_deref(), Some("2025-12-23"));
        assert_eq!(day_minus("2024-03-01", 1).as_deref(), Some("2024-02-29"));
        assert_eq!(day_minus("2026-03-01", 1).as_deref(), Some("2026-02-28"));
        assert_eq!(day_minus("2026-11-25", 0).as_deref(), Some("2026-11-25"));
        assert_eq!(day_minus("сегодня", 1), None);
    }

    #[test]
    fn recent_reasons_newest_first_unique_and_limited() {
        let tmp = tempfile::tempdir().unwrap();
        let day = |name: &str, reasons: &[&str]| {
            let text: String = reasons.iter().map(|r| reason("10:00", "10:10", r)).collect();
            write(&tmp.path().join(name), "pauses.jsonl", &text);
        };
        day("2026-11-25", &["кофе", "созвон", " кофе "]); // последняя строка — самая новая
        day("2026-11-24", &["обед", "созвон", "устал"]);
        day("2026-11-23", &["седьмая по счёту", "читал", "думал"]);
        day("2026-11-26", &["из завтра"]);
        write(&tmp.path().join("notes"), "pauses.jsonl", &reason("10:00", "10:10", "не день"));

        // от новых к старым, повтор один раз, седьмая разная причина не попадает
        assert_eq!(recent_reasons(tmp.path(), "2026-11-25"), ["кофе", "созвон", "устал", "обед", "думал", "читал"]);
        assert_eq!(recent_reasons(tmp.path(), "2026-11-23"), ["думал", "читал", "седьмая по счёту"]);
    }

    #[test]
    fn recent_reasons_window_is_fourteen_days() {
        let tmp = tempfile::tempdir().unwrap();
        for (day, text) in [("2026-11-25", "сегодня"), ("2026-11-12", "13 дней назад"), ("2026-11-11", "14 дней назад"), ("2026-11-10", "15 дней назад")] {
            write(&tmp.path().join(day), "pauses.jsonl", &reason("10:00", "10:10", text));
        }
        assert_eq!(recent_reasons(tmp.path(), "2026-11-25"), ["сегодня", "13 дней назад"]);
    }

    #[test]
    fn recent_reasons_without_data() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(recent_reasons(&tmp.path().join("нет"), "2026-11-25"), Vec::<String>::new());
        assert_eq!(recent_reasons(tmp.path(), "2026-11-25"), Vec::<String>::new());
        assert_eq!(recent_reasons(tmp.path(), "не день"), Vec::<String>::new());
        // день без pauses.jsonl и «день», который файл, а не каталог
        write(&tmp.path().join("2026-11-25"), "captures.jsonl", &cap("09:00", "Arc"));
        write(tmp.path(), "2026-11-24", "файл");
        assert_eq!(recent_reasons(tmp.path(), "2026-11-25"), Vec::<String>::new());
    }
}
