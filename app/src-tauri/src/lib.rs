pub mod collector;
pub mod data;
pub mod pauses;
pub mod procs;
pub mod report;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

use collector::{Collector, CollectorError, CollectorStatus};
use pauses::{PausePrompt, Prompts};
use procs::Runtime;
use report::{ClassifyJobs, ClassifyStart, ReportJobs};

/// Метка окна с вопросом о причине паузы (`tauri.conf.json`).
const PROMPT_WINDOW: &str = "pause-prompt";

struct AppState {
    rt: Runtime,
    collector: Collector,
    jobs: Arc<ReportJobs>,
    classify: Arc<ClassifyJobs>,
    /// Вопрос о причине паузы: о какой паузе спрашивать, отложен ли он, показано ли окно.
    prompts: Prompts,
    /// `state.json` в каталоге конфигурации приложения: `{"recording": bool}`.
    state_file: PathBuf,
    resume_attempted: AtomicBool,
}

impl AppState {
    fn save_recording(&self, recording: bool) {
        if let Err(e) = collector::write_recording(&self.state_file, recording) {
            eprintln!("daylog: не удалось записать {}: {e}", self.state_file.display());
        }
    }
}

#[tauri::command(async)]
fn list_days(s: State<AppState>) -> Result<Vec<data::DaySummary>, String> {
    data::list_days(&s.rt.data_dir).map_err(|e| format!("не удалось прочитать {}: {e}", s.rt.data_dir.display()))
}

#[tauri::command(async)]
fn get_day(s: State<AppState>, day: String) -> Result<data::Day, String> {
    data::get_day(&s.rt.data_dir, &day)
}

#[tauri::command(async)]
fn collector_status(s: State<AppState>) -> Result<CollectorStatus, String> {
    s.collector.status(&s.rt.data_dir)
}

fn join_error(e: impl std::fmt::Display) -> CollectorError {
    CollectorError { kind: collector::ErrorKind::Failed, message: e.to_string() }
}

/// Запуск сборщика в фоновом потоке: ожидание до 3 секунд не должно держать главный поток.
async fn start_in_background(app: AppHandle) -> Result<CollectorStatus, CollectorError> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = app.state::<AppState>();
        s.collector.start(&s.rt, collector::check_screen_permission, collector::START_WAIT)
    })
    .await
    .map_err(join_error)?
}

#[tauri::command]
async fn start_collector(app: AppHandle) -> Result<CollectorStatus, CollectorError> {
    app.state::<AppState>().save_recording(true);
    start_in_background(app).await
}

#[tauri::command]
async fn stop_collector(app: AppHandle) -> Result<CollectorStatus, CollectorError> {
    app.state::<AppState>().save_recording(false);
    tauri::async_runtime::spawn_blocking(move || {
        let s = app.state::<AppState>();
        s.collector.stop(&s.rt.data_dir, collector::STOP_WAIT)
    })
    .await
    .map_err(join_error)?
}

/// Возобновление записи при запуске приложения: один раз за процесс, если при прошлом
/// выходе запись шла и не была остановлена кнопкой. `None` — возобновлять нечего.
#[tauri::command]
async fn resume_collector(app: AppHandle) -> Result<Option<CollectorStatus>, CollectorError> {
    let s = app.state::<AppState>();
    if s.resume_attempted.swap(true, Ordering::SeqCst) || !collector::read_recording(&s.state_file) {
        return Ok(None);
    }
    start_in_background(app.clone()).await.map(Some)
}

#[tauri::command]
fn open_screen_recording_settings() -> Result<(), String> {
    std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .status()
        .map_err(|e| format!("не удалось открыть System Settings: {e}"))
        .map(|_| ())
}

#[tauri::command(async)]
fn generate_report(app: AppHandle, s: State<AppState>, day: String) -> Result<(), String> {
    report::start_report(s.rt.clone(), s.jobs.clone(), day, move |finished| {
        let _ = app.emit("report-finished", finished);
    })
}

#[tauri::command]
fn reports_in_progress(s: State<AppState>) -> Vec<String> {
    s.jobs.list()
}

/// Доразметка категорий дня: не больше одной на день и одной за запуск приложения.
/// Результат — событие `classify-finished`.
#[tauri::command(async)]
fn classify_day(app: AppHandle, s: State<AppState>, day: String) -> Result<ClassifyStart, String> {
    report::start_classify(s.rt.clone(), s.classify.clone(), day, move |finished| {
        let _ = app.emit("classify-finished", finished);
    })
}

