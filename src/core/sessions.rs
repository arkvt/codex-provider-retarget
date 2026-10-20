use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::types::ProviderDistribution;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionUpdate {
    pub changed: bool,
    pub provider: Option<String>,
}

/// How one line of a session file relates to the metadata we care about.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MetaLine {
    /// Not the `session_meta` record.
    Other,
    /// The `session_meta` record, but with no usable `payload` object.
    UnusablePayload,
    /// The `session_meta` record with its `payload.model_provider` value.
    Provider(Option<String>),
}

fn classify_line(line: &str) -> MetaLine {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return MetaLine::Other;
    };
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return MetaLine::Other;
    }

    match value.get("payload") {
        Some(Value::Object(payload)) => {
            MetaLine::Provider(payload.get("model_provider").and_then(Value::as_str).map(str::to_string))
        }
        _ => MetaLine::UnusablePayload,
    }
}

/// Split a line into its content and its original line ending so that rewrites
/// preserve CRLF vs LF exactly.
fn split_line_ending(segment: &str) -> (&str, &str) {
    if let Some(body) = segment.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = segment.strip_suffix('\n') {
        (body, "\n")
    } else {
        (segment, "")
    }
}

/// All `.jsonl` files below `root`, sorted for deterministic output.
pub fn walk_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !root.exists() {
        return found;
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
            {
                found.push(path);
            }
        }
    }

    found.sort();
    found
}

/// The provider recorded in the first `session_meta` record of `file`.
///
/// `Ok(None)` means the file simply has no usable metadata; `Err` means it could
/// not be read at all — typically because a running Codex host holds it open.
pub fn try_read_session_meta_provider(file: &Path) -> Result<Option<String>, String> {
    let content = fs::read_to_string(file)
        .map_err(|error| format!("无法读取 {}：{error}", file.display()))?;
    for line in content.lines() {
        match classify_line(line) {
            MetaLine::Provider(provider) => return Ok(provider),
            MetaLine::UnusablePayload => return Ok(None),
            MetaLine::Other => {}
        }
    }
    Ok(None)
}

/// Convenience wrapper for callers that treat an unreadable file as "no
/// metadata" (per-file failures are collected elsewhere).
pub fn read_session_meta_provider(file: &Path) -> Option<String> {
    try_read_session_meta_provider(file).ok().flatten()
}

/// Outcome of walking the sessions tree.
#[derive(Debug, Clone, Default)]
pub struct SessionScan {
    pub distribution: ProviderDistribution,
    /// Number of files that yielded a provider.
    pub count: usize,
    /// Files that exist but could not be read at all.
    pub unreadable: Vec<String>,
}

pub fn scan_session_distribution(sessions_path: &Path) -> SessionScan {
    let mut scan = SessionScan::default();

    for file in walk_jsonl_files(sessions_path) {
        match try_read_session_meta_provider(&file) {
            Ok(Some(provider)) => {
                *scan.distribution.entry(provider).or_insert(0) += 1;
                scan.count += 1;
            }
            Ok(None) => {}
            Err(error) => scan.unreadable.push(error),
        }
    }

    scan
}

/// Rewrite `payload.model_provider` on the `session_meta` line.
///
/// Only that single line is replaced; every other byte of the file, including
/// its original line endings, is copied through untouched.
pub fn update_session_provider(file: &Path, target_provider: &str) -> Result<SessionUpdate, String> {
    let raw = fs::read_to_string(file)
        .map_err(|error| format!("无法读取 {}：{error}", file.display()))?;

    let mut output = String::with_capacity(raw.len() + 16);
    let mut result = SessionUpdate {
        changed: false,
        provider: None,
    };
    let mut decided = false;

    for segment in raw.split_inclusive('\n') {
        if decided {
            output.push_str(segment);
            continue;
        }

        let (body, ending) = split_line_ending(segment);
        match classify_line(body) {
            MetaLine::Other => output.push_str(segment),
            MetaLine::UnusablePayload => {
                decided = true;
                output.push_str(segment);
            }
            MetaLine::Provider(current) => {
                decided = true;
                result.provider = current.clone();
                if current.as_deref() == Some(target_provider) {
                    output.push_str(segment);
                } else if let Some(rewritten) = rewrite_provider(body, target_provider) {
                    output.push_str(&rewritten);
                    output.push_str(ending);
                    result.changed = true;
                } else {
                    output.push_str(segment);
                }
            }
        }
    }

    if result.changed {
        fs::write(file, output).map_err(|error| format!("无法写入 {}：{error}", file.display()))?;
    }

    Ok(result)
}

