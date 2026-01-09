use std::io;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::time::{Duration, Instant};
use anyhow::Result;
use leaseq_core::{config, fs as lfs, models};
use tui_textarea::TextArea;
use crate::commands::{add, lease};

use crate::tui::ui;

pub struct App<'a> {
    pub lease_id: String,
    pub nodes: Vec<NodeState>,
    pub tasks: Vec<TaskState>,
    pub all_tasks: Vec<TaskState>, // Unfiltered tasks
    pub should_quit: bool,

    // UI State
    pub focus: Focus,
    pub mode: Mode,
    pub selected_node_idx: usize,
    pub selected_task_idx: usize,
    pub textarea: TextArea<'a>, // For adding task

    // Lease Form State
    pub lease_form: LeaseFormState<'a>,

    // Logs State
    pub logs_state: LogState,

    // Node Modal State
    pub node_modal: NodeModalState,

    // Filter State
    pub filter_state: FilterState,

    // Visible log height (set by UI)
    pub log_view_height: usize,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Focus {
    Nodes,
    Tasks,
    Logs,
}

#[derive(PartialEq)]
pub enum Mode {
    Normal,
    InputAdd,
    CreateLease,
    NodeDetails,
    Help,
}

#[derive(PartialEq, Clone, Copy)]
pub enum NodeModalAction {
    ViewStatus,
    ReleaseLease,
}

pub struct NodeModalState {
    pub selected: NodeModalAction,
}

pub struct LeaseFormState<'a> {
    pub partition: TextArea<'a>,
    pub gpus: TextArea<'a>,
    pub qos: TextArea<'a>,
    pub nodes: TextArea<'a>,
    pub time: TextArea<'a>,
    pub wait: TextArea<'a>,
    pub active_field: usize, // 0..5
}

impl Default for LeaseFormState<'_> {
    fn default() -> Self {
        let mut partition = TextArea::default();
        partition.set_placeholder_text("(required)");
        let mut gpus = TextArea::default();
        gpus.set_placeholder_text("0");
        let mut qos = TextArea::default();
        qos.set_placeholder_text("(default)");
        let mut nodes = TextArea::default();
        nodes.set_placeholder_text("1");
        let mut time = TextArea::default();
        time.set_placeholder_text("(unlimited)");
        let mut wait = TextArea::default();
        wait.set_placeholder_text("30");

        Self {
            partition,
            gpus,
            qos,
            nodes,
            time,
            wait,
            active_field: 0
        }
    }
}

pub struct LogState {
    pub task_id: Option<String>,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub auto_follow: bool,
    pub file_pos: u64,
    pub show_stderr: bool,
    pub maximized: bool,
}

