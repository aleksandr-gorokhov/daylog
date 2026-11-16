//! Сборщик `daylog.py run`: состояние по блокировке `collector.lock`, запуск и остановка.
//!
//! Источник правды — блокировка, а не PID: сборщик запущен тогда и только тогда, когда
//! кто-то держит `flock` на `$DAYLOG_DIR/collector.lock`. Поэтому одинаково видны сборщик,
//! запущенный приложением, оставшийся от прошлого запуска и запущенный из терминала.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::procs::{file_tail, Runtime};

pub const START_WAIT: Duration = Duration::from_secs(3);
pub const STOP_WAIT: Duration = Duration::from_secs(5);

pub const PERMISSION_MESSAGE: &str = "Нет разрешения на запись экрана. Включите daylog в System Settings → \
Privacy & Security → Screen & System Audio Recording.";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollectorStatus {
    pub running: bool,
    pub pid: Option<i32>,
}

impl CollectorStatus {
    const STOPPED: CollectorStatus = CollectorStatus { running: false, pid: None };
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    Permission,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollectorError {
    pub kind: ErrorKind,
    pub message: String,
}

impl CollectorError {
    fn failed(message: impl Into<String>) -> Self {
        CollectorError { kind: ErrorKind::Failed, message: message.into() }
    }

    fn permission(message: impl Into<String>) -> Self {
        CollectorError { kind: ErrorKind::Permission, message: message.into() }
    }
}

pub fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join("collector.lock")
}

pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("collector.log")
}

/// Пробует взять блокировку. Получилось — сборщика нет (блокировка сразу отпускается).
/// Занята — сборщик работает, PID читается из файла. Файл не создаётся и не меняется.
pub fn probe(data_dir: &Path) -> io::Result<CollectorStatus> {
    let path = lock_path(data_dir);
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(CollectorStatus::STOPPED),
        Err(e) => return Err(e),
    };
    let fd = file.as_raw_fd();
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        return Ok(CollectorStatus::STOPPED);
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::EWOULDBLOCK) {
        return Err(err);
    }
    let pid = fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<i32>().ok()).filter(|p| *p > 1);
    Ok(CollectorStatus { running: true, pid })
}

/// Проверка разрешения Screen Recording у приложения.
pub fn check_screen_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
            fn CGRequestScreenCaptureAccess() -> bool;
        }
        if unsafe { CGPreflightScreenCaptureAccess() } {
            return Ok(());
        }
        // Система покажет запрос и добавит приложение в список; разрешение начнёт
        // действовать после перезапуска приложения.
        unsafe { CGRequestScreenCaptureAccess() };
        Err(PERMISSION_MESSAGE.to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    Ok(())
}

/// Состояние сборщика в приложении: процесс, запущенный в этом сеансе.
#[derive(Default)]
pub struct Collector {
    child: Mutex<Option<Child>>,
    /// Запуск и остановка не выполняются одновременно.
    op: Mutex<()>,
}

impl Collector {
    /// Забирает завершившийся дочерний процесс, чтобы не копить зомби.
    fn reap(&self) {
        let mut slot = self.child.lock().unwrap();
        if let Some(child) = slot.as_mut() {
            if !matches!(child.try_wait(), Ok(None)) {
                *slot = None;
            }
        }
    }

    pub fn status(&self, data_dir: &Path) -> Result<CollectorStatus, String> {
        self.reap();
        probe(data_dir).map_err(|e| format!("не удалось проверить collector.lock: {e}"))
    }

    /// PID сборщика, запущенного этим приложением в этом сеансе, если он ещё работает.
    pub fn owned_pid(&self) -> Option<u32> {
        self.reap();
        self.child.lock().unwrap().as_ref().map(Child::id)
    }

