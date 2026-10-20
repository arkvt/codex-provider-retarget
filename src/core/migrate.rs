use std::collections::HashSet;
use std::path::Path;

use super::types::{
    MigrateError, MigrateOptions, MigrationOutcome, Progress, RunningProcess, Stage,
    resolve_source_providers,
};
use super::{backup, database, paths, processes, scan, sessions};

/// Number of session files between progress updates.
const PROGRESS_EVERY: usize = 20;

pub fn migrate(
    options: &MigrateOptions,
    on_progress: &dyn Fn(Progress),
) -> Result<MigrationOutcome, MigrateError> {
    migrate_with(
        options,
        on_progress,
        &processes::find_running_codex_processes,
    )
}

/// Same as [`migrate`], but with the process probe injected so the blocking
/// path can be tested without actually starting a Codex process.
pub fn migrate_with(
    options: &MigrateOptions,
    on_progress: &dyn Fn(Progress),
    find_running_codex: &dyn Fn() -> Vec<RunningProcess>,
) -> Result<MigrationOutcome, MigrateError> {
    let target_provider = options.target_provider.trim().to_string();
    if target_provider.is_empty() {
        return Err(MigrateError::Failed(
            "目标 Provider 不能为空。".to_string(),
        ));
    }

    // Data safety gate: Codex holds the database and session files open.
    let running = find_running_codex();
    if !running.is_empty() {
        return Err(MigrateError::CodexRunning(running));
    }

    let codex_home = paths::normalize_codex_home(&options.codex_home);
    on_progress(Progress::new(
        Stage::Scan,
        "扫描 Codex 历史",
        Some(paths::display_path(&codex_home)),
    ));

    let before = scan::scan_codex_history(&options.codex_home);
    if !before.database_exists {
        return Err(MigrateError::Failed(format!(
            "未找到 Codex 状态数据库：{}",
            before.database_path.display()
        )));
    }

    // A session file we cannot read is almost always one that a Codex host still
    // has open. Rewriting it would race with that writer and could truncate data
    // appended between our read and our write, so refuse before touching
    // anything.
    if !before.unreadable_sessions.is_empty() {
        return Err(MigrateError::Failed(format!(
            "{} 个 session 文件无法读取（位于 {}）；请先关闭占用它们的程序后重试。",
            before.unreadable_sessions.len(),
            before.sessions_path.display()
        )));
    }

    let source_providers =
        resolve_source_providers(&options.source_providers, &before, &target_provider);
    if source_providers.is_empty() {
        return Err(MigrateError::Failed(
            "没有选中任何要迁移的源 Provider。".to_string(),
        ));
    }

    on_progress(Progress::new(
        Stage::Backup,
        "创建完整备份",
        Some(paths::display_path(&before.database_path)),
    ));
    let backup_dir = backup::create_backup_dir(&before.codex_home).map_err(MigrateError::Failed)?;
    backup::backup_database(&before.database_path, &backup_dir).map_err(MigrateError::Failed)?;
    backup::backup_sessions(&before.sessions_path, &backup_dir).map_err(MigrateError::Failed)?;

    on_progress(Progress::new(
        Stage::Database,
        "更新 state_5.sqlite",
        Some(target_provider.clone()),
    ));
    let updated_thread_rows =
        database::update_providers(&before.database_path, &target_provider, &source_providers)
            .map_err(MigrateError::Failed)?;

    let (updated_session_files, session_errors) = update_session_files(
        &before.sessions_path,
        &target_provider,
        &source_providers,
        on_progress,
    );

    on_progress(Progress::new(
        Stage::Verify,
        "重新扫描校验",
        Some(paths::display_path(&backup_dir)),
    ));
    let after = scan::scan_codex_history(&options.codex_home);

    on_progress(Progress::new(
        Stage::Done,
        "迁移完成",
        Some(format!(
            "已更新 {updated_thread_rows} 条线程记录、{updated_session_files} 个 session 文件"
        )),
    ));

    Ok(MigrationOutcome {
        target_provider,
        source_providers,
        backup_dir,
        updated_thread_rows,
        updated_session_files,
        session_errors,
        before,
        after,
    })
}