/// Применяет результат пересчёта вопроса: отправляет вопрос окну, показывает или прячет
/// его и обновляет число на значке в Dock.
///
/// Окно показывается через `show()` без `set_focus()`: приложение при этом не
/// активируется, и ввод с клавиатуры остаётся у приложения, в котором работает пользователь.
fn apply_prompt_update(app: &AppHandle, update: pauses::Update) {
    if let Some(prompt) = update.prompt {
        let _ = app.emit("pause-prompt", prompt);
    }
    if let (Some(show), Some(window)) = (update.window, app.get_webview_window(PROMPT_WINDOW)) {
        let _ = if show { window.show() } else { window.hide() };
    }
    if let (Some(count), Some(window)) = (update.badge, app.get_webview_window("main")) {
        let _ = window.set_badge_count((count > 0).then_some(count as i64));
    }
}

/// Пересчитывает вопрос по сегодняшнему дню и времени без ввода. Зовётся фоновым потоком
/// раз в `pauses::TICK` и сразу после записи причины.
fn refresh_prompt(app: &AppHandle) {
    let s = app.state::<AppState>();
    let update = s.prompts.refresh(&s.rt.data_dir, &pauses::today(), pauses::idle_seconds(), Instant::now());
    apply_prompt_update(app, update);
}

/// «Позже»: прячет окно с вопросом и откладывает его.
fn snooze_prompt(app: &AppHandle) {
    let update = app.state::<AppState>().prompts.snooze(Instant::now());
    apply_prompt_update(app, update);
}

/// Причина паузы за отрезок дня. Файл дня приложение не трогает: строку дописывает
/// `daylog.py pause`. При успехе — событие `pauses-changed` с днём, и окно с вопросом
/// сразу переходит к следующей паузе без причины.
#[tauri::command(async)]
fn set_pause_reason(
    app: AppHandle,
    s: State<AppState>,
    day: String,
    start: String,
    end: String,
    reason: String,
) -> Result<(), String> {
    pauses::write_reason(&s.rt, &day, &start, &end, &reason)?;
    let _ = app.emit("pauses-changed", pauses::PausesChanged { day });
    refresh_prompt(&app);
    Ok(())
}

/// Вопрос, который сейчас должно показывать окно `pause-prompt`; `None` — спрашивать не о чем.
#[tauri::command]
fn pause_prompt(s: State<AppState>) -> Option<PausePrompt> {
    s.prompts.current()
}

#[tauri::command]
fn snooze_pause_prompt(app: AppHandle) {
    snooze_prompt(&app);
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let state_file = app.path().app_config_dir()?.join("state.json");
            app.manage(AppState {
                rt: Runtime::from_env(),
                collector: Collector::default(),
                jobs: Arc::new(ReportJobs::default()),
                classify: Arc::new(ClassifyJobs::default()),
                prompts: Prompts::default(),
                state_file,
                resume_attempted: AtomicBool::new(false),
            });
            // Вопрос о причине паузы не зависит от окон: поток следит за данными дня и
            // временем без ввода, даже когда главное окно закрыто. Первый пересчёт — через
            // один шаг, чтобы окно с вопросом успело загрузиться.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(pauses::TICK);
                refresh_prompt(&handle);
            });
            Ok(())
        })
        // Закрытие окна только прячет его: запись продолжается, окно возвращает клик по Dock.
        // Закрытие окна с вопросом — то же, что «Позже».
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == PROMPT_WINDOW {
                    snooze_prompt(window.app_handle());
                } else {
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_days,
            get_day,
            collector_status,
            start_collector,
            stop_collector,
            resume_collector,
            open_screen_recording_settings,
            generate_report,
            reports_in_progress,
            classify_day,
            set_pause_reason,
            pause_prompt,
            snooze_pause_prompt,
        ])
        .build(tauri::generate_context!())
        .expect("не удалось запустить daylog");

    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main_window(app),
        // Выход останавливает только сборщик, запущенный этим приложением; значение
        // `recording` не меняется, поэтому при следующем запуске запись возобновится.
        RunEvent::ExitRequested { .. } | RunEvent::Exit => {
            let s = app.state::<AppState>();
            if let Some(Err(e)) = s.collector.stop_owned(&s.rt.data_dir, collector::STOP_WAIT) {
                eprintln!("daylog: при выходе не удалось остановить сборщик: {}", e.message);
            }
        }
        _ => {}
    });
}