    /// Запускает `daylog.py run`, если сборщик ещё не запущен.
    ///
    /// Ждёт `wait` целиком: блокировку сборщик берёт до пробного кадра, а без разрешения
    /// выходит с кодом 3 уже после него. Успех — процесс жив и держит блокировку.
    pub fn start(
        &self,
        rt: &Runtime,
        check_permission: impl FnOnce() -> Result<(), String>,
        wait: Duration,
    ) -> Result<CollectorStatus, CollectorError> {
        let _op = self.op.lock().unwrap();
        let current = self.status(&rt.data_dir).map_err(CollectorError::failed)?;
        if current.running {
            return Ok(current);
        }
        check_permission().map_err(CollectorError::permission)?;

        let io_err = |what: &str, e: io::Error| CollectorError::failed(format!("{what}: {e}"));
        fs::create_dir_all(&rt.data_dir).map_err(|e| io_err("не удалось создать каталог данных", e))?;
        let log_file = log_path(&rt.data_dir);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|e| io_err("не удалось открыть collector.log", e))?;
        let offset = log.metadata().map(|m| m.len()).unwrap_or(0);
        let log_err = log.try_clone().map_err(|e| io_err("collector.log", e))?;

        let mut cmd = rt.command(&["run"]).map_err(CollectorError::failed)?;
        cmd.stdout(log).stderr(log_err).process_group(0);
        let mut child = cmd
            .spawn()
            .map_err(|e| CollectorError::failed(format!("не удалось запустить сборщик: {e}")))?;

        let deadline = Instant::now() + wait;
        loop {
            if let Ok(Some(exit)) = child.try_wait() {
                let tail = file_tail(&log_file, offset, 500);
                return match exit.code() {
                    Some(3) => Err(CollectorError::permission(format!("{PERMISSION_MESSAGE}\n\n{tail}"))),
                    Some(2) => match probe(&rt.data_dir) {
                        // Пока запускали, сборщик успел запустить кто-то другой.
                        Ok(st) if st.running => Ok(st),
                        _ => Err(CollectorError::failed(tail)),
                    },
                    code => Err(CollectorError::failed(format!(
                        "сборщик завершился сразу после запуска (код {}):\n{tail}",
                        code.map_or("сигнал".to_owned(), |c| c.to_string())
                    ))),
                };
            }
            if Instant::now() >= deadline {
                break;
            }
            sleep(Duration::from_millis(50));
        }

