//! Interactive terminal dashboard for the server binary.
//!
//! This module deliberately builds on crossterm directly.  It keeps the
//! dependency surface small, renders safely escaped snapshots of shared state,
//! and leaves all command semantics in `ConsoleHandler`.
//!
//! The dashboard never paints a background color.  Cells use the terminal
//! default so opacity, acrylic, and theme backgrounds stay visible.

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use oxide::console::command::ServerCommand;
use oxide::console::handler::ConsoleHandler;
use oxide::console::output::{set_line_sink, LineSink};
use std::collections::VecDeque;
use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing_subscriber::fmt::MakeWriter;

const LOG_CAPACITY: usize = 2_000;
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const FG: Color = Color::Reset;
const ACCENT: Color = Color::Cyan;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const ERR: Color = Color::Red;

#[derive(Clone, Copy)]
struct Style {
    color: Color,
    bold: bool,
    dim: bool,
}

impl Style {
    const fn new(color: Color) -> Self {
        Self {
            color,
            bold: false,
            dim: false,
        }
    }

    const fn bold(self) -> Self {
        Self {
            color: self.color,
            bold: true,
            dim: self.dim,
        }
    }

    const fn dim(self) -> Self {
        Self {
            color: self.color,
            bold: self.bold,
            dim: true,
        }
    }
}

const S_FG: Style = Style::new(FG);
const S_DIM: Style = Style::new(FG).dim();
const S_ACCENT: Style = Style::new(ACCENT);
const S_ACCENT_BOLD: Style = Style::new(ACCENT).bold();
const S_OK: Style = Style::new(OK);
const S_OK_BOLD: Style = Style::new(OK).bold();
const S_WARN: Style = Style::new(WARN);
const S_ERR: Style = Style::new(ERR);
const S_ERR_BOLD: Style = Style::new(ERR).bold();

/// Thread-safe scrollback used both as a tracing writer and as the destination
/// for messages produced by `ConsoleHandler`.
#[derive(Clone, Default)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
    stderr_passthrough: Arc<AtomicBool>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, text: impl Into<String>) {
        let text = text.into();
        let mut emitted = Vec::new();
        {
            let mut lines = self
                .lines
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for line in text.lines() {
                let line = line.trim_end_matches('\r').to_string();
                if line.is_empty() {
                    continue;
                }
                if lines.len() == LOG_CAPACITY {
                    lines.pop_front();
                }
                lines.push_back(line.clone());
                emitted.push(line);
            }
        }
        if self.stderr_passthrough.load(Ordering::Relaxed) {
            for line in emitted {
                std::eprintln!("{line}");
            }
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn len(&self) -> usize {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    fn clear(&self) {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// Used only if terminal setup fails and `main` falls back to the classic
    /// console. Existing buffered startup messages are replayed once.
    pub fn enable_stderr(&self) {
        if self.stderr_passthrough.swap(true, Ordering::SeqCst) {
            return;
        }
        for line in self.snapshot() {
            std::eprintln!("{line}");
        }
    }
}

pub struct LogWriter {
    buffer: LogBuffer,
    pending: Vec<u8>,
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.commit();
        Ok(())
    }
}

impl LogWriter {
    fn commit(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        self.buffer.push(text);
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        self.commit();
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter {
            buffer: self.clone(),
            pending: Vec::with_capacity(256),
        }
    }
}

struct ConsoleSinkGuard;

impl ConsoleSinkGuard {
    fn install(logs: LogBuffer) -> Self {
        let sink: LineSink = Arc::new(move |line| logs.push(format!("console  {line}")));
        set_line_sink(Some(sink));
        Self
    }
}

impl Drop for ConsoleSinkGuard {
    fn drop(&mut self) {
        set_line_sink(None);
    }
}

static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

fn restore_terminal() {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, Show, ResetColor, LeaveAlternateScreen);
    let _ = stdout.flush();
}

struct TerminalSession {
    stdout: Stdout,
}

impl TerminalSession {
    fn enter(force: bool) -> io::Result<Self> {
        if !force && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the TUI needs an interactive stdin and stdout",
            ));
        }

        enable_raw_mode()?;
        TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        let mut session = Self {
            stdout: io::stdout(),
        };
        if let Err(error) = execute!(
            session.stdout,
            EnterAlternateScreen,
            Hide,
            ResetColor,
            Clear(ClearType::All)
        ) {
            restore_terminal();
            return Err(error);
        }

        // Even release builds configured with panic=abort invoke the hook.
        // Restore the terminal first, then preserve the previous report.
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            restore_terminal();
            previous_hook(panic_info);
        }));
        Ok(session)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore_terminal();
    }
}

