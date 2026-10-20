use std::collections::BTreeMap;
use std::path::PathBuf;

/// Provider name -> number of history entries using it.
pub type ProviderDistribution = BTreeMap<String, usize>;

/// A Codex process that must be closed before history may be rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
}

/// Everything discovered about one Codex home directory.
#[derive(Debug, Clone)]
pub struct ScanResult {
    pub codex_home: PathBuf,
    pub database_path: PathBuf,
    pub sessions_path: PathBuf,
    pub config_provider: Option<String>,
    pub database_exists: bool,
    pub sessions_exists: bool,
    pub database_distribution: ProviderDistribution,
    pub session_distribution: ProviderDistribution,
    pub thread_count: usize,
    pub session_count: usize,
    /// Session files that exist but could not be read, which almost always means
    /// a Codex host still has them open.
    pub unreadable_sessions: Vec<String>,
    pub errors: Vec<String>,
}

impl ScanResult {
    /// Every distinct provider name seen in either data source, sorted
    /// case-insensitively.
    pub fn all_providers(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .database_distribution
            .keys()
            .chain(self.session_distribution.keys())
            .cloned()
            .collect();
        names.sort_by(|a, b| {
            a.to_ascii_lowercase()
                .cmp(&b.to_ascii_lowercase())
                .then_with(|| a.cmp(b))
        });
        names.dedup();
        names
    }
}

/// The four user-visible phases of a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Scan,
    Backup,
    Database,
    Sessions,
    Verify,
    Done,
}

impl Stage {
    pub fn step(self) -> usize {
        match self {
            Stage::Scan => 1,
            Stage::Backup => 2,
            Stage::Database | Stage::Sessions => 3,
            Stage::Verify | Stage::Done => 4,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Stage::Scan => "扫描配置",
            Stage::Backup => "完整备份",
            Stage::Database => "更新数据库",
            Stage::Sessions => "更新 Session Metadata",
            Stage::Verify => "校验结果",
            Stage::Done => "迁移完成",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub stage: Stage,
    pub label: String,
    pub detail: Option<String>,
}

impl Progress {
    pub fn new(stage: Stage, label: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            stage,
            label: label.into(),
            detail,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MigrateOptions {
    pub codex_home: String,
    pub target_provider: String,
    pub source_providers: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MigrationOutcome {
    pub target_provider: String,
    pub source_providers: Vec<String>,
    pub backup_dir: PathBuf,
    pub updated_thread_rows: u64,
    pub updated_session_files: usize,
    /// Session files that could not be rewritten. The migration continues past
    /// them instead of aborting half-way; the backup stays valid either way.
    pub session_errors: Vec<String>,
    pub before: ScanResult,
    pub after: ScanResult,
}

#[derive(Debug)]
pub enum MigrateError {
    /// Migration could not start or failed outright; nothing is half-done that
    /// the backup cannot explain.
    Failed(String),
    /// Codex is running and holds the database/session files open.
    CodexRunning(Vec<RunningProcess>),
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrateError::Failed(message) => write!(f, "{message}"),
            MigrateError::CodexRunning(_) => write!(
                f,
                "迁移前请先关闭所有 Codex CLI 与 Codex App 进程。"
            ),
        }
    }
}

impl std::error::Error for MigrateError {}

/// Resolve the caller's selection against what the scan actually found.
///
/// Names that are blank, equal to the target, or absent from the scanned data
/// are dropped. An empty selection therefore resolves to nothing at all: a
/// destructive rewrite must never widen beyond what was explicitly picked, so
/// the "everything except the target" default lives in the UI, where the user
/// can see it, rather than here.
pub fn resolve_source_providers(
    requested: &[String],
    scan: &ScanResult,
    target_provider: &str,
) -> Vec<String> {
    let available = scan.all_providers();

    let mut selected: Vec<String> = requested
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty() && value != target_provider && available.contains(value)
        })
        .collect();

    selected.sort_by(|a, b| {
        a.to_ascii_lowercase()
            .cmp(&b.to_ascii_lowercase())
            .then_with(|| a.cmp(b))
    });
    selected.dedup();
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_with(db: &[(&str, usize)], sessions: &[(&str, usize)]) -> ScanResult {
        ScanResult {
            codex_home: PathBuf::from("/tmp/.codex"),
            database_path: PathBuf::from("/tmp/.codex/state_5.sqlite"),
            sessions_path: PathBuf::from("/tmp/.codex/sessions"),
            config_provider: None,
            database_exists: true,
            sessions_exists: true,
            database_distribution: db.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            session_distribution: sessions.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            thread_count: db.iter().map(|(_, v)| *v).sum(),
            session_count: sessions.iter().map(|(_, v)| *v).sum(),
            unreadable_sessions: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn all_providers_dedupes_and_sorts_case_insensitively() {
        let scan = scan_with(&[("openai", 3), ("OpenAI", 1)], &[("codex", 2), ("openai", 1)]);
        assert_eq!(scan.all_providers(), vec!["codex", "OpenAI", "openai"]);
    }

    #[test]
    fn an_empty_selection_resolves_to_nothing() {
        let scan = scan_with(&[("openai", 3), ("ccswitch", 5)], &[("codex", 1)]);
        assert_eq!(
            resolve_source_providers(&[], &scan, "ccswitch"),
            Vec::<String>::new()
        );
        assert_eq!(
            resolve_source_providers(&["   ".into()], &scan, "ccswitch"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn requested_names_are_filtered_against_scanned_data() {
        let scan = scan_with(&[("openai", 3)], &[]);
        assert_eq!(
            resolve_source_providers(&[" ghost ".into(), "openai".into()], &scan, "ccswitch"),
            vec!["openai"]
        );
    }

    #[test]
    fn target_is_never_selected_as_a_source() {
        let scan = scan_with(&[("ccswitch", 1), ("openai", 1)], &[]);
        assert_eq!(
            resolve_source_providers(&["ccswitch".into()], &scan, "ccswitch"),
            Vec::<String>::new()
        );
        assert_eq!(
            resolve_source_providers(&["ccswitch".into(), "openai".into()], &scan, "ccswitch"),
            vec!["openai"]
        );
    }

    #[test]
    fn stage_steps_map_onto_four_phases() {
        assert_eq!(Stage::Scan.step(), 1);
        assert_eq!(Stage::Backup.step(), 2);
        assert_eq!(Stage::Database.step(), 3);
        assert_eq!(Stage::Sessions.step(), 3);
        assert_eq!(Stage::Verify.step(), 4);
        assert_eq!(Stage::Done.step(), 4);
    }
}