        let st = probe(&rt.data_dir).map_err(|e| io_err("не удалось проверить collector.lock", e))?;
        if st.running && st.pid == Some(child.id() as i32) {
            *self.child.lock().unwrap() = Some(child);
            return Ok(st);
        }
        let _ = child.kill();
        let _ = child.wait();
        if st.running {
            return Ok(st);
        }
        Err(CollectorError::failed(format!(
            "сборщик не занял collector.lock за {} с:\n{}",
            wait.as_secs(),
            file_tail(&log_file, offset, 500)
        )))
    }

    /// Останавливает работающий сборщик, кем бы он ни был запущен: SIGTERM по PID из
    /// `collector.lock`, только пока блокировка занята, и ожидание её освобождения.
    pub fn stop(&self, data_dir: &Path, wait: Duration) -> Result<CollectorStatus, CollectorError> {
        let _op = self.op.lock().unwrap();
        let st = self.status(data_dir).map_err(CollectorError::failed)?;
        if !st.running {
            return Ok(st);
        }
        let pid = st.pid.ok_or_else(|| CollectorError::failed("в collector.lock нет PID сборщика"))?;
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(CollectorError::failed(format!("не удалось остановить сборщик (PID {pid}): {err}")));
            }
        }
        let deadline = Instant::now() + wait;
        loop {
            sleep(Duration::from_millis(100));
            let st = self.status(data_dir).map_err(CollectorError::failed)?;
            if !st.running {
                return Ok(st);
            }
            if Instant::now() >= deadline {
                return Err(CollectorError::failed(format!(
                    "сборщик (PID {pid}) не завершился за {} с",
                    wait.as_secs()
                )));
            }
        }
    }

    /// При выходе из приложения: останавливает сборщик, только если его запустило это приложение.
    pub fn stop_owned(&self, data_dir: &Path, wait: Duration) -> Option<Result<CollectorStatus, CollectorError>> {
        let pid = self.owned_pid()?;
        let st = probe(data_dir).ok()?;
        if st.pid != Some(pid as i32) {
            return None;
        }
        Some(self.stop(data_dir, wait))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedState {
    recording: bool,
}

/// `recording` из `state.json`: шла ли запись, когда приложение закрыли. Нет файла — `false`.
pub fn read_recording(state_file: &Path) -> bool {
    fs::read_to_string(state_file)
        .ok()
        .and_then(|s| serde_json::from_str::<SavedState>(&s).ok())
        .is_some_and(|s| s.recording)
}

pub fn write_recording(state_file: &Path, recording: bool) -> io::Result<()> {
    if let Some(dir) = state_file.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = state_file.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string(&SavedState { recording }).unwrap())?;
    fs::rename(tmp, state_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::resolve_python;
    use std::process::{Command, Stdio};

    /// Подставной сборщик: держит блокировку, как `daylog.py run`, и спит.
    const HOLDS_LOCK: &str = r#"
import fcntl, os, signal, sys, time
if os.environ.get("STUB_IGNORE_TERM"):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
d = os.environ["DAYLOG_DIR"]
open(os.path.join(d, "starts"), "a").write("x")
f = open(os.path.join(d, "collector.lock"), "a+")
fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
f.seek(0); f.truncate(); f.write(str(os.getpid())); f.flush()
print("stub: пишу", flush=True)
time.sleep(120)
"#;
    const EXITS_3: &str = "import sys\nprint('screencapture не снял экран', file=sys.stderr)\nsys.exit(3)\n";
    const EXITS_1: &str = "raise SystemExit('ошибка в скрипте сборщика')\n";

    fn python() -> PathBuf {
        resolve_python(None, |p| p.is_file()).expect("python3 для тестов")
    }

    fn runtime(dir: &Path, script: &str) -> Runtime {
        let path = dir.join("stub.py");
        fs::write(&path, script).unwrap();
        Runtime {
            data_dir: dir.to_owned(),
            python: Some(python()),
            script: path,
            path_var: std::env::var("PATH").unwrap_or_default(),
        }
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn kill9(pid: i32) {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }

    const SHORT: Duration = Duration::from_millis(1500);

    #[test]
    fn status_without_lock_file_or_with_free_lock() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(probe(tmp.path()).unwrap(), CollectorStatus::STOPPED);
        fs::write(lock_path(tmp.path()), "12345\n").unwrap();
        assert_eq!(probe(tmp.path()).unwrap(), CollectorStatus::STOPPED);
        assert_eq!(fs::read_to_string(lock_path(tmp.path())).unwrap(), "12345\n", "файл не меняется");
    }

    #[test]
    fn status_with_lock_held_by_child() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("stub.py"), HOLDS_LOCK).unwrap();
        let mut child = Command::new(python())
            .arg(tmp.path().join("stub.py"))
            .env("DAYLOG_DIR", tmp.path())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut st = probe(tmp.path()).unwrap();
        while !(st.running && st.pid.is_some()) && Instant::now() < deadline {
            sleep(Duration::from_millis(50));
            st = probe(tmp.path()).unwrap();
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(st, CollectorStatus { running: true, pid: Some(pid) });
        assert_eq!(probe(tmp.path()).unwrap(), CollectorStatus::STOPPED, "после смерти процесса блокировка свободна");
    }

    #[test]
    fn start_success_then_second_start_is_noop_then_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), HOLDS_LOCK);
        let c = Collector::default();

        let st = c.start(&rt, || Ok(()), SHORT).unwrap();
        assert!(st.running);
        let pid = st.pid.unwrap();
        assert_eq!(c.owned_pid(), Some(pid as u32));
        assert!(fs::read_to_string(log_path(tmp.path())).unwrap().contains("stub: пишу"));

        let again = c.start(&rt, || panic!("при работающем сборщике разрешение не проверяется"), SHORT).unwrap();
        assert_eq!(again, st);
        assert_eq!(fs::read_to_string(tmp.path().join("starts")).unwrap(), "x", "второй не запускался");

        let stopped = c.stop(tmp.path(), STOP_WAIT).unwrap();
        assert_eq!(stopped, CollectorStatus::STOPPED);
        assert!(!alive(pid));
        assert_eq!(c.owned_pid(), None);
    }

    #[test]
    fn start_without_permission_does_not_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), HOLDS_LOCK);
        let err = Collector::default().start(&rt, || Err(PERMISSION_MESSAGE.into()), SHORT).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Permission);
        assert!(!tmp.path().join("starts").exists());
    }

    #[test]
    fn start_exit_code_3_is_permission() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), EXITS_3);
        let c = Collector::default();
        let err = c.start(&rt, || Ok(()), SHORT).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Permission);
        assert!(err.message.contains("screencapture не снял экран"), "{}", err.message);
        assert_eq!(c.status(tmp.path()).unwrap(), CollectorStatus::STOPPED);
    }

    #[test]
    fn start_exit_code_1_is_failed_with_log_tail() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(log_path(tmp.path()), "старый вывод прошлого запуска\n").unwrap();
        let rt = runtime(tmp.path(), EXITS_1);
        let err = Collector::default().start(&rt, || Ok(()), SHORT).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        assert!(err.message.contains("код 1"), "{}", err.message);
        assert!(err.message.contains("ошибка в скрипте сборщика"), "{}", err.message);
        assert!(!err.message.contains("старый вывод"), "{}", err.message);
    }

    #[test]
    fn start_with_missing_script_is_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut rt = runtime(tmp.path(), EXITS_1);
        rt.script = "/нет/такого/daylog.py".into();
        let err = Collector::default().start(&rt, || Ok(()), SHORT).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Failed);
        assert!(err.message.contains("/нет/такого/daylog.py"), "{}", err.message);
    }

    #[test]
    fn stop_collector_ignoring_sigterm_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), HOLDS_LOCK);
        let c = Collector::default();
        let mut cmd = rt.command(&["run"]).unwrap();
        let mut child = cmd.env("STUB_IGNORE_TERM", "1").stdout(Stdio::null()).spawn().unwrap();
        let pid = child.id() as i32;
        while probe(tmp.path()).unwrap().pid != Some(pid) {
            sleep(Duration::from_millis(50));
        }

        let err = c.stop(tmp.path(), Duration::from_secs(1)).unwrap_err();
        let still = probe(tmp.path()).unwrap();
        kill9(pid);
        child.wait().unwrap();
        assert_eq!(err.kind, ErrorKind::Failed);
        assert!(err.message.contains("не завершился"), "{}", err.message);
        assert_eq!(still, CollectorStatus { running: true, pid: Some(pid) });
    }

    #[test]
    fn stop_with_free_lock_sends_no_signal() {
        let tmp = tempfile::tempdir().unwrap();
        // Посторонний процесс, чей PID остался в collector.lock от упавшего сборщика.
        let mut other = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = other.id() as i32;
        fs::write(lock_path(tmp.path()), format!("{pid}\n")).unwrap();

        let st = Collector::default().stop(tmp.path(), Duration::from_secs(1)).unwrap();
        let survived = matches!(other.try_wait(), Ok(None));
        kill9(pid);
        other.wait().unwrap();
        assert_eq!(st, CollectorStatus::STOPPED);
        assert!(survived, "сигнал не должен был отправляться");
    }

    #[test]
    fn stop_owned_ignores_foreign_collector() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), HOLDS_LOCK);
        let c = Collector::default();
        // Сборщик «из терминала»: запущен не через Collector.
        let mut foreign = rt.command(&["run"]).unwrap().stdout(Stdio::null()).spawn().unwrap();
        let pid = foreign.id() as i32;
        while probe(tmp.path()).unwrap().pid != Some(pid) {
            sleep(Duration::from_millis(50));
        }
        assert!(c.stop_owned(tmp.path(), STOP_WAIT).is_none());
        let survived = alive(pid);
        kill9(pid);
        foreign.wait().unwrap();
        assert!(survived);
    }

    #[test]
    fn stop_owned_stops_own_collector() {
        let tmp = tempfile::tempdir().unwrap();
        let rt = runtime(tmp.path(), HOLDS_LOCK);
        let c = Collector::default();
        let pid = c.start(&rt, || Ok(()), SHORT).unwrap().pid.unwrap();
        assert_eq!(c.stop_owned(tmp.path(), STOP_WAIT), Some(Ok(CollectorStatus::STOPPED)));
        assert!(!alive(pid));
    }

    #[test]
    fn recording_state_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("config/state.json");
        assert!(!read_recording(&f));
        write_recording(&f, true).unwrap();
        assert!(read_recording(&f));
        write_recording(&f, false).unwrap();
        assert!(!read_recording(&f));
        assert_eq!(fs::read_to_string(&f).unwrap(), r#"{"recording":false}"#);
    }
}
