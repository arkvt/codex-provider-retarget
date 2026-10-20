use super::types::ProviderDistribution;
use super::{config, database, paths, sessions};

use super::types::ScanResult;

/// Inspect one Codex home directory.
///
/// This never fails: every problem is collected into `ScanResult::errors` so
/// the UI can show partial results and still let the user fix the path.
pub fn scan_codex_history(codex_home_input: &str) -> ScanResult {
    let codex_home = paths::normalize_codex_home(codex_home_input);
    let database_path = codex_home.join("state_5.sqlite");
    let sessions_path = codex_home.join("sessions");
    let mut errors = Vec::new();

    let config_provider = match config::read_config_provider(&codex_home) {
        Ok(provider) => provider,
        Err(error) => {
            errors.push(error);
            None
        }
    };

    let database_exists = database_path.is_file();
    let mut database_distribution = ProviderDistribution::new();
    if database_exists {
        match database::read_distribution(&database_path) {
            Ok(distribution) => database_distribution = distribution,
            Err(error) => errors.push(error),
        }
    }
    let thread_count = database_distribution.values().sum();

    let sessions_exists = sessions_path.is_dir();
    let session_scan = sessions::scan_session_distribution(&sessions_path);

    ScanResult {
        codex_home,
        database_path,
        sessions_path,
        config_provider,
        database_exists,
        sessions_exists,
        database_distribution,
        session_distribution: session_scan.distribution,
        thread_count,
        session_count: session_scan.count,
        unreadable_sessions: session_scan.unreadable,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn scans_a_populated_codex_home() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        fs::write(home.join("config.toml"), "model_provider = \"ccswitch\"\n").expect("config");

        let connection = Connection::open(home.join("state_5.sqlite")).expect("open");
        connection
            .execute_batch(
                "CREATE TABLE threads (id INTEGER PRIMARY KEY, model_provider TEXT);
                 INSERT INTO threads (model_provider) VALUES ('openai'), ('openai'), ('codex');",
            )
            .expect("seed");
        connection.close().expect("close");

        fs::create_dir_all(home.join("sessions/2026")).expect("mkdir");
        fs::write(
            home.join("sessions/2026/a.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n",
        )
        .expect("session");

        let scan = scan_codex_history(&home.to_string_lossy());

        assert_eq!(scan.config_provider, Some("ccswitch".to_string()));
        assert!(scan.database_exists && scan.sessions_exists);
        assert_eq!(scan.thread_count, 3);
        assert_eq!(scan.session_count, 1);
        assert_eq!(scan.database_distribution.get("openai"), Some(&2));
        assert_eq!(scan.session_distribution.get("openai"), Some(&1));
        assert!(scan.errors.is_empty(), "unexpected errors: {:?}", scan.errors);
        assert_eq!(scan.all_providers(), vec!["codex", "openai"]);
    }

    #[test]
    fn empty_home_reports_missing_pieces_without_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scan = scan_codex_history(&dir.path().to_string_lossy());

        assert!(!scan.database_exists);
        assert!(!scan.sessions_exists);
        assert_eq!(scan.config_provider, None);
        assert_eq!(scan.thread_count, 0);
        assert_eq!(scan.session_count, 0);
        // A missing config.toml is normal, not an error worth surfacing.
        assert!(scan.errors.is_empty(), "unexpected errors: {:?}", scan.errors);
    }

    #[test]
    fn a_broken_database_is_reported_but_sessions_still_scan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        fs::write(home.join("state_5.sqlite"), b"this is not a database").expect("bogus db");
        fs::create_dir_all(home.join("sessions")).expect("mkdir");
        fs::write(
            home.join("sessions/a.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n",
        )
        .expect("session");

        let scan = scan_codex_history(&home.to_string_lossy());

        assert_eq!(scan.session_count, 1);
        assert_eq!(scan.thread_count, 0);
        assert_eq!(scan.errors.len(), 1, "errors: {:?}", scan.errors);
        assert!(scan.errors[0].contains("state_5.sqlite"));
    }

    #[test]
    fn unreadable_session_files_are_surfaced_separately_from_scan_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        fs::create_dir_all(home.join("sessions")).expect("mkdir");
        fs::write(
            home.join("sessions/ok.jsonl"),
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n",
        )
        .expect("session");
        fs::write(home.join("sessions/locked.jsonl"), [0xFFu8, 0xFE, 0x00]).expect("locked");

        let scan = scan_codex_history(&home.to_string_lossy());

        assert_eq!(scan.session_count, 1);
        assert_eq!(scan.unreadable_sessions.len(), 1);
        assert!(
            scan.errors.is_empty(),
            "unreadable files are not plain scan errors: {:?}",
            scan.errors
        );
    }
}
