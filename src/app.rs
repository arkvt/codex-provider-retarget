//! UI state machine. Deliberately free of any terminal dependency: key presses
//! arrive as a plain [`Key`] enum, so the whole interaction model is testable.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::core::migrate::migrate;
use crate::core::paths;
use crate::core::scan::scan_codex_history;
use crate::core::types::{
    MigrateError, MigrateOptions, MigrationOutcome, Progress, RunningProcess, ScanResult, Stage,
};

/// Keep memory bounded when scanning huge session trees.
const MAX_LOG_LINES: usize = 200;

pub enum WorkerEvent {
    ScanFinished(Box<ScanResult>),
    Progress(Box<Progress>),
    MigrationFinished(Box<MigrationOutcome>),
    MigrationFailed(Box<MigrateError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Idle,
    Scanning,
    Migrating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditField {
    CodexHome,
    TargetProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Help,
    Confirm,
    Blocked,
    Report,
    Error,
    Edit(EditField),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Pending,
    Active,
    Done,
}

/// A plain key press, independent of the terminal backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
}

#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub name: String,
    pub database_count: usize,
    pub session_count: usize,
    pub selected: bool,
}

impl ProviderRow {
    pub fn total(&self) -> usize {
        self.database_count + self.session_count
    }
}

pub struct App {
    pub codex_home: String,
    pub target_provider: String,
    pub scan: Option<ScanResult>,
    pub rows: Vec<ProviderRow>,
    pub cursor: usize,
    pub overlay: Option<Overlay>,
    pub edit_buffer: String,
    pub busy: Busy,
    pub log: Vec<Progress>,
    pub outcome: Option<MigrationOutcome>,
    pub error: Option<String>,
    pub blocked: Vec<RunningProcess>,
    pub should_quit: bool,
    sender: Sender<WorkerEvent>,
    receiver: Receiver<WorkerEvent>,
}

impl App {
    pub fn new() -> Self {
        let mut app = Self::build(paths::display_path(&paths::default_codex_home()));
        app.start_scan();
        app
    }

