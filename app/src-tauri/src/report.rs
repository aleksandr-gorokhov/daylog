//! Фоновые команды дня: сборка отчёта (`daylog.py report <день>`) и доразметка категорий
//! (`daylog.py classify <день>`), не больше одной каждого вида на день.

use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::data::check_day;
use crate::procs::{tail, Runtime};

/// Дни, для которых сейчас идёт сборка.
#[derive(Default)]
pub struct ReportJobs(Mutex<BTreeSet<String>>);

impl ReportJobs {
    /// `false`, если для этого дня сборка уже идёт.
    pub fn try_begin(&self, day: &str) -> bool {
        self.0.lock().unwrap().insert(day.to_owned())
    }

    pub fn finish(&self, day: &str) {
        self.0.lock().unwrap().remove(day);
    }

    pub fn list(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
}

/// События `report-finished` и `classify-finished`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobFinished {
    pub day: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Запускает `daylog.py <команда> <день>` и ждёт завершения. Ошибка — конец вывода команды.
pub fn run_daylog(rt: &Runtime, command: &str, day: &str) -> Result<(), String> {
    check_day(day)?;
    let out = rt
        .command(&[command, day])?
        .output()
        .map_err(|e| format!("не удалось запустить {}: {e}", rt.script.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let text = if stderr.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { stderr };
    let code = out.status.code().map_or("сигнал".to_owned(), |c| c.to_string());
    Err(format!("daylog.py {command} завершился с кодом {code}: {}", tail(&text, 500)))
}

pub fn run_report(rt: &Runtime, day: &str) -> Result<(), String> {
    run_daylog(rt, "report", day)
}

/// Проверяет день, регистрирует команду в `jobs` и запускает её в отдельном потоке.
/// По завершении вызывает `on_done` (приложение шлёт из него событие).
fn start_job(
    rt: Runtime,
    jobs: Arc<ReportJobs>,
    command: &'static str,
    day: String,
    busy: impl FnOnce(&str) -> String,
    on_done: impl FnOnce(JobFinished) + Send + 'static,
) -> Result<(), String> {
    check_day(&day)?;
    if rt.python.is_none() {
        return rt.command(&[]).map(|_| ());
    }
    if !jobs.try_begin(&day) {
        return Err(busy(&day));
    }
    std::thread::spawn(move || {
        let result = run_daylog(&rt, command, &day);
        jobs.finish(&day);
        on_done(JobFinished { ok: result.is_ok(), error: result.err(), day });
    });
    Ok(())
}

/// Сборка отчёта в фоне; повторный запуск для того же дня — ошибка.
pub fn start_report(
    rt: Runtime,
    jobs: Arc<ReportJobs>,
    day: String,
    on_done: impl FnOnce(JobFinished) + Send + 'static,
) -> Result<(), String> {
    start_job(rt, jobs, "report", day, |d| format!("отчёт за {d} уже собирается"), on_done)
}

/// Доразметка: дни, где она идёт, и дни, для которых она уже запускалась в этом запуске приложения.
#[derive(Default)]
pub struct ClassifyJobs {
    running: Arc<ReportJobs>,
    attempted: Mutex<BTreeSet<String>>,
}

/// Что сделал `start_classify`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClassifyStart {
    /// Доразметка запущена, результат придёт событием.
    Started,
    /// Для этого дня она уже идёт.
    Running,
    /// Уже запускалась в этом запуске приложения: повторно не запускается.
    Skipped,
}

/// Доразметка категорий в фоне: не больше одной на день и не больше одной за запуск приложения.
pub fn start_classify(
    rt: Runtime,
    jobs: Arc<ClassifyJobs>,
    day: String,
    on_done: impl FnOnce(JobFinished) + Send + 'static,
) -> Result<ClassifyStart, String> {
    check_day(&day)?;
    if jobs.running.list().contains(&day) {
        return Ok(ClassifyStart::Running);
    }
    if !jobs.attempted.lock().unwrap().insert(day.clone()) {
        return Ok(ClassifyStart::Skipped);
    }
    let running = jobs.running.clone();
    start_job(rt, running, "classify", day, |d| format!("разметка {d} уже идёт"), on_done)?;
    Ok(ClassifyStart::Started)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::resolve_python;
    use std::fs;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Подставной daylog.py: отмечает запуск файлом `<день>-ran` в DAYLOG_DIR.
    const STUB: &str = r#"
import os, sys, time
d = os.environ["DAYLOG_DIR"]
cmd, day = sys.argv[1], sys.argv[2]
open(os.path.join(d, day + "-ran"), "a").write("x")
time.sleep(float(os.environ.get("STUB_SLEEP", "0")))
if day == "2000-01-01":
    print("что-то в stdout")
    print("Traceback: LLM упал", file=sys.stderr)
    sys.exit(1)
"#;

    fn runtime(dir: &Path, script: &str) -> Runtime {
        let path = dir.join("stub.py");
        fs::write(&path, script).unwrap();
        Runtime {
            data_dir: dir.to_owned(),
            python: resolve_python(None, |p| p.is_file()),
            script: path,
            path_var: std::env::var("PATH").unwrap_or_default(),
        }
    }

    #[test]
    fn success_and_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), STUB);
        assert_eq!(run_report(&rt, "2026-11-16"), Ok(()));
        assert!(tmp.path().join("2026-11-16-ran").exists());