#[derive(Default)]
struct InputLine {
    chars: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    draft: Vec<char>,
}

impl InputLine {
    fn insert(&mut self, character: char) {
        self.chars.insert(self.cursor, character);
        self.cursor += 1;
        self.history_index = None;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
        self.history_index = None;
    }

    fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
        self.history_index = None;
    }

    fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.history_index = None;
    }

    fn take(&mut self) -> String {
        let command: String = self.chars.iter().collect();
        let command = command.trim().to_string();
        if !command.is_empty() && self.history.last() != Some(&command) {
            self.history.push(command.clone());
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.clear();
        self.draft.clear();
        command
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.draft = self.chars.clone();
                self.history.len() - 1
            }
        };
        self.load_history(next);
    }

    fn history_down(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.load_history(index + 1);
        } else {
            self.chars = std::mem::take(&mut self.draft);
            self.cursor = self.chars.len();
            self.history_index = None;
        }
    }

    fn load_history(&mut self, index: usize) {
        self.chars = self.history[index].chars().collect();
        self.cursor = self.chars.len();
        self.history_index = Some(index);
    }
}

struct Dashboard {
    input: InputLine,
    started: Instant,
    sample_time: Instant,
    sample_ticks: u64,
    ups: f64,
    log_offset: usize,
    log_rows: usize,
}

impl Dashboard {
    fn new(total_ticks: u64) -> Self {
        let now = Instant::now();
        Self {
            input: InputLine::default(),
            started: now,
            sample_time: now,
            sample_ticks: total_ticks,
            ups: 0.0,
            log_offset: 0,
            log_rows: 0,
        }
    }

    fn sample_ups(&mut self, total_ticks: u64) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.sample_time);
        if elapsed >= Duration::from_millis(500) {
            self.ups = total_ticks.saturating_sub(self.sample_ticks) as f64 / elapsed.as_secs_f64();
            self.sample_ticks = total_ticks;
            self.sample_time = now;
        }
    }

    fn scroll_logs_up(&mut self, total_lines: usize) {
        if self.log_rows == 0 {
            return;
        }
        let max_offset = total_lines.saturating_sub(self.log_rows);
        self.log_offset = self.log_offset.saturating_add(8).min(max_offset);
    }

    fn clamp_log_offset(&mut self, total_lines: usize) {
        self.log_offset = self
            .log_offset
            .min(total_lines.saturating_sub(self.log_rows));
    }
}

struct Snapshot {
    server_name: String,
    hosting: bool,
    paused: bool,
    map: String,
    mode: String,
    wave: u32,
    wave_time: f32,
    enemies: u32,
    players_count: u32,
    player_limit: u32,
    players: Vec<PlayerSnapshot>,
    total_ticks: u64,
    target_tps: u32,
    tick_last_us: u64,
    drops: u64,
}

struct PlayerSnapshot {
    name: String,
    admin: bool,
}

impl Snapshot {
    fn capture(console: &ConsoleHandler) -> Self {
        Self {
            server_name: console.admin.server_name(),
            hosting: console.state.is_hosting.load(Ordering::Relaxed),
            paused: console.state.is_paused.load(Ordering::Relaxed),
            map: console.state.map_name.read().clone(),
            mode: format!("{:?}", *console.state.mode.read()),
            wave: console.state.wave.load(Ordering::Relaxed),
            wave_time: *console.state.wave_time.read(),
            enemies: console.state.enemies_count.load(Ordering::Relaxed),
            players_count: console.state.players_count.load(Ordering::Relaxed),
            player_limit: console.admin.get_player_limit(),
            players: console
                .admin
                .connected_players_list()
                .into_iter()
                .map(|player| PlayerSnapshot {
                    admin: console.admin.is_admin(&player.uuid),
                    name: player.name,
                })
                .collect(),
            total_ticks: console.tick_engine.total_ticks.load(Ordering::Relaxed),
            target_tps: console.tick_engine.target_tps,
            tick_last_us: console.state.world_tick_us.load(Ordering::Relaxed),
            drops: console.state.dropped_frames_total.load(Ordering::Relaxed),
        }
    }
}