    fn build(codex_home: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            codex_home,
            target_provider: String::new(),
            scan: None,
            rows: Vec::new(),
            cursor: 0,
            overlay: None,
            edit_buffer: String::new(),
            busy: Busy::Idle,
            log: Vec::new(),
            outcome: None,
            error: None,
            blocked: Vec::new(),
            should_quit: false,
            sender,
            receiver,
        }
    }

    /// An app that has not scanned yet, for tests and for rendering snapshots.
    #[cfg(test)]
    pub fn for_test(codex_home: &str) -> Self {
        Self::build(codex_home.to_string())
    }

    #[cfg(test)]
    pub fn apply_scan_for_test(&mut self, scan: ScanResult) {
        self.apply_scan(scan);
    }

    #[cfg(test)]
    pub fn apply_event_for_test(&mut self, event: WorkerEvent) {
        self.apply_event(event);
    }

    // ---------------------------------------------------------------- scanning

    pub fn start_scan(&mut self) {
        if self.busy != Busy::Idle {
            return;
        }
        self.busy = Busy::Scanning;
        self.error = None;

        let codex_home = self.codex_home.clone();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let scan = scan_codex_history(&codex_home);
            let _ = sender.send(WorkerEvent::ScanFinished(Box::new(scan)));
        });
    }

    fn apply_scan(&mut self, scan: ScanResult) {
        if self.target_provider.trim().is_empty()
            && let Some(provider) = scan.config_provider.clone()
        {
            self.target_provider = provider;
        }

        self.rows = build_rows(&scan);
        self.scan = Some(scan);
        self.sync_selection();
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    /// Everything except the target starts out selected; changing the target
    /// re-derives the selection so the target itself is never a source.
    fn sync_selection(&mut self) {
        let target = self.target_provider.trim().to_string();
        for row in &mut self.rows {
            row.selected = row.name != target;
        }
    }

    // -------------------------------------------------------------- migration

    pub fn start_migration(&mut self) {
        if self.busy != Busy::Idle {
            return;
        }

        self.overlay = None;
        self.error = None;
        self.blocked.clear();
        self.outcome = None;
        self.log.clear();
        self.busy = Busy::Migrating;

        let options = MigrateOptions {
            codex_home: self.codex_home.clone(),
            target_provider: self.target_provider.trim().to_string(),
            source_providers: self.selected_sources(),
        };

        let sender = self.sender.clone();
        thread::spawn(move || {
            let progress_sender = sender.clone();
            let result = migrate(&options, &|progress| {
                let _ = progress_sender.send(WorkerEvent::Progress(Box::new(progress)));
            });

            let event = match result {
                Ok(outcome) => WorkerEvent::MigrationFinished(Box::new(outcome)),
                Err(error) => WorkerEvent::MigrationFailed(Box::new(error)),
            };
            let _ = sender.send(event);
        });
    }

    /// Drain everything the worker threads have reported since the last frame,
    /// returning how many events were applied.
    pub fn poll(&mut self) -> usize {
        let mut applied = 0;
        while let Ok(event) = self.receiver.try_recv() {
            self.apply_event(event);
            applied += 1;
        }
        applied
    }

    fn apply_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::ScanFinished(scan) => {
                self.busy = Busy::Idle;
                self.apply_scan(*scan);
            }
            WorkerEvent::Progress(progress) => {
                if self.log.len() >= MAX_LOG_LINES {
                    self.log.remove(0);
                }
                self.log.push(*progress);
            }
            WorkerEvent::MigrationFinished(outcome) => {
                self.busy = Busy::Idle;
                self.rows = build_rows(&outcome.after);
                self.scan = Some(outcome.after.clone());
                self.sync_selection();
                self.cursor = 0;
                self.outcome = Some(*outcome);
                self.overlay = Some(Overlay::Report);
            }
            WorkerEvent::MigrationFailed(error) => {
                self.busy = Busy::Idle;
                match *error {
                    MigrateError::CodexRunning(processes) => {
                        self.blocked = processes;
                        self.overlay = Some(Overlay::Blocked);
                    }
                    MigrateError::Failed(message) => {
                        self.error = Some(message);
                        self.overlay = Some(Overlay::Error);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------- selections

    pub fn is_target(&self, row: &ProviderRow) -> bool {
        row.name == self.target_provider.trim()
    }

    pub fn selected_sources(&self) -> Vec<String> {
        let target = self.target_provider.trim().to_string();
        self.rows
            .iter()
            .filter(|row| row.selected && row.name != target)
            .map(|row| row.name.clone())
            .collect()
    }

    /// `(database rows, session files)` that the current selection would rewrite.
    pub fn pending_counts(&self) -> (usize, usize) {
        let target = self.target_provider.trim().to_string();
        self.rows
            .iter()
            .filter(|row| row.selected && row.name != target)
            .fold((0, 0), |(database, sessions), row| {
                (
                    database + row.database_count,
                    sessions + row.session_count,
                )
            })
    }

    pub fn selectable_count(&self) -> usize {
        let target = self.target_provider.trim().to_string();
        self.rows.iter().filter(|row| row.name != target).count()
    }

    pub fn toggle_current(&mut self) {
        let target = self.target_provider.trim().to_string();
        if let Some(row) = self.rows.get_mut(self.cursor)
            && row.name != target
        {
            row.selected = !row.selected;
        }
    }

    pub fn select_all(&mut self) {
        let target = self.target_provider.trim().to_string();
        for row in &mut self.rows {
            row.selected = row.name != target;
        }
    }

    pub fn select_none(&mut self) {
        for row in &mut self.rows {
            row.selected = false;
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.rows.is_empty() {
            self.cursor = 0;
            return;
        }
        let last = self.rows.len() - 1;
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, last as isize) as usize;
    }

    // ------------------------------------------------------------------ gating

    pub fn can_migrate(&self) -> bool {
        self.migration_blocker().is_none()
    }

    /// Why the migration cannot start right now, or `None` when it can.
    pub fn migration_blocker(&self) -> Option<&'static str> {
        if self.busy == Busy::Scanning {
            return Some("正在扫描 Codex 目录…");
        }
        if self.busy == Busy::Migrating {
            return Some("迁移正在进行中");
        }
        match &self.scan {
            None => Some("尚未扫描 - 按 r 开始扫描"),
            Some(scan) if !scan.database_exists => {
                Some("该 Codex 目录下未找到 state_5.sqlite")
            }
            Some(_) => {
                if self.target_provider.trim().is_empty() {
                    Some("请先设置目标 Provider（按 t）")
                } else if self.selected_sources().is_empty() {
                    Some("请至少勾选一个要迁移的源 Provider（按空格）")
                } else {
                    None
                }
            }
        }
    }

    pub fn stage_state(&self, stage: Stage) -> StageState {
        if self.outcome.is_some() {
            return StageState::Done;
        }
        let current = self
            .log
            .last()
            .map(|progress| progress.stage.step())
            .unwrap_or(0);
        if current == 0 {
            return StageState::Pending;
        }
        let step = stage.step();
        if step < current {
            StageState::Done
        } else if step == current {
            StageState::Active
        } else {
            StageState::Pending
        }
    }

    pub fn status_text(&self) -> String {
        if let Some(error) = &self.error {
            return format!("失败：{error}");
        }
        if let Some(outcome) = &self.outcome {
            if outcome.session_errors.is_empty() {
                return "迁移完成".to_string();
            }
            return format!(
                "迁移完成，但有 {} 个文件出错",
                outcome.session_errors.len()
            );
        }
        match self.busy {
            Busy::Scanning => "正在扫描…".to_string(),
            Busy::Migrating => "正在迁移…".to_string(),
            Busy::Idle => match self.log.last() {
                Some(progress) => progress.label.clone(),
                None => "就绪".to_string(),
            },
        }
    }

    // -------------------------------------------------------------- key routing

    pub fn begin_edit(&mut self, field: EditField) {
        if self.busy != Busy::Idle {
            return;
        }
        self.edit_buffer = match field {
            EditField::CodexHome => self.codex_home.clone(),
            EditField::TargetProvider => self.target_provider.clone(),
        };
        self.overlay = Some(Overlay::Edit(field));
    }

    pub fn handle_key(&mut self, key: Key) {
        if let Some(Overlay::Edit(field)) = self.overlay {
            self.handle_edit_key(field, key);
            return;
        }

        match self.overlay {
            Some(Overlay::Help) => {
                if matches!(key, Key::Esc | Key::Enter | Key::Char('q') | Key::Char('?')) {
                    self.overlay = None;
                }
                return;
            }
            Some(Overlay::Confirm) => {
                match key {
                    Key::Char('y') | Key::Char('Y') | Key::Enter => self.start_migration(),
                    Key::Esc | Key::Char('n') | Key::Char('N') => self.overlay = None,
                    _ => {}
                }
                return;
            }
            Some(Overlay::Blocked) => {
                match key {
                    // Re-running the migration re-checks the process list.
                    Key::Char('r') | Key::Char('R') | Key::Enter => self.start_migration(),
                    Key::Esc | Key::Char('q') => self.overlay = None,
                    _ => {}
                }
                return;
            }
            Some(Overlay::Report | Overlay::Error) => {
                if matches!(key, Key::Esc | Key::Enter | Key::Char('q')) {
                    self.overlay = None;
                }
                return;
            }
            Some(Overlay::Edit(_)) => return,
            None => {}
        }

        match key {
            Key::Char('q') | Key::Esc => self.should_quit = true,
            Key::Char('?') | Key::Char('h') => self.overlay = Some(Overlay::Help),
            Key::Char('r') => self.start_scan(),
            Key::Char('p') => self.begin_edit(EditField::CodexHome),
            Key::Char('t') => self.begin_edit(EditField::TargetProvider),
            Key::Char(' ') => self.toggle_current(),
            Key::Char('a') => self.select_all(),
            Key::Char('n') => self.select_none(),
            Key::Char('m') | Key::Enter => {
                if self.can_migrate() {
                    self.overlay = Some(Overlay::Confirm);
                }
            }
            Key::Up | Key::Char('k') => self.move_cursor(-1),
            Key::Down | Key::Char('j') => self.move_cursor(1),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.rows.len().saturating_sub(1),
            Key::PageUp => self.move_cursor(-10),
            Key::PageDown => self.move_cursor(10),
            Key::Char(_)
            | Key::Backspace
            | Key::Delete
            | Key::Left
            | Key::Right
            | Key::Tab => {}
        }
    }

    fn handle_edit_key(&mut self, field: EditField, key: Key) {
        match key {
            Key::Esc => self.overlay = None,
            Key::Backspace => {
                self.edit_buffer.pop();
            }
            Key::Char(character) => self.edit_buffer.push(character),
            Key::Enter => {
                match field {
                    EditField::CodexHome => {
                        let value = self.edit_buffer.trim().to_string();
                        self.codex_home = if value.is_empty() {
                            paths::display_path(&paths::default_codex_home())
                        } else {
                            value
                        };
                        self.overlay = None;
                        self.start_scan();
                    }
                    EditField::TargetProvider => {
                        self.target_provider = self.edit_buffer.trim().to_string();
                        self.overlay = None;
                        self.sync_selection();
                    }
                }
            }
            Key::Delete
            | Key::Up
            | Key::Down
            | Key::Left
            | Key::Right
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::Tab => {}
        }
    }
}

fn build_rows(scan: &ScanResult) -> Vec<ProviderRow> {
    scan.all_providers()
        .into_iter()
        .map(|name| ProviderRow {
            database_count: scan.database_distribution.get(&name).copied().unwrap_or(0),
            session_count: scan.session_distribution.get(&name).copied().unwrap_or(0),
            selected: false,
            name,
        })
        .collect()
}

/// A synthetic scan used by the unit tests and the render snapshots.
#[cfg(test)]
pub fn fixture_scan(
    codex_home: &str,
    database: &[(&str, usize)],
    sessions: &[(&str, usize)],
    config_provider: Option<&str>,
) -> ScanResult {
    use crate::core::types::ProviderDistribution;
    use std::path::PathBuf;

    let database_distribution: ProviderDistribution = database
        .iter()
        .map(|(name, count)| (name.to_string(), *count))
        .collect();
    let session_distribution: ProviderDistribution = sessions
        .iter()
        .map(|(name, count)| (name.to_string(), *count))
        .collect();

    ScanResult {
        codex_home: PathBuf::from(codex_home),
        database_path: PathBuf::from(codex_home).join("state_5.sqlite"),
        sessions_path: PathBuf::from(codex_home).join("sessions"),
        config_provider: config_provider.map(str::to_string),
        database_exists: true,
        sessions_exists: true,
        thread_count: database_distribution.values().sum(),
        session_count: session_distribution.values().sum(),
        database_distribution,
        session_distribution,
        unreadable_sessions: Vec::new(),
        errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_scan() -> App {
        let mut app = App::for_test(r"C:\Users\me\.codex");
        app.apply_scan(fixture_scan(
            r"C:\Users\me\.codex",
            &[("openai", 1200), ("ccswitch", 5)],
            &[("openai", 400), ("codex", 36)],
            Some("ccswitch"),
        ));
        app
    }

    #[test]
    fn a_scan_adopts_the_configured_provider_as_target() {
        let app = app_with_scan();
        assert_eq!(app.target_provider, "ccswitch");
    }

    #[test]
    fn everything_except_the_target_starts_selected() {
        let app = app_with_scan();
        assert_eq!(app.rows.len(), 3);
        assert_eq!(app.selected_sources(), vec!["codex", "openai"]);
        assert!(!app.rows.iter().find(|r| r.name == "ccswitch").unwrap().selected);
    }

    #[test]
    fn pending_counts_only_include_selected_rows() {
        let app = app_with_scan();
        assert_eq!(app.pending_counts(), (1200, 436));

        let mut app = app_with_scan();
        app.select_none();
        assert_eq!(app.pending_counts(), (0, 0));

        // Select just `codex`, which only exists in sessions.
        let mut app = app_with_scan();
        app.select_none();
        app.cursor = app.rows.iter().position(|r| r.name == "codex").unwrap();
        app.toggle_current();
        assert_eq!(app.pending_counts(), (0, 36));
    }

    #[test]
    fn the_target_row_cannot_be_toggled() {
        let mut app = app_with_scan();
        app.select_none();
        app.cursor = app.rows.iter().position(|r| r.name == "ccswitch").unwrap();
        app.toggle_current();
        assert_eq!(app.selected_sources(), Vec::<String>::new());
    }

    #[test]
    fn changing_the_target_re_derives_the_selection() {
        let mut app = app_with_scan();
        // Rows sort as: ccswitch (target), codex, openai.
        app.cursor = app.rows.iter().position(|row| row.name == "codex").unwrap();
        app.toggle_current();
        assert_eq!(app.selected_sources(), vec!["openai"]);

        app.target_provider = "openai".to_string();
        app.sync_selection();
        assert_eq!(app.selected_sources(), vec!["ccswitch", "codex"]);
    }

    #[test]
    fn migration_is_gated_on_scan_target_and_selection() {
        let mut app = App::for_test("x");
        assert_eq!(app.migration_blocker(), Some("尚未扫描 - 按 r 开始扫描"));

        app.apply_scan(fixture_scan("x", &[("openai", 1)], &[], Some("openai")));
        // Target was adopted from the config, but nothing is selectable now.
        assert_eq!(
            app.migration_blocker(),
            Some("请至少勾选一个要迁移的源 Provider（按空格）")
        );

        app.target_provider = "ccswitch".to_string();
        app.sync_selection();
        assert!(app.can_migrate());
    }

    #[test]
    fn a_missing_database_blocks_migration() {
        let mut app = App::for_test("x");
        let mut scan = fixture_scan("x", &[("openai", 1)], &[], Some("ccswitch"));
        scan.database_exists = false;
        app.apply_scan(scan);
        assert_eq!(
            app.migration_blocker(),
            Some("该 Codex 目录下未找到 state_5.sqlite")
        );
    }

    #[test]
    fn cursor_movement_is_clamped() {
        let mut app = app_with_scan();
        app.move_cursor(-5);
        assert_eq!(app.cursor, 0);
        app.move_cursor(100);
        assert_eq!(app.cursor, app.rows.len() - 1);
        app.handle_key(Key::Home);
        assert_eq!(app.cursor, 0);
        app.handle_key(Key::End);
        assert_eq!(app.cursor, app.rows.len() - 1);
    }

    #[test]
    fn q_quits_and_question_mark_opens_help() {
        let mut app = app_with_scan();
        app.handle_key(Key::Char('?'));
        assert_eq!(app.overlay, Some(Overlay::Help));
        app.handle_key(Key::Esc);
        assert_eq!(app.overlay, None);
        assert!(!app.should_quit);

        app.handle_key(Key::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn m_opens_the_confirmation_and_escape_cancels_it() {
        let mut app = app_with_scan();
        app.handle_key(Key::Char('m'));
        assert_eq!(app.overlay, Some(Overlay::Confirm));
        app.handle_key(Key::Esc);
        assert_eq!(app.overlay, None);
        assert_eq!(app.busy, Busy::Idle);
    }

    #[test]
    fn m_does_nothing_while_migration_is_blocked() {
        let mut app = app_with_scan();
        app.select_none();
        app.handle_key(Key::Char('m'));
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn keys_are_ignored_while_an_overlay_is_open() {
        let mut app = app_with_scan();
        app.overlay = Some(Overlay::Help);
        app.handle_key(Key::Char('a'));
        app.handle_key(Key::Char(' '));
        // The help overlay swallows everything except its dismiss keys.
        assert_eq!(app.selected_sources(), vec!["codex", "openai"]);
    }

    #[test]
    fn editing_the_target_provider_re_derives_the_selection() {
        let mut app = app_with_scan();
        app.begin_edit(EditField::TargetProvider);
        assert_eq!(app.overlay, Some(Overlay::Edit(EditField::TargetProvider)));
        assert_eq!(app.edit_buffer, "ccswitch");

        app.edit_buffer.clear();
        for character in "openai".chars() {
            app.handle_key(Key::Char(character));
        }
        app.handle_key(Key::Enter);

        assert_eq!(app.target_provider, "openai");
        assert_eq!(app.overlay, None);
        assert_eq!(app.selected_sources(), vec!["ccswitch", "codex"]);
    }

    #[test]
    fn editing_backspace_and_cancel_behave() {
        let mut app = app_with_scan();
        app.begin_edit(EditField::CodexHome);
        app.handle_key(Key::Backspace);
        assert_eq!(app.edit_buffer, r"C:\Users\me\.codex"[..app.edit_buffer.len()].to_string());
        app.handle_key(Key::Esc);
        assert_eq!(app.overlay, None);
        assert_eq!(app.codex_home, r"C:\Users\me\.codex");
    }

    #[test]
    fn stage_progress_follows_the_log() {
        let mut app = app_with_scan();
        assert_eq!(app.stage_state(Stage::Scan), StageState::Pending);

        app.log.push(Progress::new(Stage::Backup, "backup", None));
        assert_eq!(app.stage_state(Stage::Scan), StageState::Done);
        assert_eq!(app.stage_state(Stage::Backup), StageState::Active);
        assert_eq!(app.stage_state(Stage::Verify), StageState::Pending);

        app.outcome = Some(MigrationOutcome {
            target_provider: "ccswitch".to_string(),
            source_providers: vec!["openai".to_string()],
            backup_dir: std::path::PathBuf::from("backup"),
            updated_thread_rows: 1,
            updated_session_files: 1,
            session_errors: Vec::new(),
            before: fixture_scan("x", &[], &[], None),
            after: fixture_scan("x", &[], &[], None),
        });
        assert_eq!(app.stage_state(Stage::Scan), StageState::Done);
        assert_eq!(app.stage_state(Stage::Verify), StageState::Done);
    }

    #[test]
    fn the_log_is_capped() {
        let mut app = app_with_scan();
        for index in 0..(MAX_LOG_LINES + 50) {
            app.apply_event(WorkerEvent::Progress(Box::new(Progress::new(
                Stage::Sessions,
                format!("line {index}"),
                None,
            ))));
        }
        assert_eq!(app.log.len(), MAX_LOG_LINES);
        assert_eq!(
            app.log.last().map(|progress| progress.label.as_str()),
            Some(format!("line {}", MAX_LOG_LINES + 49).as_str())
        );
    }

    #[test]
    fn a_running_codex_produces_the_blocked_overlay() {
        let mut app = app_with_scan();
        app.apply_event(WorkerEvent::MigrationFailed(Box::new(
            MigrateError::CodexRunning(vec![RunningProcess {
                pid: 7,
                name: "codex.exe".to_string(),
                path: None,
            }]),
        )));

        assert_eq!(app.overlay, Some(Overlay::Blocked));
        assert_eq!(app.busy, Busy::Idle);
        assert_eq!(app.blocked.len(), 1);
    }

    #[test]
    fn a_hard_failure_produces_the_error_overlay() {
        let mut app = app_with_scan();
        app.apply_event(WorkerEvent::MigrationFailed(Box::new(MigrateError::Failed(
            "boom".to_string(),
        ))));

        assert_eq!(app.overlay, Some(Overlay::Error));
        assert_eq!(app.error.as_deref(), Some("boom"));
        assert!(app.status_text().contains("boom"));
    }

    #[test]
    fn a_successful_migration_switches_the_view_to_the_report() {
        let mut app = app_with_scan();
        app.busy = Busy::Migrating;
        app.apply_event(WorkerEvent::MigrationFinished(Box::new(MigrationOutcome {
            target_provider: "ccswitch".to_string(),
            source_providers: vec!["openai".to_string()],
            backup_dir: std::path::PathBuf::from(r"C:\Users\me\.codex\backups\provider-migration-20260101-000000"),
            updated_thread_rows: 1200,
            updated_session_files: 400,
            session_errors: Vec::new(),
            before: fixture_scan("x", &[("openai", 1200)], &[], None),
            after: fixture_scan("x", &[("ccswitch", 1205)], &[], None),
        })));

        assert_eq!(app.overlay, Some(Overlay::Report));
        assert_eq!(app.busy, Busy::Idle);
        assert_eq!(app.status_text(), "迁移完成");
        // The view now reflects the post-migration data.
        assert_eq!(app.rows.len(), 1);
        assert_eq!(app.rows[0].name, "ccswitch");
        assert!(!app.rows[0].selected);
    }
}
