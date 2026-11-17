//! Чтение каталога данных daylog. Только чтение и без зависимостей от Tauri.
//!
//! Каталог данных: `$DAYLOG_DIR`, иначе `~/.local/share/daylog`; внутри по каталогу на день
//! `YYYY-MM-DD/` с `captures.jsonl`, `minutes.jsonl` и (после сборки) `report.json`.

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

#[derive(Deserialize)]
struct CaptureRow {
    t: String,
    #[serde(default)]
    app: String,
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Day {
    pub day: String,
    /// Время за компьютером: уникальные минуты в `captures.jsonl`.
    pub active_minutes: usize,
    /// Описанные минуты без категории (кроме «(нет описания)»): их нужно доразметить.
    pub unclassified: usize,
    pub minutes: Vec<Minute>,
    pub report: Option<Report>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DaySummary {
    pub day: String,
    pub active_minutes: usize,
    /// Минуты с категорией «рабочее».
    pub work_minutes: usize,
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

    let report = read_report(&dir, described.len() as u64);
    Ok(Day {
        day: day.to_owned(),
        active_minutes: active_minutes(&captures),
        unclassified: minutes.iter().filter(|m| m.needs_category()).count(),
        minutes,
        report,
    })
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
}