enum UiAction {
    None,
    Submit(String),
    Shutdown,
}

/// Runs until `exit`, `quit`, Ctrl-C, or F10 requests a graceful shutdown.
pub async fn run(console: &ConsoleHandler, logs: LogBuffer, force: bool) -> io::Result<()> {
    let mut terminal = TerminalSession::enter(force)?;
    let _sink = ConsoleSinkGuard::install(logs.clone());
    logs.push("type help for commands");

    let initial_ticks = console.tick_engine.total_ticks.load(Ordering::Relaxed);
    let mut dashboard = Dashboard::new(initial_ticks);

    loop {
        let snapshot = Snapshot::capture(console);
        dashboard.sample_ups(snapshot.total_ticks);
        render(&mut terminal.stdout, &snapshot, &mut dashboard, &logs)?;

        let Some(event) = next_event(FRAME_INTERVAL).await? else {
            continue;
        };
        let action = handle_event(event, &mut dashboard, &logs);
        match action {
            UiAction::None => {}
            UiAction::Shutdown => {
                logs.push("graceful shutdown requested");
                let _ = console.handle_command(ServerCommand::Exit).await;
                break;
            }
            UiAction::Submit(command) => {
                dashboard.log_offset = 0;
                logs.push(format!("> {command}"));
                if let Some(command) = ServerCommand::parse(&command) {
                    if console.handle_command(command).await {
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn next_event(timeout: Duration) -> io::Result<Option<Event>> {
    tokio::task::spawn_blocking(move || {
        if event::poll(timeout)? {
            event::read().map(Some)
        } else {
            Ok(None)
        }
    })
    .await
    .map_err(|error| io::Error::other(format!("terminal event task failed: {error}")))?
}

fn handle_event(event: Event, dashboard: &mut Dashboard, logs: &LogBuffer) -> UiAction {
    let Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press | KeyEventKind::Repeat,
        ..
    }) = event
    else {
        return UiAction::None;
    };

    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('c') => return UiAction::Shutdown,
            KeyCode::Char('u') => dashboard.input.clear(),
            KeyCode::Char('a') => dashboard.input.cursor = 0,
            KeyCode::Char('e') => dashboard.input.cursor = dashboard.input.chars.len(),
            KeyCode::Char('l') => {
                logs.clear();
                dashboard.log_offset = 0;
            }
            _ => {}
        }
        return UiAction::None;
    }

    match code {
        KeyCode::F(10) => UiAction::Shutdown,
        KeyCode::Enter => {
            let command = dashboard.input.take();
            if command.is_empty() {
                UiAction::None
            } else {
                UiAction::Submit(command)
            }
        }
        KeyCode::Char(character) => {
            dashboard.input.insert(character);
            UiAction::None
        }
        KeyCode::Backspace => {
            dashboard.input.backspace();
            UiAction::None
        }
        KeyCode::Delete => {
            dashboard.input.delete();
            UiAction::None
        }
        KeyCode::Left => {
            dashboard.input.cursor = dashboard.input.cursor.saturating_sub(1);
            UiAction::None
        }
        KeyCode::Right => {
            dashboard.input.cursor = (dashboard.input.cursor + 1).min(dashboard.input.chars.len());
            UiAction::None
        }
        KeyCode::Home => {
            dashboard.input.cursor = 0;
            UiAction::None
        }
        KeyCode::End => {
            dashboard.input.cursor = dashboard.input.chars.len();
            UiAction::None
        }
        KeyCode::Up => {
            dashboard.input.history_up();
            UiAction::None
        }
        KeyCode::Down => {
            dashboard.input.history_down();
            UiAction::None
        }
        KeyCode::PageUp => {
            dashboard.scroll_logs_up(logs.len());
            UiAction::None
        }
        KeyCode::PageDown => {
            dashboard.log_offset = dashboard.log_offset.saturating_sub(8);
            UiAction::None
        }
        KeyCode::Esc => {
            dashboard.input.clear();
            UiAction::None
        }
        _ => UiAction::None,
    }
}

fn render(
    stdout: &mut Stdout,
    snapshot: &Snapshot,
    dashboard: &mut Dashboard,
    logs: &LogBuffer,
) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    queue!(stdout, Hide, ResetColor, Clear(ClearType::All))?;

    if width < 40 || height < 8 {
        dashboard.log_rows = 0;
        dashboard.log_offset = 0;
        render_compact(stdout, width, height, snapshot, dashboard)?;
        stdout.flush()?;
        return Ok(());
    }

    render_header(stdout, width, snapshot, dashboard)?;
    render_metrics(stdout, 1, width.saturating_sub(2), snapshot, dashboard)?;

    let input_y = height.saturating_sub(1);
    let body_y = 3;
    let body_h = input_y.saturating_sub(body_y + 1);
    let players_w = if width >= 72 && !snapshot.players.is_empty() {
        (width / 4).clamp(18, 24)
    } else {
        0
    };
    let divider = u16::from(players_w > 0);
    let logs_w = width.saturating_sub(players_w + divider);

    dashboard.log_rows = body_h as usize;
    dashboard.clamp_log_offset(logs.len());

    hairline(stdout, 0, 2, width, dashboard.log_offset)?;
    render_logs(
        stdout,
        1,
        body_y,
        logs_w.saturating_sub(1),
        body_h,
        dashboard.log_offset,
        logs,
    )?;
    if players_w > 0 {
        column_rule(stdout, logs_w, body_y, body_h)?;
        render_players(
            stdout,
            logs_w + 1,
            body_y,
            players_w.saturating_sub(1),
            body_h,
            snapshot,
        )?;
    }
    hairline(stdout, 0, input_y.saturating_sub(1), width, 0)?;
    render_input(stdout, 0, input_y, width, dashboard)?;
    stdout.flush()
}

fn render_header(
    stdout: &mut Stdout,
    width: u16,
    snapshot: &Snapshot,
    dashboard: &Dashboard,
) -> io::Result<()> {
    let (status, status_style) = runtime_status(snapshot.hosting, snapshot.paused);
    let uptime = format_duration(dashboard.started.elapsed());
    let right = format!("{status}  {uptime}");
    let right_width = display_width(&right) as u16;
    let right_x = width.saturating_sub(right_width.saturating_add(1)).max(1);

    let mut x = 1u16;
    x += paint(stdout, x, 0, 5, "oxide", S_ACCENT_BOLD)?;
    x += paint(stdout, x, 0, 2, "  ", S_FG)?;
    let name_width = right_x.saturating_sub(x + 2);
    paint(stdout, x, 0, name_width, &snapshot.server_name, S_FG)?;

    let status_width = display_width(status) as u16;
    paint(stdout, right_x, 0, status_width, status, status_style)?;
    paint(
        stdout,
        right_x + status_width,
        0,
        right_width.saturating_sub(status_width),
        &format!("  {uptime}"),
        S_DIM,
    )?;
    Ok(())
}

fn render_metrics(
    stdout: &mut Stdout,
    x: u16,
    width: u16,
    snapshot: &Snapshot,
    dashboard: &Dashboard,
) -> io::Result<()> {
    let limit = player_limit_label(snapshot.player_limit);
    let ups_style = if dashboard.ups + 1.0 >= snapshot.target_tps as f64 {
        S_OK
    } else {
        S_WARN
    };
    let wave = if snapshot.wave_time > 0.0 {
        let seconds = snapshot.wave_time / snapshot.target_tps.max(1) as f32;
        format!("wave {}  {:.0}s", snapshot.wave, seconds.max(0.0))
    } else {
        format!("wave {}", snapshot.wave)
    };
    let items = [
        (owned_or_dash(&snapshot.map), S_FG),
        (snapshot.mode.to_ascii_lowercase(), S_DIM),
        (wave, S_FG),
        (format!("{} enemies", snapshot.enemies), S_DIM),
        (format!("{}/{}", snapshot.players_count, limit), S_FG),
        (
            format!("{:.1}/{} ups", dashboard.ups, snapshot.target_tps),
            ups_style,
        ),
        (
            format!("{:.1}ms", snapshot.tick_last_us as f64 / 1000.0),
            S_DIM,
        ),
        (
            if snapshot.drops == 0 {
                String::new()
            } else {
                format!("drops {}", snapshot.drops)
            },
            S_ERR,
        ),
    ];

    let mut cursor = x;
    let end = x.saturating_add(width);
    let mut first = true;
    for (text, style) in items {
        if text.is_empty() || cursor >= end {
            continue;
        }
        if !first {
            let used = paint(
                stdout,
                cursor,
                1,
                end.saturating_sub(cursor),
                "  ·  ",
                S_DIM,
            )?;
            if used == 0 {
                break;
            }
            cursor += used;
        }
        let used = paint(stdout, cursor, 1, end.saturating_sub(cursor), &text, style)?;
        if used == 0 {
            break;
        }
        cursor += used;
        first = false;
    }
    Ok(())
}

fn owned_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    }
}

