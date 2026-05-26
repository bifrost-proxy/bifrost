use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use bifrost_core::{BifrostError, Result};
use chrono::TimeZone;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use dialoguer::Select;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::CrosstermBackend,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Row, Table, Wrap},
    Frame, Terminal,
};
use serde::Deserialize;
use serde_json::Value;

use super::asr::AsrTaskClient;

#[derive(Debug, Clone)]
pub(crate) struct AsrTaskTuiOptions {
    pub(crate) task: Option<String>,
    pub(crate) refresh_ms: u64,
    pub(crate) no_interactive_select: bool,
    pub(crate) all: bool,
    pub(crate) json_snapshot: bool,
    pub(crate) read_only: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchListResponse {
    #[serde(default)]
    tasks: Vec<WatchSnapshot>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchSnapshot {
    #[serde(default)]
    task: WatchTask,
    #[serde(default)]
    progress: WatchProgress,
    #[serde(default)]
    consumption: WatchConsumption,
    #[serde(default)]
    service: Option<WatchService>,
    #[serde(default)]
    recent_files: Vec<WatchFile>,
    #[serde(default)]
    daily_agent: WatchDailyAgent,
    #[serde(default)]
    snapshot_source: String,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    updated_at_ms: u64,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchTask {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    audio_dir: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    schedule: Value,
    #[serde(default)]
    language: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    runtime_strategy: String,
    #[serde(default)]
    last_run_at_ms: Option<i64>,
    #[serde(default)]
    next_run_at_ms: Option<i64>,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchProgress {
    #[serde(default)]
    discovered: usize,
    #[serde(default)]
    processed: usize,
    #[serde(default)]
    pending: usize,
    #[serde(default)]
    failed: usize,
    #[serde(default)]
    partial_success: usize,
    #[serde(default)]
    failed_chunk_count: usize,
    #[serde(default)]
    file_percent: f64,
    #[serde(default)]
    current_source_path: Option<String>,
    #[serde(default)]
    current_file_index: usize,
    #[serde(default)]
    current_file_total: usize,
    #[serde(default)]
    current_chunk_done: usize,
    #[serde(default)]
    current_chunk_total: usize,
    #[serde(default)]
    eta_ms: Option<u64>,
    #[serde(default)]
    eta_confidence: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchConsumption {
    #[serde(default)]
    source_bytes_total: u64,
    #[serde(default)]
    source_bytes_processed: u64,
    #[serde(default)]
    source_bytes_processed_estimated: Option<u64>,
    #[serde(default)]
    audio_duration_ms_total: u64,
    #[serde(default)]
    audio_duration_ms_processed: u64,
    #[serde(default)]
    audio_duration_ms_processed_estimated: Option<u64>,
    #[serde(default)]
    inference_elapsed_ms: u64,
    #[serde(default)]
    average_rtf: Option<f64>,
    #[serde(default)]
    last_chunk_rtf: Option<f64>,
    #[serde(default)]
    text_chars: usize,
    #[serde(default)]
    chunks_completed: usize,
    #[serde(default)]
    chunks_failed: usize,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchService {
    #[serde(default)]
    server_url: String,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    owner_module: String,
    #[serde(default)]
    owner_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchFile {
    #[serde(default)]
    source_path: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    source_size: Option<u64>,
    #[serde(default)]
    media_duration_ms: Option<u64>,
    #[serde(default)]
    progress_current: Option<usize>,
    #[serde(default)]
    progress_total: Option<usize>,
    #[serde(default)]
    text_chars: usize,
    #[serde(default)]
    last_chunk_rtf: Option<f64>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchDailyAgent {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    runner: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    last_run_id: Option<String>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    last_run_at_ms: Option<i64>,
    #[serde(default)]
    daily_files: usize,
    #[serde(default)]
    processed_documents: usize,
    #[serde(default)]
    pending_documents: usize,
    #[serde(default)]
    report_files: usize,
    #[serde(default)]
    indexed_reports: usize,
    #[serde(default)]
    unindexed_reports: usize,
    #[serde(default)]
    processed_missing_report: usize,
    #[serde(default)]
    recent_documents: Vec<WatchDailyAgentDocument>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct WatchDailyAgentDocument {
    #[serde(default)]
    date: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    change_kind: String,
    #[serde(default)]
    source_path: Option<String>,
    #[serde(default)]
    report_path: Option<String>,
    #[serde(default)]
    source_len_bytes: Option<u64>,
    #[serde(default)]
    processed_at_ms: Option<i64>,
    #[serde(default)]
    runner: Option<String>,
    #[serde(default)]
    last_run_id: Option<String>,
}

pub(crate) fn handle_asr_task_tui(
    client: &AsrTaskClient,
    options: AsrTaskTuiOptions,
) -> Result<()> {
    let selected = select_initial_task(client, &options)?;
    if options.json_snapshot {
        let value = match selected {
            SelectedTask::All => client.get_json("/asr/tasks/-/watch")?,
            SelectedTask::One(task_id) => client.get_json(&format!(
                "/asr/tasks/{}/watch",
                urlencoding::encode(&task_id)
            ))?,
        };
        print_json(&value)?;
        return Ok(());
    }

    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        return Err(BifrostError::Config(
            "ASR task TUI requires an interactive terminal; use --json-snapshot for non-TTY output"
                .to_string(),
        ));
    }

    run_tui(client, selected, options)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectedTask {
    All,
    One(String),
}

fn select_initial_task(
    client: &AsrTaskClient,
    options: &AsrTaskTuiOptions,
) -> Result<SelectedTask> {
    if options.all {
        return Ok(SelectedTask::All);
    }
    let list_value = client.get_json("/asr/tasks/-/watch")?;
    let list: WatchListResponse = serde_json::from_value(list_value)
        .map_err(|error| BifrostError::Config(format!("parse ASR task watch list: {error}")))?;
    if let Some(query) = options.task.as_deref() {
        let task = resolve_task_query(&list.tasks, query)?;
        return Ok(SelectedTask::One(task.task.id.clone()));
    }
    match list.tasks.len() {
        0 => Err(BifrostError::Config("No ASR directory tasks.".to_string())),
        1 => Ok(SelectedTask::One(list.tasks[0].task.id.clone())),
        _ if options.no_interactive_select || !io::stdin().is_terminal() => Err(
            BifrostError::Config(
                "Multiple ASR directory tasks exist; pass a task id, unique id prefix, unique name, or --all"
                    .to_string(),
            ),
        ),
        _ => {
            let items = list
                .tasks
                .iter()
                .map(selection_label)
                .collect::<Vec<_>>();
            let index = Select::new()
                .with_prompt("Select ASR task to watch")
                .items(&items)
                .default(0)
                .interact()
                .map_err(|error| BifrostError::Io(io::Error::other(error)))?;
            Ok(SelectedTask::One(list.tasks[index].task.id.clone()))
        }
    }
}

fn resolve_task_query<'a>(tasks: &'a [WatchSnapshot], query: &str) -> Result<&'a WatchSnapshot> {
    if let Some(task) = tasks.iter().find(|task| task.task.id == query) {
        return Ok(task);
    }
    let prefix_matches = tasks
        .iter()
        .filter(|task| task.task.id.starts_with(query))
        .collect::<Vec<_>>();
    if prefix_matches.len() == 1 {
        return Ok(prefix_matches[0]);
    }
    if prefix_matches.len() > 1 {
        return Err(BifrostError::Config(format!(
            "ambiguous ASR task id prefix '{query}'; use the full task id"
        )));
    }
    let name_matches = tasks
        .iter()
        .filter(|task| task.task.name == query)
        .collect::<Vec<_>>();
    if name_matches.len() == 1 {
        return Ok(name_matches[0]);
    }
    if name_matches.len() > 1 {
        return Err(BifrostError::Config(format!(
            "ambiguous ASR task name '{query}'; use a task id"
        )));
    }
    Err(BifrostError::Config(format!(
        "ASR task '{query}' not found"
    )))
}

fn run_tui(
    client: &AsrTaskClient,
    selected: SelectedTask,
    options: AsrTaskTuiOptions,
) -> Result<()> {
    let _terminal_guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(BifrostError::Io)?;
    let mut app = TuiApp::new(selected, options);
    app.refresh(client);

    loop {
        terminal
            .draw(|frame| render(frame, &mut app))
            .map_err(BifrostError::Io)?;
        if app.should_quit {
            break;
        }
        let poll_interval = Duration::from_millis(100);
        if event::poll(poll_interval).map_err(BifrostError::Io)? {
            match event::read().map_err(BifrostError::Io)? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(client, key.code);
                }
                Event::Mouse(mouse) => {
                    app.handle_mouse(mouse.kind, mouse.column, mouse.row);
                }
                _ => {}
            }
        }
        if app.last_refresh.elapsed() >= app.refresh_interval() {
            app.refresh(client);
        }
    }
    Ok(())
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(BifrostError::Io)?;
        io::stdout()
            .execute(EnterAlternateScreen)
            .and_then(|stdout| stdout.execute(EnableMouseCapture))
            .map_err(BifrostError::Io)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = io::stdout()
            .execute(DisableMouseCapture)
            .and_then(|stdout| stdout.execute(LeaveAlternateScreen));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTable {
    Files,
    DailyDocs,
}

struct TuiApp {
    selected: SelectedTask,
    options: AsrTaskTuiOptions,
    snapshot: Option<WatchSnapshot>,
    list: Vec<WatchSnapshot>,
    selected_index: usize,
    detail_table: DetailTable,
    selected_file_index: usize,
    selected_doc_index: usize,
    last_table_area: Option<Rect>,
    last_refresh: Instant,
    message: Option<String>,
    should_quit: bool,
}

impl TuiApp {
    fn new(selected: SelectedTask, options: AsrTaskTuiOptions) -> Self {
        Self {
            selected,
            options,
            snapshot: None,
            list: Vec::new(),
            selected_index: 0,
            detail_table: DetailTable::Files,
            selected_file_index: 0,
            selected_doc_index: 0,
            last_table_area: None,
            last_refresh: Instant::now() - Duration::from_secs(10),
            message: None,
            should_quit: false,
        }
    }

    fn refresh_interval(&self) -> Duration {
        let ms = self.options.refresh_ms.max(500);
        if self.selected == SelectedTask::All {
            Duration::from_millis(ms.max(2_000))
        } else {
            Duration::from_millis(ms)
        }
    }

    fn refresh(&mut self, client: &AsrTaskClient) {
        let result = match &self.selected {
            SelectedTask::All => client
                .get_json("/asr/tasks/-/watch")
                .and_then(|value| {
                    serde_json::from_value::<WatchListResponse>(value).map_err(|error| {
                        BifrostError::Config(format!("parse ASR task watch list: {error}"))
                    })
                })
                .map(|list| {
                    self.list = list.tasks;
                }),
            SelectedTask::One(task_id) => client
                .get_json(&format!(
                    "/asr/tasks/{}/watch",
                    urlencoding::encode(task_id)
                ))
                .and_then(|value| {
                    serde_json::from_value::<WatchSnapshot>(value).map_err(|error| {
                        BifrostError::Config(format!("parse ASR task watch snapshot: {error}"))
                    })
                })
                .map(|snapshot| {
                    self.selected_file_index = self
                        .selected_file_index
                        .min(snapshot.recent_files.len().saturating_sub(1));
                    self.selected_doc_index = self.selected_doc_index.min(
                        snapshot
                            .daily_agent
                            .recent_documents
                            .len()
                            .saturating_sub(1),
                    );
                    if snapshot.recent_files.is_empty()
                        && !snapshot.daily_agent.recent_documents.is_empty()
                    {
                        self.detail_table = DetailTable::DailyDocs;
                    }
                    self.snapshot = Some(snapshot);
                }),
        };
        match result {
            Ok(()) => self.message = None,
            Err(error) => self.message = Some(error.to_string()),
        }
        self.last_refresh = Instant::now();
    }

    fn handle_key(&mut self, client: &AsrTaskClient, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('r') => self.refresh(client),
            KeyCode::Down if self.selected == SelectedTask::All && !self.list.is_empty() => {
                self.selected_index = (self.selected_index + 1).min(self.list.len() - 1);
            }
            KeyCode::Up if self.selected == SelectedTask::All => {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            KeyCode::Enter if self.selected == SelectedTask::All => {
                if let Some(task) = self.list.get(self.selected_index) {
                    self.selected = SelectedTask::One(task.task.id.clone());
                    self.snapshot = None;
                    self.refresh(client);
                }
            }
            KeyCode::Tab if self.selected != SelectedTask::All => self.toggle_detail_table(),
            KeyCode::Down if self.selected != SelectedTask::All => self.move_detail_selection(1),
            KeyCode::Up if self.selected != SelectedTask::All => self.move_detail_selection(-1),
            KeyCode::Enter if self.selected != SelectedTask::All => self.open_selected_path(false),
            KeyCode::Char('o') if self.selected != SelectedTask::All => {
                self.open_selected_path(true)
            }
            KeyCode::Char('R') => self.run_now(client),
            KeyCode::Char('p') => self.toggle_pause(client, false),
            KeyCode::Char('P') => self.toggle_pause(client, true),
            _ => {}
        }
    }

    fn handle_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        if self.selected == SelectedTask::All {
            return;
        }
        let MouseEventKind::Down(MouseButton::Left) = kind else {
            return;
        };
        let Some(area) = self.last_table_area else {
            return;
        };
        if column <= area.x
            || column >= area.x.saturating_add(area.width)
            || row <= area.y.saturating_add(1)
            || row >= area.y.saturating_add(area.height).saturating_sub(1)
        {
            return;
        }
        let index = row.saturating_sub(area.y).saturating_sub(2) as usize;
        match self.detail_table {
            DetailTable::Files => self.selected_file_index = index,
            DetailTable::DailyDocs => self.selected_doc_index = index,
        }
        self.open_selected_path(false);
    }

    fn toggle_detail_table(&mut self) {
        self.detail_table = match self.detail_table {
            DetailTable::Files => DetailTable::DailyDocs,
            DetailTable::DailyDocs => DetailTable::Files,
        };
    }

    fn move_detail_selection(&mut self, delta: isize) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let len = match self.detail_table {
            DetailTable::Files => snapshot.recent_files.len(),
            DetailTable::DailyDocs => snapshot.daily_agent.recent_documents.len(),
        };
        if len == 0 {
            return;
        }
        let selected = match self.detail_table {
            DetailTable::Files => &mut self.selected_file_index,
            DetailTable::DailyDocs => &mut self.selected_doc_index,
        };
        if delta < 0 {
            *selected = selected.saturating_sub(delta.unsigned_abs());
        } else {
            *selected = (*selected + delta as usize).min(len - 1);
        }
    }

    fn open_selected_path(&mut self, reveal: bool) {
        let Some(path) = self.selected_open_path() else {
            self.message = Some("No file selected.".to_string());
            return;
        };
        match open_path(&path, reveal) {
            Ok(()) => {
                self.message = Some(if reveal {
                    format!("Revealed {}", short_path(&path))
                } else {
                    format!("Opened {}", short_path(&path))
                });
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn selected_open_path(&self) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        match self.detail_table {
            DetailTable::Files => snapshot
                .recent_files
                .get(self.selected_file_index)
                .map(|file| file.source_path.clone()),
            DetailTable::DailyDocs => snapshot
                .daily_agent
                .recent_documents
                .get(self.selected_doc_index)
                .and_then(|document| {
                    document
                        .report_path
                        .clone()
                        .or_else(|| document.source_path.clone())
                }),
        }
    }

    fn selected_task_id(&self) -> Option<&str> {
        match &self.selected {
            SelectedTask::One(task_id) => Some(task_id.as_str()),
            SelectedTask::All => None,
        }
    }

    fn run_now(&mut self, client: &AsrTaskClient) {
        if self.options.read_only {
            self.message = Some("read-only mode: run shortcut disabled".to_string());
            return;
        }
        let Some(task_id) = self.selected_task_id() else {
            self.message = Some("Select one ASR task before requesting a run.".to_string());
            return;
        };
        if let Some(task) = self.snapshot.as_ref().map(|snapshot| &snapshot.task) {
            if task.running {
                self.message = Some(
                    "ASR task is already running; refresh shows current progress.".to_string(),
                );
                return;
            }
            if task.paused {
                self.message =
                    Some("ASR task is paused; press p to resume before running.".to_string());
                return;
            }
        }
        match client.post_json(&format!("/asr/tasks/{}/run", urlencoding::encode(task_id))) {
            Ok(value) => {
                self.message = Some(
                    json_message(&value).unwrap_or_else(|| "ASR task run requested".to_string()),
                );
                self.refresh(client);
            }
            Err(error) => self.message = Some(tui_control_error_message(&error)),
        }
    }

    fn toggle_pause(&mut self, client: &AsrTaskClient, force: bool) {
        if self.options.read_only {
            self.message = Some("read-only mode: pause/resume shortcut disabled".to_string());
            return;
        }
        let Some(task_id) = self.selected_task_id() else {
            self.message = Some("Select one ASR task before changing pause state.".to_string());
            return;
        };
        let paused = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.task.paused)
            .unwrap_or(false);
        let path = pause_control_path(paused, force);
        match client.post_json(&format!(
            "/asr/tasks/{}/{}",
            urlencoding::encode(task_id),
            path
        )) {
            Ok(value) => {
                self.message = Some(json_message(&value).unwrap_or_else(|| {
                    if paused {
                        "ASR task resume requested".to_string()
                    } else if force {
                        "ASR task force-pause requested".to_string()
                    } else {
                        "ASR task temporary pause requested".to_string()
                    }
                }));
                self.refresh(client);
            }
            Err(error) => self.message = Some(tui_control_error_message(&error)),
        }
    }
}

fn pause_control_path(paused: bool, force: bool) -> &'static str {
    if paused {
        "resume"
    } else if force {
        "pause?mode=long_term&force=true"
    } else {
        "pause?mode=temporary"
    }
}

fn json_message(value: &Value) -> Option<String> {
    value
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
}

fn tui_control_error_message(error: &BifrostError) -> String {
    humanize_api_error(&error.to_string())
}

fn humanize_api_error(message: &str) -> String {
    let message = message.strip_prefix("Config error: ").unwrap_or(message);
    let Some(json_start) = message.find('{') else {
        return message.to_string();
    };
    let Ok(value) = serde_json::from_str::<Value>(&message[json_start..]) else {
        return message.to_string();
    };
    json_message(&value).unwrap_or_else(|| message.to_string())
}

fn render(frame: &mut Frame, app: &mut TuiApp) {
    match app.selected {
        SelectedTask::All => render_all(frame, app),
        SelectedTask::One(_) => render_one(frame, app),
    }
}

fn render_all(frame: &mut Frame, app: &TuiApp) {
    let area = frame.area();
    let rows = app.list.iter().enumerate().map(|(index, snapshot)| {
        let marker = if index == app.selected_index {
            ">"
        } else {
            " "
        };
        Row::new(vec![
            format!("{marker} {}", truncate(&snapshot.task.name, 24)),
            task_state(&snapshot.task).to_string(),
            format!(
                "{}/{} {:.0}%",
                snapshot.progress.processed,
                snapshot.progress.discovered,
                snapshot.progress.file_percent
            ),
            format_optional_ms(snapshot.task.next_run_at_ms),
            snapshot
                .consumption
                .average_rtf
                .map(|value| format!("{value:.2} rtf"))
                .unwrap_or_else(|| "-".to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(32),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(20),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["Task", "State", "Progress", "Next run", "Cost"]).style(Style::new().bold()),
    )
    .block(
        Block::default()
            .title("ASR Scheduled Tasks")
            .borders(Borders::ALL),
    );
    frame.render_widget(table, area);
}

fn render_one(frame: &mut Frame, app: &mut TuiApp) {
    let area = frame.area();
    let Some(snapshot) = app.snapshot.clone() else {
        let paragraph = Paragraph::new("Loading ASR task snapshot...")
            .block(Block::default().title("ASR Task").borders(Borders::ALL));
        frame.render_widget(paragraph, area);
        return;
    };
    app.last_table_area = None;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(area);
    render_header(frame, vertical[0], &snapshot);
    render_progress(frame, vertical[1], &snapshot);
    render_consumption(frame, vertical[2], &snapshot);
    render_daily_agent(frame, vertical[3], &snapshot);
    app.last_table_area = Some(vertical[4]);
    match app.detail_table {
        DetailTable::Files => {
            render_recent_files(frame, vertical[4], &snapshot, app.selected_file_index)
        }
        DetailTable::DailyDocs => {
            render_daily_agent_documents(frame, vertical[4], &snapshot, app.selected_doc_index)
        }
    }
    render_footer(frame, vertical[5], app);
}

fn render_header(frame: &mut Frame, area: Rect, snapshot: &WatchSnapshot) {
    let title = format!(
        "ASR Task: {} ({})",
        snapshot.task.name,
        task_state(&snapshot.task)
    );
    let lines = vec![
        Line::from(vec![
            Span::raw(format!("ID {}  ", truncate(&snapshot.task.id, 12))),
            Span::raw(format!("Model {}  ", snapshot.task.model)),
            Span::raw(format!("Lang {}  ", snapshot.task.language)),
            Span::raw(format!("Runtime {}  ", snapshot.task.runtime_strategy)),
            Span::raw(format!(
                "Schedule {}",
                format_value(&snapshot.task.schedule)
            )),
        ]),
        Line::from(format!(
            "Dir {}  Last {}  Next {}  Snapshot {}  Updated {}",
            short_path(&snapshot.task.audio_dir),
            format_optional_ms(snapshot.task.last_run_at_ms),
            format_optional_ms(snapshot.task.next_run_at_ms),
            snapshot.snapshot_source,
            format_ms(snapshot.updated_at_ms as i64)
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

fn render_progress(frame: &mut Frame, area: Rect, snapshot: &WatchSnapshot) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(area);
    let file_gauge = Gauge::default()
        .block(Block::default().borders(Borders::NONE))
        .gauge_style(Style::default().fg(Color::Green))
        .percent(snapshot.progress.file_percent.min(100.0) as u16)
        .label(format!(
            "Files {}/{} {:.0}%  pending {} failed {}",
            snapshot.progress.processed,
            snapshot.progress.discovered,
            snapshot.progress.file_percent,
            snapshot.progress.pending,
            snapshot.progress.failed
        ));
    frame.render_widget(file_gauge, chunks[0]);

    let chunk_percent = if snapshot.progress.current_chunk_total > 0 {
        snapshot.progress.current_chunk_done as f64 / snapshot.progress.current_chunk_total as f64
            * 100.0
    } else {
        0.0
    };
    let chunk_gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Cyan))
        .percent(chunk_percent.min(100.0) as u16)
        .label(format!(
            "Chunk {}/{}  file {}/{}",
            snapshot.progress.current_chunk_done,
            snapshot.progress.current_chunk_total,
            snapshot.progress.current_file_index,
            snapshot.progress.current_file_total
        ));
    frame.render_widget(chunk_gauge, chunks[1]);

    let eta = snapshot
        .progress
        .eta_ms
        .map(format_duration_ms)
        .unwrap_or_else(|| "-".to_string());
    let current = snapshot
        .progress
        .current_source_path
        .as_deref()
        .map(short_path)
        .unwrap_or_else(|| "-".to_string());
    frame.render_widget(
        Paragraph::new(format!(
            "Current {current}  partial {} failed_chunks {} eta {} ({})",
            snapshot.progress.partial_success,
            snapshot.progress.failed_chunk_count,
            eta,
            snapshot.progress.eta_confidence
        )),
        chunks[2],
    );
}

fn render_consumption(frame: &mut Frame, area: Rect, snapshot: &WatchSnapshot) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let c = &snapshot.consumption;
    let left = vec![
        ListItem::new(format!(
            "Audio processed: {} / {}",
            format_duration_ms(
                c.audio_duration_ms_processed_estimated
                    .unwrap_or(c.audio_duration_ms_processed)
            ),
            format_duration_ms(c.audio_duration_ms_total)
        )),
        ListItem::new(format!(
            "Inference elapsed: {}",
            format_duration_ms(c.inference_elapsed_ms)
        )),
        ListItem::new(format!(
            "Average RTF: {}",
            c.average_rtf
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "-".to_string())
        )),
        ListItem::new(format!(
            "Last chunk RTF: {}",
            c.last_chunk_rtf
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "-".to_string())
        )),
        ListItem::new(format!(
            "Source bytes: {} / {}",
            format_bytes(
                c.source_bytes_processed_estimated
                    .unwrap_or(c.source_bytes_processed)
            ),
            format_bytes(c.source_bytes_total)
        )),
        ListItem::new(format!(
            "Chunks: ok {} failed {}",
            c.chunks_completed, c.chunks_failed
        )),
        ListItem::new(format!("Text chars: {}", c.text_chars)),
    ];
    frame.render_widget(
        List::new(left).block(Block::default().title("Consumption").borders(Borders::ALL)),
        columns[0],
    );
    let service = snapshot
        .service
        .as_ref()
        .map(|service| {
            vec![
                ListItem::new(format!("Service: {}", service.server_url)),
                ListItem::new(format!(
                    "PID: {}",
                    service
                        .pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "-".to_string())
                )),
                ListItem::new(format!("Owner: {}", service.owner_module)),
                ListItem::new(format!(
                    "Owner id: {}",
                    service.owner_id.as_deref().unwrap_or("-")
                )),
            ]
        })
        .unwrap_or_else(|| vec![ListItem::new("Service: -")]);
    frame.render_widget(
        List::new(service).block(
            Block::default()
                .title("Current Runtime")
                .borders(Borders::ALL),
        ),
        columns[1],
    );
}

fn render_daily_agent(frame: &mut Frame, area: Rect, snapshot: &WatchSnapshot) {
    let agent = &snapshot.daily_agent;
    let status = agent.status.as_deref().unwrap_or("-");
    let last_run = format_optional_ms(agent.last_run_at_ms);
    let error = agent
        .last_error
        .as_deref()
        .map(|error| truncate(error, 80))
        .unwrap_or_else(|| "-".to_string());
    let lines = vec![
        Line::from(vec![
            Span::raw(format!("Enabled {}  ", agent.enabled)),
            Span::raw(format!("Runner {}  ", empty_dash(&agent.runner))),
            Span::raw(format!("Status {status}  ")),
            Span::raw(format!("Last {last_run}")),
        ]),
        Line::from(format!(
            "Processed {}/{}  pending {}  reports {} indexed {} unindexed {} missing_report {}",
            agent.processed_documents,
            agent.daily_files,
            agent.pending_documents,
            agent.report_files,
            agent.indexed_reports,
            agent.unindexed_reports,
            agent.processed_missing_report
        )),
        Line::from(format!(
            "Run {}  Error {}",
            agent.last_run_id.as_deref().unwrap_or("-"),
            error
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .title("Daily/Jennie Agent")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn render_recent_files(
    frame: &mut Frame,
    area: Rect,
    snapshot: &WatchSnapshot,
    selected_index: usize,
) {
    let rows = snapshot
        .recent_files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let progress = match (file.progress_current, file.progress_total) {
                (Some(current), Some(total)) if total > 0 => format!("{current}/{total}"),
                _ => "-".to_string(),
            };
            let source = file
                .error
                .as_ref()
                .map(|error| {
                    format!(
                        "{} ({})",
                        short_path(&file.source_path),
                        truncate(error, 48)
                    )
                })
                .unwrap_or_else(|| short_path(&file.source_path));
            let marker = if index == selected_index { ">" } else { " " };
            Row::new(vec![
                marker.to_string(),
                file.status.clone(),
                file.text_chars.to_string(),
                file.media_duration_ms
                    .map(format_duration_ms)
                    .unwrap_or_else(|| "-".to_string()),
                file.source_size
                    .map(format_bytes)
                    .unwrap_or_else(|| "-".to_string()),
                file.last_chunk_rtf
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_else(|| "-".to_string()),
                progress,
                source,
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Min(20),
        ],
    )
    .header(Row::new(vec![
        "", "Status", "Chars", "Duration", "Size", "RTF", "Progress", "Source",
    ]))
    .block(
        Block::default()
            .title("Recent Files (Tab switch, Enter open, o reveal, click open)")
            .borders(Borders::ALL),
    );
    frame.render_widget(table, area);
}

fn render_daily_agent_documents(
    frame: &mut Frame,
    area: Rect,
    snapshot: &WatchSnapshot,
    selected_index: usize,
) {
    let rows = snapshot
        .daily_agent
        .recent_documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let marker = if index == selected_index { ">" } else { " " };
            let path = document
                .report_path
                .as_deref()
                .or(document.source_path.as_deref())
                .map(short_path)
                .unwrap_or_else(|| "-".to_string());
            Row::new(vec![
                marker.to_string(),
                document.date.clone(),
                document.status.clone(),
                document.change_kind.clone(),
                document
                    .source_len_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "-".to_string()),
                format_optional_ms(document.processed_at_ms),
                document.runner.as_deref().unwrap_or("-").to_string(),
                document
                    .last_run_id
                    .as_deref()
                    .map(|run_id| truncate(run_id, 12))
                    .unwrap_or_else(|| "-".to_string()),
                path,
            ])
        });
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Length(19),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Min(20),
        ],
    )
    .header(Row::new(vec![
        "",
        "Date",
        "Status",
        "Change",
        "Size",
        "Processed",
        "Runner",
        "Run",
        "Path",
    ]))
    .block(
        Block::default()
            .title("Daily Agent Docs (Tab switch, Enter open, o reveal, click open)")
            .borders(Borders::ALL),
    );
    frame.render_widget(table, area);
}

fn render_footer(frame: &mut Frame, area: Rect, app: &TuiApp) {
    let mut text = if app.options.read_only {
        "q quit  r refresh  Tab files/docs  Enter open  o reveal  read-only".to_string()
    } else {
        "q quit  r refresh  R run now  p pause/resume  P force pause  Tab files/docs  Enter open  o reveal"
            .to_string()
    };
    if let Some(message) = &app.message {
        text.push_str("  |  ");
        text.push_str(message);
    }
    if let Some(snapshot) = &app.snapshot {
        if let Some(error) = &snapshot.task.last_error {
            text.push_str("  |  last error: ");
            text.push_str(&truncate(error, 80));
        }
        if !snapshot.warnings.is_empty() {
            text.push_str("  |  ");
            text.push_str(&snapshot.warnings.join("; "));
        }
    }
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), area);
}

fn selection_label(snapshot: &WatchSnapshot) -> String {
    format!(
        "{:<24} {:<10} {:>3}/{:<3} next {}",
        truncate(&snapshot.task.name, 24),
        task_state(&snapshot.task),
        snapshot.progress.processed,
        snapshot.progress.discovered,
        format_optional_ms(snapshot.task.next_run_at_ms)
    )
}

fn task_state(task: &WatchTask) -> &'static str {
    if task.running {
        "running"
    } else if task.paused {
        "paused"
    } else if !task.enabled {
        "disabled"
    } else {
        "enabled"
    }
}