        let err = run_report(&rt, "2000-01-01").unwrap_err();
        assert!(err.contains("кодом 1"), "{err}");
        assert!(err.contains("LLM упал"), "{err}");
    }

    #[test]
    fn missing_script_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rt = runtime(tmp.path(), STUB);
        rt.script = "/нет/такого/daylog.py".into();
        let err = run_report(&rt, "2026-11-16").unwrap_err();
        assert!(err.contains("/нет/такого/daylog.py"), "{err}");
    }

    #[test]
    fn bad_day_does_not_start_process() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), STUB);
        let jobs = Arc::new(ReportJobs::default());
        let err = start_report(rt, jobs.clone(), "../x".into(), |_| panic!("не должно вызываться")).unwrap_err();
        assert!(err.contains("некорректный день"), "{err}");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1, "только stub.py");
        assert!(jobs.list().is_empty());
    }

    #[test]
    fn one_job_per_day_and_event_on_finish() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), STUB);
        let jobs = Arc::new(ReportJobs::default());
        let (tx, rx) = mpsc::channel();

        let mut slow = rt.clone();
        slow.script = tmp.path().join("slow.py");
        fs::write(&slow.script, format!("import os\nos.environ['STUB_SLEEP'] = '1'\n{STUB}")).unwrap();

        let tx1 = tx.clone();
        start_report(slow.clone(), jobs.clone(), "2026-11-16".into(), move |e| tx1.send(e).unwrap()).unwrap();
        assert_eq!(jobs.list(), vec!["2026-11-16".to_owned()]);
        let again = start_report(slow, jobs.clone(), "2026-11-16".into(), |_| {}).unwrap_err();
        assert!(again.contains("уже собирается"), "{again}");

        let tx2 = tx.clone();
        start_report(rt, jobs.clone(), "2000-01-01".into(), move |e| tx2.send(e).unwrap()).unwrap();

        let mut events: Vec<JobFinished> =
            (0..2).map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        events.sort_by(|a, b| a.day.cmp(&b.day));
        assert_eq!(events[0].day, "2000-01-01");
        assert!(!events[0].ok);
        assert!(events[0].error.as_deref().unwrap().contains("LLM упал"));
        assert_eq!(events[1], JobFinished { day: "2026-11-16".into(), ok: true, error: None });
        assert!(jobs.list().is_empty());
        assert_eq!(fs::read_to_string(tmp.path().join("2026-11-16-ran")).unwrap(), "x", "запущен один раз");
    }

    #[test]
    fn classify_runs_once_per_app_run() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rt = runtime(tmp.path(), STUB);
        fs::write(&rt.script, format!("import os\nos.environ['STUB_SLEEP'] = '1'\n{STUB}")).unwrap();
        rt.script = tmp.path().join("stub.py");
        let jobs = Arc::new(ClassifyJobs::default());
        let (tx, rx) = mpsc::channel();

        let tx1 = tx.clone();
        let first = start_classify(rt.clone(), jobs.clone(), "2026-11-16".into(), move |e| tx1.send(e).unwrap());
        assert_eq!(first, Ok(ClassifyStart::Started));
        let during = start_classify(rt.clone(), jobs.clone(), "2026-11-16".into(), |_| panic!("не запускается"));
        assert_eq!(during, Ok(ClassifyStart::Running));

        let done = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(done, JobFinished { day: "2026-11-16".into(), ok: true, error: None });
        let after = start_classify(rt.clone(), jobs.clone(), "2026-11-16".into(), |_| panic!("не запускается"));
        assert_eq!(after, Ok(ClassifyStart::Skipped));
        assert_eq!(fs::read_to_string(tmp.path().join("2026-11-16-ran")).unwrap(), "x", "запущен один раз");
    }

    #[test]
    fn classify_failure_is_reported_and_not_retried() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), STUB);
        let jobs = Arc::new(ClassifyJobs::default());
        let (tx, rx) = mpsc::channel();
        assert_eq!(
            start_classify(rt.clone(), jobs.clone(), "2000-01-01".into(), move |e| tx.send(e).unwrap()),
            Ok(ClassifyStart::Started)
        );
        let done = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!done.ok);
        let error = done.error.unwrap();
        assert!(error.contains("daylog.py classify завершился с кодом 1"), "{error}");
        assert!(error.contains("LLM упал"), "{error}");
        assert_eq!(
            start_classify(rt, jobs, "2000-01-01".into(), |_| panic!("не запускается")),
            Ok(ClassifyStart::Skipped)
        );
    }

    #[test]
    fn classify_bad_day_does_not_start_process() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), STUB);
        let err = start_classify(rt, Arc::new(ClassifyJobs::default()), "../x".into(), |_| panic!()).unwrap_err();
        assert!(err.contains("некорректный день"), "{err}");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1, "только stub.py");
    }
}