fn render_logs(
    stdout: &mut Stdout,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    offset: usize,
    logs: &LogBuffer,
) -> io::Result<()> {
    let rows = height as usize;
    if rows == 0 || width == 0 {
        return Ok(());
    }
    let lines = logs.snapshot();
    let end = lines.len().saturating_sub(offset.min(lines.len()));
    let start = end.saturating_sub(rows);
    for (row, line) in lines[start..end].iter().enumerate() {
        paint_log_line(stdout, x, y + row as u16, width, line)?;
    }
    Ok(())
}

fn paint_log_line(stdout: &mut Stdout, x: u16, y: u16, width: u16, line: &str) -> io::Result<()> {
    let line = line.strip_prefix("console  ").unwrap_or(line);
    if let Some(command) = line.strip_prefix("> ") {
        let mut cursor = x;
        cursor += paint(stdout, cursor, y, width, "› ", S_ACCENT)?;
        paint(
            stdout,
            cursor,
            y,
            width.saturating_sub(cursor - x),
            command,
            S_ACCENT,
        )?;
        return Ok(());
    }
    if let Some((prefix, level, rest)) = split_log_level(line) {
        let mut cursor = x;
        if !prefix.is_empty() {
            let prefix = compact_log_prefix(prefix);
            cursor += paint(stdout, cursor, y, width, &prefix, S_DIM)?;
            cursor += paint(
                stdout,
                cursor,
                y,
                width.saturating_sub(cursor - x),
                " ",
                S_DIM,
            )?;
        }
        cursor += paint(
            stdout,
            cursor,
            y,
            width.saturating_sub(cursor - x),
            level,
            level_style(level),
        )?;
        cursor += paint(
            stdout,
            cursor,
            y,
            width.saturating_sub(cursor - x),
            "  ",
            S_FG,
        )?;
        paint(
            stdout,
            cursor,
            y,
            width.saturating_sub(cursor - x),
            rest.trim_start(),
            S_FG,
        )?;
        return Ok(());
    }
    let style = if line.contains("ERROR") {
        S_ERR
    } else if line.contains("WARN") {
        S_WARN
    } else {
        S_FG
    };
    paint(stdout, x, y, width, line, style)?;
    Ok(())
}