fn print_json(value: &Value) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|error| BifrostError::Config(error.to_string()))?
    );
    Ok(())
}

fn format_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "-".to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "-".to_string()),
    }
}

fn format_optional_ms(value: Option<i64>) -> String {
    value.map(format_ms).unwrap_or_else(|| "-".to_string())
}

fn format_ms(value: i64) -> String {
    chrono::Local
        .timestamp_millis_opt(value)
        .earliest()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn format_duration_ms(value: u64) -> String {
    let total_secs = value / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn empty_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn open_path(path: &str, reveal: bool) -> std::result::Result<(), String> {
    if path.trim().is_empty() {
        return Err("No file path selected.".to_string());
    }
    let path_ref = Path::new(path);
    if !path_ref.exists() {
        return Err(format!("File does not exist: {path}"));
    }
    if let Ok(log_path) = std::env::var("BIFROST_ASR_TUI_OPEN_LOG") {
        let action = if reveal { "reveal" } else { "open" };
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| format!("open test log {log_path}: {error}"))?;
        writeln!(file, "{action}\t{path}")
            .map_err(|error| format!("write test log {log_path}: {error}"))?;
        return Ok(());
    }
    let mut command = open_path_command(path_ref, reveal)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("open {path}: {error}"))?;
    Ok(())
}

