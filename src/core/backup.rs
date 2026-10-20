use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;

/// `<codex-home>/backups/provider-migration-YYYYMMDD-HHMMSS`, with a numeric
/// suffix if two migrations land in the same second.
pub fn create_backup_dir(codex_home: &Path) -> Result<PathBuf, String> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let root = codex_home.join("backups");
    fs::create_dir_all(&root).map_err(|error| format!("无法创建 {}：{error}", root.display()))?;

    let mut candidate = root.join(format!("provider-migration-{timestamp}"));
    let mut suffix = 1;
    while candidate.exists() {
        suffix += 1;
        candidate = root.join(format!("provider-migration-{timestamp}-{suffix}"));
    }

    fs::create_dir_all(&candidate)
        .map_err(|error| format!("无法创建 {}：{error}", candidate.display()))?;
    Ok(candidate)
}

/// Copy the database plus whatever WAL sidecars currently exist.
pub fn backup_database(database_path: &Path, backup_dir: &Path) -> Result<(), String> {
    let file_name = database_path
        .file_name()
        .ok_or_else(|| format!("{} 没有文件名", database_path.display()))?;

    let target = backup_dir.join(file_name);
    fs::copy(database_path, &target)
        .map_err(|error| format!("备份 {} 失败：{error}", database_path.display()))?;

    for suffix in ["-wal", "-shm"] {
        let sidecar = with_suffix(database_path, suffix);
        if sidecar.is_file() {
            let mut name = file_name.to_os_string();
            name.push(suffix);
            fs::copy(&sidecar, backup_dir.join(name))
                .map_err(|error| format!("备份 {} 失败：{error}", sidecar.display()))?;
        }
    }

    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Copy the whole `sessions` tree into `<backup>/sessions`.
pub fn backup_sessions(sessions_path: &Path, backup_dir: &Path) -> Result<(), String> {
    if !sessions_path.is_dir() {
        return Ok(());
    }
    copy_dir_recursive(sessions_path, &backup_dir.join("sessions"))
}

pub fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| format!("无法创建 {}：{error}", to.display()))?;

    let entries =
        fs::read_dir(from).map_err(|error| format!("无法读取目录 {}：{error}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取 {}：{error}", from.display()))?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法检查 {}：{error}", source.display()))?;

        if file_type.is_dir() {
            copy_dir_recursive(&source, &destination)?;
        } else if file_type.is_file() {
            fs::copy(&source, &destination)
                .map_err(|error| format!("复制 {} 失败：{error}", source.display()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_directories_are_unique_and_timestamped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = create_backup_dir(dir.path()).expect("first");
        let second = create_backup_dir(dir.path()).expect("second");

        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        assert_eq!(first.parent(), Some(dir.path().join("backups").as_path()));
        let name = first.file_name().expect("name").to_string_lossy().to_string();
        assert!(
            name.starts_with("provider-migration-"),
            "unexpected name {name}"
        );
    }

    #[test]
    fn database_backup_copies_sidecars_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let database = dir.path().join("state_5.sqlite");
        fs::write(&database, b"db").expect("db");
        fs::write(with_suffix(&database, "-wal"), b"wal").expect("wal");
        // no -shm on purpose

        let backup = create_backup_dir(dir.path()).expect("backup dir");
        backup_database(&database, &backup).expect("backup database");

        assert_eq!(fs::read(backup.join("state_5.sqlite")).expect("read"), b"db");
        assert_eq!(
            fs::read(backup.join("state_5.sqlite-wal")).expect("read"),
            b"wal"
        );
        assert!(!backup.join("state_5.sqlite-shm").exists());
    }

    #[test]
    fn sessions_backup_mirrors_nested_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(sessions.join("2026/01")).expect("mkdir");
        fs::write(sessions.join("2026/01/a.jsonl"), b"one").expect("write");
        fs::write(sessions.join("top.jsonl"), b"two").expect("write");

        let backup = create_backup_dir(dir.path()).expect("backup dir");
        backup_sessions(&sessions, &backup).expect("backup sessions");

        assert_eq!(
            fs::read(backup.join("sessions/2026/01/a.jsonl")).expect("read"),
            b"one"
        );
        assert_eq!(
            fs::read(backup.join("sessions/top.jsonl")).expect("read"),
            b"two"
        );
    }

    #[test]
    fn missing_sessions_directory_is_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backup = create_backup_dir(dir.path()).expect("backup dir");
        backup_sessions(&dir.path().join("absent"), &backup).expect("no sessions");
        assert!(!backup.join("sessions").exists());
    }
}