fn render_players(
    stdout: &mut Stdout,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    snapshot: &Snapshot,
) -> io::Result<()> {
    if height == 0 || width == 0 {
        return Ok(());
    }
    let title = format!("players  {}", snapshot.players_count);
    paint(stdout, x + 1, y, width.saturating_sub(1), &title, S_DIM)?;
    if height == 1 {
        return Ok(());
    }
    if snapshot.players.is_empty() {
        return paint(
            stdout,
            x + 1,
            y + 1,
            width.saturating_sub(1),
            "waiting",
            S_DIM,
        )
        .map(|_| ());
    }
    let rows = height.saturating_sub(1) as usize;
    let visible = snapshot.players.len().min(rows);
    let extra = snapshot.players.len().saturating_sub(visible);
    let name_rows = if extra > 0 {
        visible.saturating_sub(1)
    } else {
        visible
    };
    for (row, player) in snapshot.players.iter().take(name_rows).enumerate() {
        let mut cursor = x + 1;
        let inner = width.saturating_sub(1);
        cursor += paint(
            stdout,
            cursor,
            y + 1 + row as u16,
            inner,
            &player.name,
            S_FG,
        )?;
        if player.admin {
            paint(
                stdout,
                cursor,
                y + 1 + row as u16,
                (x + width).saturating_sub(cursor),
                "  admin",
                S_DIM,
            )?;
        }
    }
    if extra > 0 && name_rows < rows {
        paint(
            stdout,
            x + 1,
            y + height - 1,
            width.saturating_sub(1),
            &format!("+{extra}"),
            S_DIM,
        )?;
    }
    Ok(())
}