fn open_path_command(path: &Path, reveal: bool) -> std::result::Result<Command, String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        if reveal {
            command.arg("-R");
        }
        command.arg(path);
        return Ok(command);
    }
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("explorer");
        if reveal {
            command.arg(format!("/select,{}", path.display()));
        } else {
            command.arg(path);
        }
        return Ok(command);
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let target = if reveal {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let mut command = Command::new("xdg-open");
        command.arg(target);
        return Ok(command);
    }
    #[allow(unreachable_code)]
    Err("opening files is not supported on this platform".to_string())
}

fn short_path(path: &str) -> String {
    if path.len() <= 48 {
        return path.to_string();
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    let parent = path
        .rsplit('/')
        .nth(1)
        .map(|parent| format!("{parent}/"))
        .unwrap_or_default();
    truncate(&format!(".../{parent}{file}"), 48)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}~", value.chars().take(keep).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: &str, name: &str) -> WatchSnapshot {
        WatchSnapshot {
            task: WatchTask {
                id: id.to_string(),
                name: name.to_string(),
                ..WatchTask::default()
            },
            ..WatchSnapshot::default()
        }
    }

    #[test]
    fn resolve_task_query_accepts_unique_prefix_and_name() {
        let tasks = vec![
            snapshot("abcdef123456", "Meeting"),
            snapshot("123456abcdef", "Notes"),
        ];
        assert_eq!(
            resolve_task_query(&tasks, "abc").unwrap().task.name,
            "Meeting"
        );
        assert_eq!(
            resolve_task_query(&tasks, "Notes").unwrap().task.id,
            "123456abcdef"
        );
    }

    #[test]
    fn resolve_task_query_rejects_ambiguous_name_or_prefix() {
        let tasks = vec![
            snapshot("abcdef123456", "Meeting"),
            snapshot("abc999123456", "Meeting"),
        ];
        assert!(resolve_task_query(&tasks, "abc").is_err());
        assert!(resolve_task_query(&tasks, "Meeting").is_err());
    }

    #[test]
    fn short_path_preserves_file_name() {
        let value = short_path("/very/long/path/to/recordings/2026-05-24/example-file.m4a");
        assert!(value.ends_with("example-file.m4a"));
        assert!(value.len() <= 48);
    }

    #[test]
    fn open_path_command_uses_platform_default_opener() {
        let command = open_path_command(Path::new("/tmp/bifrost-asr-open-test.md"), false).unwrap();
        #[cfg(target_os = "macos")]
        assert_eq!(command.get_program(), "open");
        #[cfg(target_os = "windows")]
        assert_eq!(command.get_program(), "explorer");
        #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
        assert_eq!(command.get_program(), "xdg-open");
    }

    #[test]
    fn pause_control_path_supports_resume_temporary_and_force_pause() {
        assert_eq!(pause_control_path(true, false), "resume");
        assert_eq!(pause_control_path(false, false), "pause?mode=temporary");
        assert_eq!(
            pause_control_path(false, true),
            "pause?mode=long_term&force=true"
        );
    }

    #[test]
    fn humanize_api_error_prefers_response_message() {
        let message = humanize_api_error(
            "Config error: POST http://127.0.0.1/_bifrost/api/asr/tasks/task/run failed with HTTP 409: {\"message\":\"ASR task is already running\",\"running\":true}",
        );
        assert_eq!(message, "ASR task is already running");
    }
}