/// Rewrite the selected sessions. Files that fail are collected instead of
/// aborting the run, so a single unreadable file cannot strand the migration
/// half-way through.
fn update_session_files(
    sessions_path: &Path,
    target_provider: &str,
    source_providers: &[String],
    on_progress: &dyn Fn(Progress),
) -> (usize, Vec<String>) {
    let files = sessions::walk_jsonl_files(sessions_path);
    let selected: HashSet<&str> = source_providers.iter().map(String::as_str).collect();
    let mut updated = 0usize;
    let mut errors = Vec::new();

    on_progress(Progress::new(
        Stage::Sessions,
        "更新 Session Metadata",
        Some(format!("共 {} 个候选文件", files.len())),
    ));

    for (index, file) in files.iter().enumerate() {
        let current = sessions::read_session_meta_provider(file);
        if let Some(provider) = current
            && provider != target_provider
            && selected.contains(provider.as_str())
        {
            match sessions::update_session_provider(file, target_provider) {
                Ok(update) if update.changed => updated += 1,
                Ok(_) => {}
                Err(error) => errors.push(error),
            }
        }

        if index % PROGRESS_EVERY == 0 || index + 1 == files.len() {
            on_progress(Progress::new(
                Stage::Sessions,
                "更新 Session Metadata",
                Some(format!("已重写 {} 个文件", updated)),
            ));
        }
    }

    (updated, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ScanResult;
    use rusqlite::Connection;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        _dir: tempfile::TempDir,
        home: PathBuf,
    }

    fn session_body(provider: &str) -> String {
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"model_provider\":\"{provider}\",\"id\":\"x\"}}}}\n{{\"type\":\"event\",\"payload\":{{}}}}\n"
        )
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().to_path_buf();
        fs::write(home.join("config.toml"), "model_provider = \"ccswitch\"\n").expect("config");

        let connection = Connection::open(home.join("state_5.sqlite")).expect("open");
        connection
            .execute_batch(
                "CREATE TABLE threads (id INTEGER PRIMARY KEY, model_provider TEXT);
                 INSERT INTO threads (model_provider) VALUES ('openai'), ('openai'), ('codex'), ('ccswitch');",
            )
            .expect("seed");
        connection.close().expect("close");

        fs::create_dir_all(home.join("sessions/2026/01")).expect("mkdir");
        fs::write(
            home.join("sessions/2026/01/openai.jsonl"),
            session_body("openai"),
        )
        .expect("session");
        fs::write(
            home.join("sessions/2026/01/codex.jsonl"),
            session_body("codex"),
        )
        .expect("session");
        fs::write(home.join("sessions/note.txt"), "ignored").expect("note");

        Fixture { _dir: dir, home }
    }

    fn no_processes() -> Vec<RunningProcess> {
        Vec::new()
    }

    fn options(home: &Path, sources: &[&str]) -> MigrateOptions {
        MigrateOptions {
            codex_home: home.to_string_lossy().to_string(),
            target_provider: "ccswitch".to_string(),
            source_providers: sources.iter().map(|value| value.to_string()).collect(),
        }
    }

    fn distribution(scan: &ScanResult) -> Vec<(String, usize)> {
        scan.database_distribution
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect()
    }

    #[test]
    fn migration_rewrites_database_sessions_and_writes_a_backup() {
        let fixture = fixture();
        let collected = RefCell::new(Vec::new());

        let outcome = migrate_with(
            &options(&fixture.home, &["openai", "codex"]),
            &|progress| collected.borrow_mut().push(progress),
            &no_processes,
        )
        .expect("migration");

        assert_eq!(outcome.target_provider, "ccswitch");
        assert_eq!(outcome.source_providers, vec!["codex", "openai"]);
        assert_eq!(outcome.updated_thread_rows, 3);
        assert_eq!(outcome.updated_session_files, 2);
        assert!(outcome.session_errors.is_empty());

        // The database now only knows the target provider.
        assert_eq!(
            distribution(&outcome.after),
            vec![("ccswitch".to_string(), 4)]
        );

        // ...and so do the session metadata records.
        assert_eq!(outcome.after.session_count, 2);
        assert_eq!(outcome.after.session_distribution.get("ccswitch"), Some(&2));

        // Untouched records must survive byte-for-byte.
        let rewritten =
            fs::read_to_string(fixture.home.join("sessions/2026/01/openai.jsonl")).expect("read");
        assert!(rewritten.contains("\"id\":\"x\""));
        assert!(rewritten.contains("{\"type\":\"event\",\"payload\":{}}"));

        // The backup holds the pre-migration database and sessions.
        assert!(outcome.backup_dir.is_dir());
        assert!(outcome.backup_dir.join("state_5.sqlite").is_file());
        let backup_session =
            fs::read_to_string(outcome.backup_dir.join("sessions/2026/01/openai.jsonl"))
                .expect("backup session");
        assert!(backup_session.contains("\"model_provider\":\"openai\""));

        // Progress was reported for every phase, ending with Done.
        let stages: Vec<Stage> = collected
            .borrow()
            .iter()
            .map(|progress| progress.stage)
            .collect();
        assert!(stages.contains(&Stage::Scan));
        assert!(stages.contains(&Stage::Backup));
        assert!(stages.contains(&Stage::Database));
        assert!(stages.contains(&Stage::Sessions));
        assert!(stages.contains(&Stage::Verify));
        assert_eq!(stages.last(), Some(&Stage::Done));
    }

    #[test]
    fn only_selected_providers_are_migrated() {
        let fixture = fixture();

        let outcome = migrate_with(
            &options(&fixture.home, &["openai"]),
            &|_| {},
            &no_processes,
        )
        .expect("migration");

        assert_eq!(outcome.updated_thread_rows, 2);
        assert_eq!(outcome.updated_session_files, 1);
        // `codex` keeps its own row; the two `openai` rows join the existing
        // `ccswitch` row.
        assert_eq!(
            distribution(&outcome.after),
            vec![("ccswitch".to_string(), 3), ("codex".to_string(), 1)]
        );
        assert_eq!(outcome.after.session_distribution.get("codex"), Some(&1));
    }

    #[test]
    fn running_codex_blocks_the_migration_and_changes_nothing() {
        let fixture = fixture();
        let running = || {
            vec![RunningProcess {
                pid: 4242,
                name: "codex.exe".to_string(),
                path: Some(r"C:\bin\codex.exe".to_string()),
            }]
        };

        let error = migrate_with(
            &options(&fixture.home, &["openai"]),
            &|_| {},
            &running,
        )
        .expect_err("must be blocked");

        match error {
            MigrateError::CodexRunning(processes) => {
                assert_eq!(processes.len(), 1);
                assert_eq!(processes[0].pid, 4242);
            }
            other => panic!("expected CodexRunning, got {other:?}"),
        }

        // Nothing was written and no backup directory was created.
        let scan = scan::scan_codex_history(&fixture.home.to_string_lossy());
        assert_eq!(scan.database_distribution.get("openai"), Some(&2));
        assert!(!fixture.home.join("backups").exists());
    }

    #[test]
    fn blank_target_provider_is_rejected() {
        let fixture = fixture();
        let mut options = options(&fixture.home, &["openai"]);
        options.target_provider = "   ".to_string();

        let error = migrate_with(&options, &|_| {}, &no_processes).expect_err("must fail");
        assert!(matches!(error, MigrateError::Failed(_)));
    }

    /// A session file that cannot be read is nearly always one a Codex host still
    /// has open; rewriting it would race with that writer.
    #[test]
    fn an_unreadable_session_file_blocks_the_migration() {
        let fixture = fixture();
        fs::write(
            fixture.home.join("sessions/2026/01/locked.jsonl"),
            [0xFFu8, 0xFE, 0x00],
        )
        .expect("write locked session");

        let error = migrate_with(&options(&fixture.home, &["openai"]), &|_| {}, &no_processes)
            .expect_err("must fail");
        let message = error.to_string();
        assert!(message.contains("无法读取"), "message: {message}");

        // Nothing was written and no backup directory was created.
        let scan = scan::scan_codex_history(&fixture.home.to_string_lossy());
        assert_eq!(scan.database_distribution.get("openai"), Some(&2));
        assert_eq!(scan.database_distribution.get("codex"), Some(&1));
        assert!(!fixture.home.join("backups").exists());
    }

    #[test]
    fn missing_database_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");

        let error = migrate_with(&options(dir.path(), &["openai"]), &|_| {}, &no_processes)
            .expect_err("must fail");
        let message = error.to_string();
        assert!(message.contains("状态数据库"), "message: {message}");
    }

    #[test]
    fn selecting_only_the_target_leaves_nothing_to_do() {
        let fixture = fixture();

        let error = migrate_with(
            &options(&fixture.home, &["ccswitch"]),
            &|_| {},
            &no_processes,
        )
        .expect_err("must fail");
        let message = error.to_string();
        assert!(message.contains("没有选中"), "message: {message}");
    }

    /// Regression guard: an empty selection must never be read as "migrate
    /// everything except the target".
    #[test]
    fn an_empty_selection_is_rejected_without_touching_anything() {
        let fixture = fixture();

        let error = migrate_with(&options(&fixture.home, &[]), &|_| {}, &no_processes)
            .expect_err("must fail");
        assert!(error.to_string().contains("没有选中"));

        let scan = scan::scan_codex_history(&fixture.home.to_string_lossy());
        assert_eq!(scan.database_distribution.get("openai"), Some(&2));
        assert_eq!(scan.database_distribution.get("codex"), Some(&1));
        assert!(!fixture.home.join("backups").exists());
    }
}
