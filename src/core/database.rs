use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params_from_iter};

use super::types::ProviderDistribution;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

fn open_for_read(database_path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("无法打开 {}：{error}", database_path.display()))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| format!("无法设置 busy timeout：{error}"))?;
    ensure_threads_table(&connection, database_path)?;
    Ok(connection)
}

fn ensure_threads_table(connection: &Connection, database_path: &Path) -> Result<(), String> {
    let found: Option<String> = connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'threads'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法检查 {}：{error}", database_path.display()))?;

    if found.is_none() {
        return Err(format!(
            "{} 里没有 threads 表，这真的是 Codex 的状态数据库吗？",
            database_path.display()
        ));
    }
    Ok(())
}

/// `threads.model_provider` counts. Rows with a NULL provider are ignored: they
/// can never match a migration source.
pub fn read_distribution(database_path: &Path) -> Result<ProviderDistribution, String> {
    let connection = open_for_read(database_path)?;
    let mut statement = connection
        .prepare(
            "SELECT model_provider, COUNT(*) FROM threads \
             WHERE model_provider IS NOT NULL \
             GROUP BY model_provider \
             ORDER BY model_provider COLLATE NOCASE",
        )
        .map_err(|error| format!("查询 {} 失败：{error}", database_path.display()))?;

    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| format!("读取 {} 失败：{error}", database_path.display()))?;

    let mut distribution = ProviderDistribution::new();
    for row in rows {
        let (provider, count) = row.map_err(|error| format!("读取数据行失败：{error}"))?;
        distribution.insert(provider, count.max(0) as usize);
    }
    Ok(distribution)
}

/// Rewrite every selected `threads.model_provider` value inside one transaction.
///
/// Returns the number of rows changed. The WAL is checkpointed back into the
/// main database afterwards; the `-wal`/`-shm` sidecars are deliberately left
/// in place, because deleting them while a checkpoint is still pending is what
/// loses data.
pub fn update_providers(
    database_path: &Path,
    target_provider: &str,
    sources: &[String],
) -> Result<u64, String> {
    if sources.is_empty() {
        return Ok(0);
    }

    let mut connection = Connection::open(database_path)
        .map_err(|error| format!("无法打开 {}：{error}", database_path.display()))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|error| format!("无法设置 busy timeout：{error}"))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|error| format!("无法启用外键约束：{error}"))?;
    ensure_threads_table(&connection, database_path)?;

    let placeholders: Vec<String> = (0..sources.len())
        .map(|index| format!("?{}", index + 2))
        .collect();
    let update_sql = format!(
        "UPDATE threads SET model_provider = ?1 WHERE model_provider IN ({})",
        placeholders.join(", ")
    );

    let transaction = connection
        .transaction()
        .map_err(|error| format!("无法开启事务：{error}"))?;
    let changed = {
        let parameters =
            std::iter::once(target_provider).chain(sources.iter().map(String::as_str));
        transaction
            .execute(&update_sql, params_from_iter(parameters))
            .map_err(|error| format!("更新 threads 失败：{error}"))?
    };
    transaction
        .commit()
        .map_err(|error| format!("提交更新失败：{error}"))?;

    // Best effort only: a busy checkpoint does not invalidate the commit.
    let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");

    Ok(changed as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_database(dir: &Path, providers: &[Option<&str>]) -> std::path::PathBuf {
        let path = dir.join("state_5.sqlite");
        let connection = Connection::open(&path).expect("open");
        connection
            .execute_batch("CREATE TABLE threads (id INTEGER PRIMARY KEY, model_provider TEXT);")
            .expect("schema");
        for (index, provider) in providers.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO threads (id, model_provider) VALUES (?1, ?2)",
                    rusqlite::params![index as i64, provider],
                )
                .expect("insert");
        }
        connection.close().expect("close");
        path
    }

    #[test]
    fn distribution_groups_counts_and_ignores_null_providers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = make_database(
            dir.path(),
            &[
                Some("openai"),
                Some("openai"),
                Some("codex"),
                None,
                Some("ccswitch"),
            ],
        );

        let distribution = read_distribution(&path).expect("distribution");
        assert_eq!(distribution.get("openai"), Some(&2));
        assert_eq!(distribution.get("codex"), Some(&1));
        assert_eq!(distribution.get("ccswitch"), Some(&1));
        assert_eq!(distribution.values().sum::<usize>(), 4);
    }

    #[test]
    fn update_only_touches_selected_providers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = make_database(
            dir.path(),
            &[Some("openai"), Some("openai"), Some("codex"), Some("ccswitch")],
        );

        let changed = update_providers(&path, "ccswitch", &["openai".to_string()]).expect("update");
        assert_eq!(changed, 2);

        let distribution = read_distribution(&path).expect("distribution");
        assert_eq!(distribution.get("ccswitch"), Some(&3));
        assert_eq!(distribution.get("codex"), Some(&1));
        assert_eq!(distribution.get("openai"), None);
    }

    #[test]
    fn update_matches_provider_names_case_sensitively() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = make_database(dir.path(), &[Some("openai"), Some("OpenAI")]);

        update_providers(&path, "ccswitch", &["openai".to_string()]).expect("update");

        let distribution = read_distribution(&path).expect("distribution");
        assert_eq!(distribution.get("ccswitch"), Some(&1));
        assert_eq!(
            distribution.get("OpenAI"),
            Some(&1),
            "OpenAI must stay untouched"
        );
    }

    #[test]
    fn update_with_no_sources_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = make_database(dir.path(), &[Some("openai")]);
        assert_eq!(update_providers(&path, "ccswitch", &[]).expect("update"), 0);
        assert_eq!(
            read_distribution(&path).expect("distribution").get("openai"),
            Some(&1)
        );
    }

    #[test]
    fn database_without_threads_table_is_rejected_clearly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state_5.sqlite");
        Connection::open(&path)
            .expect("open")
            .execute_batch("CREATE TABLE unrelated (id INTEGER);")
            .expect("schema");

        let error = read_distribution(&path).expect_err("must fail");
        assert!(error.contains("threads"), "unexpected error: {error}");
    }

    #[test]
    fn missing_database_reports_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_distribution(&dir.path().join("absent.sqlite")).is_err());
    }
}