fn render_input(
    stdout: &mut Stdout,
    x: u16,
    y: u16,
    width: u16,
    dashboard: &Dashboard,
) -> io::Result<()> {
    let prompt = "❯ ";
    let prompt_w = 2u16;
    paint(stdout, x + 1, y, prompt_w, prompt, S_ACCENT_BOLD)?;
    let available = width.saturating_sub(prompt_w + 2) as usize;
    let start = dashboard
        .input
        .cursor
        .saturating_sub(available.saturating_sub(1));
    let visible: String = dashboard
        .input
        .chars
        .iter()
        .skip(start)
        .take(available)
        .collect();
    paint(
        stdout,
        x + 1 + prompt_w,
        y,
        available as u16,
        &visible,
        S_FG,
    )?;
    let cursor = dashboard.input.cursor.saturating_sub(start).min(available) as u16;
    queue!(stdout, MoveTo(x + 1 + prompt_w + cursor, y), Show)?;
    Ok(())
}

fn render_compact(
    stdout: &mut Stdout,
    width: u16,
    height: u16,
    snapshot: &Snapshot,
    dashboard: &Dashboard,
) -> io::Result<()> {
    let (status, status_style) = runtime_status(snapshot.hosting, snapshot.paused);
    let mut x = 1u16;
    x += paint(
        stdout,
        x,
        0,
        width.saturating_sub(1),
        "oxide  ",
        S_ACCENT_BOLD,
    )?;
    x += paint(stdout, x, 0, width.saturating_sub(x), status, status_style)?;
    let rest = format!("  {}", snapshot.server_name);
    paint(stdout, x, 0, width.saturating_sub(x), &rest, S_DIM)?;

    if height > 2 {
        let limit = player_limit_label(snapshot.player_limit);
        let line = format!(
            "{}  wave {}  {}/{}  {:.0} ups",
            snapshot.map, snapshot.wave, snapshot.players_count, limit, dashboard.ups
        );
        paint(stdout, 1, 1, width.saturating_sub(2), &line, S_FG)?;
    }

    let input_y = height.saturating_sub(1);
    let visible: String = dashboard
        .input
        .chars
        .iter()
        .take(width.saturating_sub(4) as usize)
        .collect();
    paint(stdout, 1, input_y, 2, "❯ ", S_ACCENT_BOLD)?;
    paint(stdout, 3, input_y, width.saturating_sub(4), &visible, S_FG)?;
    queue!(
        stdout,
        MoveTo(
            (3 + dashboard.input.cursor as u16).min(width.saturating_sub(1)),
            input_y
        ),
        Show
    )?;
    Ok(())
}

fn runtime_status(hosting: bool, paused: bool) -> (&'static str, Style) {
    if !hosting {
        ("stopped", S_ERR_BOLD)
    } else if paused {
        ("paused", S_WARN)
    } else {
        ("hosting", S_OK_BOLD)
    }
}

fn player_limit_label(limit: u32) -> String {
    if limit == 0 {
        "∞".to_string()
    } else {
        limit.to_string()
    }
}

fn compact_log_prefix(prefix: &str) -> String {
    if let Some(time) = prefix.split('T').nth(1) {
        if time.len() >= 8 && time.as_bytes()[2] == b':' && time.as_bytes()[5] == b':' {
            return time[..8].to_string();
        }
    }
    prefix.to_string()
}

fn split_log_level(line: &str) -> Option<(&str, &str, &str)> {
    const LEVELS: [&str; 5] = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"];
    for level in LEVELS {
        let padded = format!(" {level} ");
        if let Some(index) = line.find(&padded) {
            return Some((
                line[..index].trim_end(),
                level,
                &line[index + padded.len()..],
            ));
        }
        if let Some(rest) = line.strip_prefix(level) {
            let rest = rest.strip_prefix(':').unwrap_or(rest);
            return Some(("", level, rest));
        }
    }
    None
}

fn level_style(level: &str) -> Style {
    match level {
        "ERROR" => S_ERR_BOLD,
        "WARN" => S_WARN,
        "INFO" => S_DIM,
        _ => S_DIM,
    }
}