fn rewrite_provider(body: &str, target_provider: &str) -> Option<String> {
    let mut value: Value = serde_json::from_str(body).ok()?;
    let payload = value.get_mut("payload")?.as_object_mut()?;
    payload.insert(
        "model_provider".to_string(),
        Value::String(target_provider.to_string()),
    );
    serde_json::to_string(&value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_session(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(&path, body).expect("write session");
        path
    }

    #[test]
    fn walks_jsonl_files_recursively_and_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), "b/second.jsonl", "");
        write_session(dir.path(), "a/first.jsonl", "");
        write_session(dir.path(), "top.jsonl", "");
        write_session(dir.path(), "notes.txt", "");

        let found = walk_jsonl_files(dir.path());
        let names: Vec<String> = found
            .iter()
            .map(|path| {
                path.strip_prefix(dir.path())
                    .expect("prefix")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["a/first.jsonl", "b/second.jsonl", "top.jsonl"]);
    }

    #[test]
    fn missing_sessions_directory_yields_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(walk_jsonl_files(&dir.path().join("nope")).is_empty());
    }

    #[test]
    fn reads_provider_from_the_session_meta_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_session(
            dir.path(),
            "s.jsonl",
            "{\"type\":\"message\",\"payload\":{}}\n{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\",\"id\":\"x\"}}\n{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"other\"}}\n",
        );
        assert_eq!(
            read_session_meta_provider(&path),
            Some("openai".to_string())
        );
    }

    #[test]
    fn unparsable_lines_are_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_session(
            dir.path(),
            "s.jsonl",
            "{not json at all\n\ntrailing garbage\n{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"codex\"}}\n",
        );
        assert_eq!(read_session_meta_provider(&path), Some("codex".to_string()));
    }

    #[test]
    fn scans_distribution_and_counts_only_parsable_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(
            dir.path(),
            "a.jsonl",
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n",
        );
        write_session(
            dir.path(),
            "b.jsonl",
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n",
        );
        write_session(
            dir.path(),
            "c.jsonl",
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"codex\"}}\n",
        );
        write_session(dir.path(), "d.jsonl", "{\"payload\":{}}\n");

        let scan = scan_session_distribution(dir.path());
        assert_eq!(scan.count, 3);
        assert_eq!(scan.distribution.get("openai"), Some(&2));
        assert_eq!(scan.distribution.get("codex"), Some(&1));
        assert!(scan.unreadable.is_empty());
    }

    #[test]
    fn unreadable_files_are_reported_but_do_not_abort_the_scan() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(
            dir.path(),
            "good.jsonl",
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n",
        );
        // Invalid UTF-8 stands in for a file held open by another process or a
        // corrupted file: both make `read_to_string` fail the same way.
        fs::write(dir.path().join("bad.jsonl"), [0xFFu8, 0xFE, 0x00, 0x9F]).expect("write");

        let scan = scan_session_distribution(dir.path());

        assert_eq!(scan.count, 1, "the readable file must still be counted");
        assert_eq!(scan.distribution.get("openai"), Some(&1));
        assert_eq!(scan.unreadable.len(), 1);
        assert!(
            scan.unreadable[0].contains("bad.jsonl"),
            "unexpected message: {:?}",
            scan.unreadable
        );
    }

    #[test]
    fn rewrite_replaces_only_the_meta_line_and_keeps_other_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = "{\"type\":\"message\",\"payload\":{\"text\":\"session_meta\"}}\n{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\",\"id\":\"abc\"}}\n{\"type\":\"event\",\"payload\":{\"n\":1}}\n";
        let path = write_session(dir.path(), "s.jsonl", original);

        let update = update_session_provider(&path, "ccswitch").expect("update");
        assert!(update.changed);
        assert_eq!(update.provider, Some("openai".to_string()));

        let rewritten = fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = rewritten.lines().collect();
        assert_eq!(lines[0], "{\"type\":\"message\",\"payload\":{\"text\":\"session_meta\"}}");
        assert_eq!(lines[2], "{\"type\":\"event\",\"payload\":{\"n\":1}}");
        assert!(lines[1].contains("\"model_provider\":\"ccswitch\""));
        assert!(lines[1].contains("\"id\":\"abc\""));
        assert!(rewritten.ends_with('\n'));
        assert_eq!(rewritten.lines().count(), 3);
    }

    #[test]
    fn rewrite_scans_past_lines_that_merely_look_like_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_session(
            dir.path(),
            "s.jsonl",
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\n{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"codex\"}}\n",
        );
        let update = update_session_provider(&path, "ccswitch").expect("update");
        assert_eq!(update.provider, Some("openai".to_string()));

        let rewritten = fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = rewritten.lines().collect();
        assert!(lines[0].contains("ccswitch"));
        assert!(
            lines[1].contains("\"model_provider\":\"codex\""),
            "the second record must be left alone: {}",
            lines[1]
        );
    }

    #[test]
    fn rewrite_preserves_crlf_line_endings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_session(
            dir.path(),
            "s.jsonl",
            "{\"type\":\"message\",\"payload\":{}}\r\n{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}\r\n",
        );

        update_session_provider(&path, "ccswitch").expect("update");
        let rewritten = fs::read_to_string(&path).expect("read");
        assert!(!rewritten.contains('\n') || rewritten.contains("\r\n"));
        assert_eq!(rewritten.matches("\r\n").count(), 2);
        assert!(!rewritten.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn rewrite_is_a_no_op_when_already_on_the_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_session(
            dir.path(),
            "s.jsonl",
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"ccswitch\"}}\n",
        );
        let update = update_session_provider(&path, "ccswitch").expect("update");
        assert!(!update.changed);
        assert_eq!(update.provider, Some("ccswitch".to_string()));
    }

    #[test]
    fn rewrite_handles_files_without_a_trailing_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_session(
            dir.path(),
            "s.jsonl",
            "{\"type\":\"session_meta\",\"payload\":{\"model_provider\":\"openai\"}}",
        );
        update_session_provider(&path, "ccswitch").expect("update");
        let rewritten = fs::read_to_string(&path).expect("read");
        assert!(rewritten.contains("ccswitch"));
        assert!(!rewritten.ends_with('\n'));
    }

    #[test]
    fn rewrite_skips_files_without_session_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_session(dir.path(), "s.jsonl", "{\"type\":\"event\",\"payload\":{}}\n");
        let update = update_session_provider(&path, "ccswitch").expect("update");
        assert!(!update.changed);
        assert_eq!(update.provider, None);
    }
}
