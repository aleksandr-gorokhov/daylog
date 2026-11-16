//! Где лежат `python3` и `daylog.py` и с каким окружением их запускать.
//!
//! У приложения, запущенного из Finder, PATH урезан, поэтому пути ищутся по явному списку,
//! а дочернему процессу в PATH добавляются каталоги, где обычно лежит `claude`.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::data;

pub const PYTHON_CANDIDATES: [&str; 3] =
    ["/opt/homebrew/bin/python3", "/usr/local/bin/python3", "/usr/bin/python3"];

/// Путь к `daylog.py`, зашитый при сборке: приложение собирается из этого же репозитория.
pub const BUILT_IN_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../daylog.py");

/// Первый существующий из `DAYLOG_PYTHON` и списка `PYTHON_CANDIDATES`.
pub fn resolve_python(daylog_python: Option<String>, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    daylog_python
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain(PYTHON_CANDIDATES.iter().map(PathBuf::from))
        .find(|p| exists(p))
}

/// `DAYLOG_SCRIPT`, иначе путь, зашитый при сборке.
pub fn resolve_script(daylog_script: Option<String>) -> PathBuf {
    match daylog_script.filter(|s| !s.is_empty()) {
        Some(s) => PathBuf::from(s),
        None => {
            let p = PathBuf::from(BUILT_IN_SCRIPT);
            p.canonicalize().unwrap_or(p)
        }
    }
}

/// PATH дочернего процесса: текущий плюс `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`.
pub fn child_path(current: Option<String>, home: Option<String>) -> String {
    let current = current.filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    let mut parts: Vec<String> = current.split(':').filter(|s| !s.is_empty()).map(str::to_owned).collect();
    let mut extra = Vec::new();
    if let Some(home) = home.filter(|s| !s.is_empty()) {
        extra.push(format!("{home}/.local/bin"));
    }
    extra.push("/opt/homebrew/bin".into());
    extra.push("/usr/local/bin".into());
    for e in extra {
        if !parts.contains(&e) {
            parts.push(e);
        }
    }
    parts.join(":")
}

/// Всё, что нужно для запуска `daylog.py`: определяется один раз при старте приложения.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub data_dir: PathBuf,
    pub python: Option<PathBuf>,
    pub script: PathBuf,
    pub path_var: String,
}

impl Runtime {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok();
        Runtime {
            data_dir: data::data_dir_from_env(),
            python: resolve_python(var("DAYLOG_PYTHON"), |p| p.is_file()),
            script: resolve_script(var("DAYLOG_SCRIPT")),
            path_var: child_path(var("PATH"), var("HOME")),
        }
    }

    /// `python3 daylog.py <args>` с `DAYLOG_DIR` и дополненным PATH, stdin — `/dev/null`.
    pub fn command(&self, args: &[&str]) -> Result<Command, String> {
        let python = self.python.as_ref().ok_or_else(|| {
            format!("не найден python3 (искал DAYLOG_PYTHON и {}); задайте DAYLOG_PYTHON", PYTHON_CANDIDATES.join(", "))
        })?;
        let mut cmd = Command::new(python);
        cmd.arg(&self.script)
            .args(args)
            .env("DAYLOG_DIR", &self.data_dir)
            .env("PATH", &self.path_var)
            .stdin(Stdio::null());
        Ok(cmd)
    }
}

/// Последние `max_chars` символов текста (по границе символа).
pub fn tail(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let skip = count - max_chars;
    format!("…{}", text.chars().skip(skip).collect::<String>())
}

/// Хвост файла, начиная с `offset` (что дописано после запуска процесса).
pub fn file_tail(path: &Path, offset: u64, max_chars: usize) -> String {
    let mut buf = Vec::new();
    if let Ok(mut f) = fs::File::open(path) {
        let _ = f.seek(SeekFrom::Start(offset));
        let _ = f.read_to_end(&mut buf);
    }
    tail(&String::from_utf8_lossy(&buf), max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_from_env_var_when_it_exists() {
        let exists = |p: &Path| p == Path::new("/my/python3") || p == Path::new("/usr/bin/python3");
        assert_eq!(resolve_python(Some("/my/python3".into()), exists), Some("/my/python3".into()));
    }

    #[test]
    fn python_falls_back_to_list_in_order() {
        let exists = |p: &Path| p == Path::new("/usr/local/bin/python3") || p == Path::new("/usr/bin/python3");
        assert_eq!(resolve_python(Some("/нет/python3".into()), exists), Some("/usr/local/bin/python3".into()));
        assert_eq!(resolve_python(None, exists), Some("/usr/local/bin/python3".into()));
        assert_eq!(resolve_python(Some(String::new()), |_| false), None);
    }

    #[test]
    fn script_from_env_var_or_built_in() {
        assert_eq!(resolve_script(Some("/x/daylog.py".into())), PathBuf::from("/x/daylog.py"));
        let built_in = resolve_script(None);
        assert!(built_in.ends_with("daylog.py"), "{built_in:?}");
        assert!(built_in.is_file(), "daylog.py рядом с app/: {built_in:?}");
        assert_eq!(resolve_script(Some(String::new())), built_in);
    }

    #[test]
    fn child_path_adds_missing_dirs_once() {
        assert_eq!(
            child_path(Some("/usr/bin:/bin:/opt/homebrew/bin".into()), Some("/Users/u".into())),
            "/usr/bin:/bin:/opt/homebrew/bin:/Users/u/.local/bin:/usr/local/bin"
        );
        assert_eq!(
            child_path(None, None),
            "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin"
        );
    }

    #[test]
    fn command_without_python_is_an_error() {
        let rt = Runtime { data_dir: "/d".into(), python: None, script: "/s".into(), path_var: String::new() };
        assert!(rt.command(&["run"]).unwrap_err().contains("DAYLOG_PYTHON"));
    }

    #[test]
    fn tail_keeps_last_chars() {
        assert_eq!(tail("  abc  ", 5), "abc");
        assert_eq!(tail("абвгд", 2), "…гд");
    }
}