fn hairline(stdout: &mut Stdout, x: u16, y: u16, width: u16, older: usize) -> io::Result<()> {
    if width == 0 {
        return Ok(());
    }
    let rule = "─".repeat(width as usize);
    paint(stdout, x, y, width, &rule, S_DIM)?;
    if older > 0 && width > 12 {
        let label = format!(" {older} older ");
        let label_w = display_width(&label) as u16;
        paint(
            stdout,
            x + width.saturating_sub(label_w),
            y,
            label_w,
            &label,
            S_DIM,
        )?;
    }
    Ok(())
}

fn column_rule(stdout: &mut Stdout, x: u16, y: u16, height: u16) -> io::Result<()> {
    for row in 0..height {
        paint(stdout, x, y + row, 1, "│", S_DIM)?;
    }
    Ok(())
}

fn paint(
    stdout: &mut Stdout,
    x: u16,
    y: u16,
    max_width: u16,
    value: &str,
    style: Style,
) -> io::Result<u16> {
    if max_width == 0 {
        return Ok(0);
    }
    let clipped = clip(value, max_width as usize);
    let used = display_width(&clipped) as u16;
    queue!(
        stdout,
        MoveTo(x, y),
        ResetColor,
        SetForegroundColor(style.color)
    )?;
    if style.bold {
        queue!(stdout, SetAttribute(Attribute::Bold))?;
    }
    if style.dim {
        queue!(stdout, SetAttribute(Attribute::Dim))?;
    }
    queue!(
        stdout,
        Print(&clipped),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    Ok(used)
}

fn clip(value: &str, max_width: usize) -> String {
    sanitize(value).chars().take(max_width).collect()
}

fn display_width(value: &str) -> usize {
    sanitize(value).chars().count()
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_buffer_is_bounded_and_keeps_newest_lines() {
        let logs = LogBuffer::new();
        for index in 0..LOG_CAPACITY + 3 {
            logs.push(format!("line-{index}"));
        }
        let lines = logs.snapshot();
        assert_eq!(lines.len(), LOG_CAPACITY);
        assert_eq!(lines.first().map(String::as_str), Some("line-3"));
        assert_eq!(lines.last().map(String::as_str), Some("line-2002"));
    }

    #[test]
    fn input_supports_unicode_editing_and_history() {
        let mut input = InputLine::default();
        for character in "say hola 🌎".chars() {
            input.insert(character);
        }
        input.backspace();
        input.insert('!');
        assert_eq!(input.take(), "say hola !");

        input.history_up();
        assert_eq!(input.chars.iter().collect::<String>(), "say hola !");
        input.history_down();
        assert!(input.chars.is_empty());
    }

    #[test]
    fn terminal_control_characters_are_neutralized() {
        assert_eq!(sanitize("player\u{1b}[2J\nname"), "player [2J name");
    }

    #[test]
    fn duration_format_is_stable() {
        assert_eq!(format_duration(Duration::from_secs(3_661)), "01:01:01");
    }

    #[test]
    fn log_scrolling_stops_at_the_oldest_full_page() {
        let mut dashboard = Dashboard::new(0);
        dashboard.log_rows = 10;

        dashboard.scroll_logs_up(6);
        assert_eq!(dashboard.log_offset, 0);

        dashboard.scroll_logs_up(25);
        assert_eq!(dashboard.log_offset, 8);
        dashboard.scroll_logs_up(25);
        assert_eq!(dashboard.log_offset, 15);
        dashboard.scroll_logs_up(25);
        assert_eq!(dashboard.log_offset, 15);

        dashboard.log_rows = 20;
        dashboard.clamp_log_offset(25);
        assert_eq!(dashboard.log_offset, 5);
    }

    #[test]
    fn runtime_status_prefers_paused_while_hosting() {
        assert_eq!(runtime_status(true, false).0, "hosting");
        assert_eq!(runtime_status(true, true).0, "paused");
        assert_eq!(runtime_status(false, true).0, "stopped");
    }

    #[test]
    fn tracing_levels_split_away_from_the_message() {
        let (prefix, level, rest) =
            split_log_level("2026-08-21T03:24:01.123456Z  INFO listener: bound 0.0.0.0:6567")
                .expect("level");
        assert!(prefix.contains("2026-08-21"));
        assert_eq!(level, "INFO");
        assert!(rest.contains("bound 0.0.0.0:6567"));
        assert_eq!(compact_log_prefix(prefix), "03:24:01");
    }
}