impl Default for LogState {
    fn default() -> Self {
        Self {
            task_id: None,
            lines: Vec::new(),
            scroll: 0,
            auto_follow: true,
            file_pos: 0,
            show_stderr: false,
            maximized: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeState {
    pub name: String,
    pub status: String,
    pub last_seen: f64,
}

#[derive(Debug, Clone)]
pub struct TaskState {
    pub id: String,
    pub command: String,
    pub state: String,
    pub node: String,
    pub exit_code: Option<i32>,
    pub gpus_requested: u32,
    pub gpus_assigned: String,
    pub finished_at: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskFilter {
    All,
    Running,
    Pending,
    Done,
    Failed,
    Recent, // Default: all active + recent completed
}

impl Default for TaskFilter {
    fn default() -> Self {
        TaskFilter::Recent
    }
}

impl std::fmt::Display for TaskFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskFilter::All => write!(f, "All"),
            TaskFilter::Running => write!(f, "Running"),
            TaskFilter::Pending => write!(f, "Pending"),
            TaskFilter::Done => write!(f, "Done"),
            TaskFilter::Failed => write!(f, "Failed"),
            TaskFilter::Recent => write!(f, "Recent"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilterState {
    pub filter: TaskFilter,
    pub recent_hours: u64, // For Recent filter: show completed within N hours
    pub max_completed: usize, // Max completed tasks to show
}

impl Default for FilterState {
    fn default() -> Self {
        Self {
            filter: TaskFilter::Recent,
            recent_hours: 24,
            max_completed: 50,
        }
    }
}

impl<'a> App<'a> {
    pub fn new(lease: Option<String>) -> Self {
        Self {
            lease_id: lease.unwrap_or_else(config::local_lease_id),
            nodes: vec![],
            tasks: vec![],
            all_tasks: vec![],
            should_quit: false,
            focus: Focus::Tasks,
            mode: Mode::Normal,
            selected_node_idx: 0,
            selected_task_idx: 0,
            textarea: TextArea::default(),
            lease_form: LeaseFormState::default(),
            logs_state: LogState::default(),
            node_modal: NodeModalState { selected: NodeModalAction::ViewStatus },
            filter_state: FilterState::default(),
            log_view_height: 10,
        }
    }

    pub fn selected_task(&self) -> Option<&TaskState> {
        self.tasks.get(self.selected_task_idx)
    }

    pub fn cycle_filter(&mut self) {
        self.filter_state.filter = match self.filter_state.filter {
            TaskFilter::Recent => TaskFilter::All,
            TaskFilter::All => TaskFilter::Running,
            TaskFilter::Running => TaskFilter::Pending,
            TaskFilter::Pending => TaskFilter::Done,
            TaskFilter::Done => TaskFilter::Failed,
            TaskFilter::Failed => TaskFilter::Recent,
        };
        self.apply_filter();
    }

    pub fn apply_filter(&mut self) {
        let now = time::OffsetDateTime::now_utc();
        let recent_cutoff = now - time::Duration::hours(self.filter_state.recent_hours as i64);

        self.tasks = match self.filter_state.filter {
            TaskFilter::All => self.all_tasks.clone(),
            TaskFilter::Running => self.all_tasks.iter()
                .filter(|t| t.state == "RUNNING")
                .cloned()
                .collect(),
            TaskFilter::Pending => self.all_tasks.iter()
                .filter(|t| t.state == "PENDING")
                .cloned()
                .collect(),
            TaskFilter::Done => self.all_tasks.iter()
                .filter(|t| t.state == "DONE")
                .cloned()
                .collect(),
            TaskFilter::Failed => self.all_tasks.iter()
                .filter(|t| t.state == "FAILED")
                .cloned()
                .collect(),
            TaskFilter::Recent => {
                // All running and pending
                let mut filtered: Vec<TaskState> = self.all_tasks.iter()
                    .filter(|t| t.state == "RUNNING" || t.state == "PENDING")
                    .cloned()
                    .collect();

                // Add recent completed (within recent_hours, up to max_completed)
                let mut completed: Vec<TaskState> = self.all_tasks.iter()
                    .filter(|t| t.state == "DONE" || t.state == "FAILED")
                    .filter(|t| {
                        t.finished_at.map(|ft| ft > recent_cutoff).unwrap_or(true)
                    })
                    .cloned()
                    .collect();

                // Sort completed by finished_at descending
                completed.sort_by(|a, b| {
                    b.finished_at.cmp(&a.finished_at)
                });

                // Take only max_completed
                completed.truncate(self.filter_state.max_completed);

                filtered.extend(completed);
                filtered
            }
        };

        // Reset selection if out of bounds
        if self.selected_task_idx >= self.tasks.len() && !self.tasks.is_empty() {
            self.selected_task_idx = 0;
        }
    }

    pub async fn run(mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let res = self.run_loop(&mut terminal).await;

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        if let Err(err) = res {
            println!("{:?}", err);
        }

        Ok(())
    }

    async fn run_loop<B: ratatui::backend::Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()> {
        let tick_rate = Duration::from_millis(250); // Fast tick for log tailing
        let mut last_tick = Instant::now();

        // Initial refresh
        self.refresh_data();
        self.refresh_logs();

        loop {
            if self.should_quit {
                break;
            }

            terminal.draw(|f| ui::draw(f, self))?;  // self is &mut App

            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));

            if event::poll(timeout)? {
                match self.mode {
                    Mode::Normal => self.handle_normal_input(event::read()?).await?,
                    Mode::InputAdd => self.handle_input_add(event::read()?).await?,
                    Mode::CreateLease => self.handle_create_lease_input(event::read()?).await?,
                    Mode::NodeDetails => self.handle_node_details_input(event::read()?).await?,
                    Mode::Help => {
                        if let Event::Key(key) = event::read()? {
                            if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                                self.mode = Mode::Normal;
                            }
                        }
                    }
                }
            }

            if last_tick.elapsed() >= tick_rate {
                self.refresh_data();
                self.refresh_logs();
                last_tick = Instant::now();
            }
        }
        Ok(())
    }

    async fn handle_normal_input(&mut self, event: Event) -> Result<()> {
        if let Event::Key(key) = event {
            // Logs always follow when not maximized
            if !self.logs_state.maximized {
                self.logs_state.auto_follow = true;
            }

            // Handle Ctrl+key combinations first (only in maximized + non-follow mode)
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Char('u') => {
                        // Half-page up in logs (only when maximized and not following)
                        if self.focus == Focus::Logs && self.logs_state.maximized && !self.logs_state.auto_follow {
                            let half_page = self.log_view_height / 2;
                            self.logs_state.scroll = self.logs_state.scroll.saturating_sub(half_page);
                        }
                        return Ok(());
                    },
                    KeyCode::Char('d') => {
                        // Half-page down in logs (only when maximized and not following)
                        if self.focus == Focus::Logs && self.logs_state.maximized && !self.logs_state.auto_follow {
                            let half_page = self.log_view_height / 2;
                            self.logs_state.scroll = self.logs_state.scroll.saturating_add(half_page);
                        }
                        return Ok(());
                    },
                    _ => {}
                }
            }

            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('?') => self.mode = Mode::Help,
                KeyCode::Tab => {
                    // Cycle: Nodes -> Tasks -> Logs -> Nodes
                    self.focus = match self.focus {
                        Focus::Nodes => Focus::Tasks,
                        Focus::Tasks => Focus::Logs,
                        Focus::Logs => Focus::Nodes,
                    };
                },
                KeyCode::Backspace => {
                    // Backspace in logs goes back to tasks
                    if self.focus == Focus::Logs {
                        self.focus = Focus::Tasks;
                    }
                },
                KeyCode::Char('h') | KeyCode::Left => {
                    // Move left in top row panes
                    match self.focus {
                        Focus::Tasks => self.focus = Focus::Nodes,
                        Focus::Logs if self.logs_state.maximized => {
                            // In maximized logs, exit maximized and go to tasks
                            self.logs_state.maximized = false;
                            self.focus = Focus::Tasks;
                        },
                        _ => {}
                    }
                },
                KeyCode::Char('l') | KeyCode::Right => {
                    // Move right in top row panes
                    if self.focus == Focus::Nodes {
                        self.focus = Focus::Tasks;
                    }
                },
                KeyCode::Char('j') | KeyCode::Down => {
                    match self.focus {
                        Focus::Nodes => {
                            // Navigate node list
                            if !self.nodes.is_empty() {
                                self.selected_node_idx = (self.selected_node_idx + 1).min(self.nodes.len() - 1);
                            }
                        },
                        Focus::Tasks => {
                            // Navigate task list
                            if !self.tasks.is_empty() {
                                self.selected_task_idx = (self.selected_task_idx + 1).min(self.tasks.len() - 1);
                            }
                        },
                        Focus::Logs => {
                            // Scroll logs only when maximized and not following
                            if self.logs_state.maximized && !self.logs_state.auto_follow {
                                self.logs_state.scroll = self.logs_state.scroll.saturating_add(1);
                            }
                        }
                    }
                },
                KeyCode::Char('k') | KeyCode::Up => {
                    match self.focus {
                        Focus::Nodes => {
                            if self.selected_node_idx > 0 {
                                self.selected_node_idx -= 1;
                            }
                        },
                        Focus::Tasks => {
                            if self.selected_task_idx > 0 {
                                self.selected_task_idx -= 1;
                            }
                        },
                        Focus::Logs => {
                            // Scroll logs only when maximized and not following
                            if self.logs_state.maximized && !self.logs_state.auto_follow {
                                self.logs_state.scroll = self.logs_state.scroll.saturating_sub(1);
                            }
                        }
                    }
                },
                KeyCode::Char('a') => {
                    self.mode = Mode::InputAdd;
                    self.textarea = TextArea::default();
                    self.textarea.set_placeholder_text("Enter command...");
                },
                KeyCode::Char('n') => {
                    self.mode = Mode::CreateLease;
                    self.lease_form = LeaseFormState::default();
                },
                KeyCode::Char('f') => {
                    // Toggle auto-follow for logs (only when maximized)
                    if self.logs_state.maximized {
                        self.logs_state.auto_follow = !self.logs_state.auto_follow;
                    }
                },
                KeyCode::Char('e') => {
                    // Toggle stderr/stdout
                    self.logs_state.show_stderr = !self.logs_state.show_stderr;
                    self.logs_state.file_pos = 0;
                    self.logs_state.lines.clear();
                    self.refresh_logs();
                },
                KeyCode::Enter => {
                    match self.focus {
                        Focus::Nodes => {
                            // Open node details modal
                            if !self.nodes.is_empty() {
                                self.node_modal.selected = NodeModalAction::ViewStatus;
                                self.mode = Mode::NodeDetails;
                            }
                        },
                        Focus::Tasks => {
                            // Select task for log viewing and focus logs
                            if !self.tasks.is_empty() {
                                let task = &self.tasks[self.selected_task_idx];
                                self.logs_state.task_id = Some(task.id.clone());
                                self.logs_state.file_pos = 0;
                                self.logs_state.lines.clear();
                                self.logs_state.auto_follow = true;
                                self.refresh_logs();
                                self.focus = Focus::Logs;
                            }
                        },
                        Focus::Logs => {
                            // Toggle maximize
                            self.logs_state.maximized = !self.logs_state.maximized;
                            if !self.logs_state.maximized {
                                // When un-maximizing, always follow
                                self.logs_state.auto_follow = true;
                            }
                        }
                    }
                },
                KeyCode::Char('G') => {
                    // Jump to end of logs (enables follow) - only when maximized
                    if self.focus == Focus::Logs && self.logs_state.maximized {
                        self.logs_state.auto_follow = true;
                    }
                },
                KeyCode::Char('g') => {
                    // Jump to start of logs (disables follow) - only when maximized
                    if self.focus == Focus::Logs && self.logs_state.maximized {
                        self.logs_state.scroll = 0;
                        self.logs_state.auto_follow = false;
                    }
                },
                KeyCode::Char('z') => {
                    // Toggle maximize logs pane
                    self.logs_state.maximized = !self.logs_state.maximized;
                    if self.logs_state.maximized {
                        self.focus = Focus::Logs;
                    } else {
                        // When un-maximizing, always follow
                        self.logs_state.auto_follow = true;
                    }
                },
                KeyCode::Char('F') => {
                    // Cycle task filter
                    self.cycle_filter();
                },
                _ => {}
            }
        }
        Ok(())
    }

    async fn handle_node_details_input(&mut self, event: Event) -> Result<()> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.mode = Mode::Normal;
                },
                KeyCode::Up | KeyCode::Char('k') => {
                    self.node_modal.selected = NodeModalAction::ViewStatus;
                },
                KeyCode::Down | KeyCode::Char('j') => {
                    self.node_modal.selected = NodeModalAction::ReleaseLease;
                },
                KeyCode::Enter => {
                    match self.node_modal.selected {
                        NodeModalAction::ViewStatus => {
                            // Just close modal, status is already shown
                            self.mode = Mode::Normal;
                        },
                        NodeModalAction::ReleaseLease => {
                            // Release/cancel the lease
                            if !self.nodes.is_empty() {
                                let node = &self.nodes[self.selected_node_idx];
                                // For local lease, we'd stop the daemon
                                // For Slurm lease, we'd call scancel
                                if self.lease_id.starts_with("local:") {
                                    // Can't release local lease from TUI easily
                                    // Just close modal for now
                                } else {
                                    let _ = std::process::Command::new("scancel")
                                        .arg(&self.lease_id)
                                        .status();
                                }
                                let _ = node; // Suppress unused warning
                            }
                            self.mode = Mode::Normal;
                        },
                    }
                },
                _ => {}
            }
        }
        Ok(())
    }

    async fn handle_input_add(&mut self, event: Event) -> Result<()> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                },
                KeyCode::Enter => {
                    let cmd = self.textarea.lines().first().cloned().unwrap_or_default();
                    if !cmd.trim().is_empty() {
                        let _ = add::add_task(cmd, Some(self.lease_id.clone()), None).await;
                        self.refresh_data();
                    }
                    self.mode = Mode::Normal;
                },
                _ => {
                    self.textarea.input(key);
                }
            }
        }
        Ok(())
    }
    
    async fn handle_create_lease_input(&mut self, event: Event) -> Result<()> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Tab => {
                     self.lease_form.active_field = (self.lease_form.active_field + 1) % 6;
                },
                KeyCode::BackTab => { // Shift+Tab often mapped to BackTab
                     if self.lease_form.active_field == 0 {
                         self.lease_form.active_field = 5;
                     } else {
                         self.lease_form.active_field -= 1;
                     }
                },
                KeyCode::Enter => {
                    // Validate and Submit
                    let part_str = self.lease_form.partition.lines().first().cloned().unwrap_or_default();
                    let partition = if part_str.trim().is_empty() { None } else { Some(part_str) };
                    let gpus = self.lease_form.gpus.lines().first().cloned().unwrap_or_default().parse::<u32>().unwrap_or(0);
                    let nodes = self.lease_form.nodes.lines().first().cloned().unwrap_or_default().parse::<u32>().unwrap_or(1);
                    let time_str = self.lease_form.time.lines().first().cloned().unwrap_or_default();
                    let time = if time_str.trim().is_empty() { None } else { Some(time_str) };
                    let qos_str = self.lease_form.qos.lines().first().cloned().unwrap_or_default();
                    let qos = if qos_str.trim().is_empty() { None } else { Some(qos_str) };
                    let wait = self.lease_form.wait.lines().first().cloned().unwrap_or_default().parse::<u64>().unwrap_or(30);

                    let args = lease::CreateLeaseArgs {
                        nodes,
                        time,
                        partition,
                        qos,
                        gpus_per_node: gpus,
                        account: None,
                        sbatch_arg: vec![],
                        wait,
                    };

                    match lease::create_lease(args).await {
                        Ok(_) => {
                            // TODO: Maybe switch to the new lease?
                            // For now just stay here.
                        },
                        Err(_) => {
                            // Ideally show error popup. For now just ignore.
                        }
                    }
                    self.mode = Mode::Normal;
                },
                _ => {
                    match self.lease_form.active_field {
                        0 => { self.lease_form.partition.input(key); },
                        1 => { self.lease_form.gpus.input(key); },
                        2 => { self.lease_form.qos.input(key); },
                        3 => { self.lease_form.nodes.input(key); },
                        4 => { self.lease_form.time.input(key); },
                        5 => { self.lease_form.wait.input(key); },
                        _ => {} // Should not happen
                    }
                }
            }
        }
        Ok(())
    }
    
    fn refresh_data(&mut self) {
        let root = if self.lease_id.starts_with("local:") {
            config::runtime_dir().join(&self.lease_id)
        } else {
            config::leaseq_home_dir().join("runs").join(&self.lease_id)
        };
        
        // Nodes
        let mut new_nodes = Vec::new();
        let hb_dir = root.join("hb");
        if let Ok(files) = lfs::list_files_sorted(&hb_dir) {
            for f in files {
                if let Ok(hb) = lfs::read_json::<models::Heartbeat, _>(&f) {
                    let age = (time::OffsetDateTime::now_utc() - hb.ts).as_seconds_f64();
                    let status = if age > 60.0 { "STALE" } else { "OK" };
                    new_nodes.push(NodeState {
                        name: hb.node,
                        status: status.to_string(),
                        last_seen: age,
                    });
                }
            }
        }
        self.nodes = new_nodes;

        // Tasks
        let mut new_tasks = Vec::new();
        // Claimed
        let claimed_dir = root.join("claimed");
        if claimed_dir.exists() {
             if let Ok(entries) = std::fs::read_dir(&claimed_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let node_name = entry.file_name().to_string_lossy().into_owned();
                         if let Ok(files) = lfs::list_files_sorted(entry.path()) {
                            for f in files {
                                if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&f) {
                                    new_tasks.push(TaskState {
                                        id: spec.task_id,
                                        command: spec.command,
                                        state: "RUNNING".to_string(),
                                        node: node_name.clone(),
                                        exit_code: None,
                                        gpus_requested: spec.gpus,
                                        gpus_assigned: String::new(), // Not known until done
                                        finished_at: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        // Inbox (Pending)
        let inbox_dir = root.join("inbox");
        if inbox_dir.exists() {
             if let Ok(entries) = std::fs::read_dir(&inbox_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let node_name = entry.file_name().to_string_lossy().into_owned();
                         if let Ok(files) = lfs::list_files_sorted(entry.path()) {
                            for f in files {
                                if let Ok(spec) = lfs::read_json::<models::TaskSpec, _>(&f) {
                                    new_tasks.push(TaskState {
                                        id: spec.task_id,
                                        command: spec.command,
                                        state: "PENDING".to_string(),
                                        node: node_name.clone(),
                                        exit_code: None,
                                        gpus_requested: spec.gpus,
                                        gpus_assigned: String::new(),
                                        finished_at: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        // Done (Finished) - show all
        let done_dir = root.join("done");
        if done_dir.exists() {
             if let Ok(entries) = std::fs::read_dir(&done_dir) {
                 for entry in entries.flatten() {
                    if entry.path().is_dir() {
                         if let Ok(files) = lfs::list_files_sorted(entry.path()) {
                            for f in files {
                                if let Ok(res) = lfs::read_json::<models::TaskResult, _>(&f) {
                                    new_tasks.push(TaskState {
                                        id: res.task_id,
                                        command: res.command,
                                        state: if res.exit_code == 0 { "DONE".to_string() } else { "FAILED".to_string() },
                                        node: res.node,
                                        exit_code: Some(res.exit_code),
                                        gpus_requested: res.gpus_requested,
                                        gpus_assigned: res.gpus_assigned,
                                        finished_at: Some(res.finished_at),
                                    });
                                }
                            }
                        }
                    }
                 }
             }
        }
        
        // Sort: RUNNING first, then PENDING, then by finished_at descending for completed
        new_tasks.sort_by(|a, b| {
            let state_order = |s: &str| match s {
                "RUNNING" => 0,
                "PENDING" => 1,
                "FAILED" => 2,
                "DONE" => 3,
                _ => 4,
            };
            let ord = state_order(&a.state).cmp(&state_order(&b.state));
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
            // For same state, sort by finished_at descending (most recent first)
            b.finished_at.cmp(&a.finished_at)
        });

        self.all_tasks = new_tasks;
        self.apply_filter();
    }
    
    fn refresh_logs(&mut self) {
        use std::io::{Read, Seek, SeekFrom};

        let tid = match &self.logs_state.task_id {
            Some(t) => t.clone(),
            None => return,
        };

        let root = if self.lease_id.starts_with("local:") {
            config::runtime_dir().join(&self.lease_id)
        } else {
            config::leaseq_home_dir().join("runs").join(&self.lease_id)
        };

        let log_path = if self.logs_state.show_stderr {
            root.join("logs").join(format!("{}.err", tid))
        } else {
            root.join("logs").join(format!("{}.out", tid))
        };

        if !log_path.exists() {
            if self.logs_state.lines.is_empty() {
                self.logs_state.lines.push("(Waiting for output...)".to_string());
            }
            return;
        }

        // Incremental read from last position
        if let Ok(mut file) = std::fs::File::open(&log_path) {
            if let Ok(metadata) = file.metadata() {
                let file_len = metadata.len();

                // If file was truncated, reset
                if file_len < self.logs_state.file_pos {
                    self.logs_state.file_pos = 0;
                    self.logs_state.lines.clear();
                }

                // Read new content
                if file_len > self.logs_state.file_pos
                    && file.seek(SeekFrom::Start(self.logs_state.file_pos)).is_ok()
                {
                    let mut new_content = String::new();
                    if file.read_to_string(&mut new_content).is_ok() {
                        for line in new_content.lines() {
                            self.logs_state.lines.push(line.to_string());
                        }
                        self.logs_state.file_pos = file_len;

                        // Auto-scroll to end if following
                        if self.logs_state.auto_follow && !self.logs_state.lines.is_empty() {
                            self.logs_state.scroll = self.logs_state.lines.len().saturating_sub(1);
                        }
                    }
                }
            }
        }

        // Limit buffer size (keep last 10000 lines)
        const MAX_LINES: usize = 10000;
        if self.logs_state.lines.len() > MAX_LINES {
            let drain_count = self.logs_state.lines.len() - MAX_LINES;
            self.logs_state.lines.drain(0..drain_count);
            self.logs_state.scroll = self.logs_state.scroll.saturating_sub(drain_count);
        }
    }
}
