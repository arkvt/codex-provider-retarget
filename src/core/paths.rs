use std::path::{Path, PathBuf};

/// `~/.codex` for the current user.
pub fn default_codex_home() -> PathBuf {
    home_dir().join(".codex")
}

pub fn home_dir() -> PathBuf {
    for key in ["USERPROFILE", "HOME"] {
        if let Some(value) = std::env::var_os(key)
            && !value.is_empty()
        {
            return PathBuf::from(value);
        }
    }
    if let (Some(drive), Some(path)) = (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH"))
        && !drive.is_empty()
        && !path.is_empty()
    {
        let mut combined = drive;
        combined.push(path);
        return PathBuf::from(combined);
    }
    PathBuf::from(".")
}

/// Turn user input into an absolute path, falling back to the default Codex
/// home when the input is blank. A leading `~` is expanded.
pub fn normalize_codex_home(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return default_codex_home();
    }

    let expanded = if let Some(rest) = trimmed.strip_prefix('~') {
        let rest = rest.trim_start_matches(['/', '\\']);
        if rest.is_empty() {
            home_dir()
        } else {
            home_dir().join(rest)
        }
    } else {
        PathBuf::from(trimmed)
    };

    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or(expanded)
    }
}

/// Render a path the way the UI shows it, without failing on odd encodings.
pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_input_falls_back_to_default() {
        assert_eq!(normalize_codex_home("   "), default_codex_home());
        assert!(normalize_codex_home("").ends_with(".codex"));
    }

    #[test]
    fn absolute_input_is_kept_as_is() {
        let absolute = if cfg!(windows) { r"C:\tmp\codex" } else { "/tmp/codex" };
        assert_eq!(normalize_codex_home(absolute), PathBuf::from(absolute));
    }

    #[test]
    fn relative_input_is_resolved_against_the_working_directory() {
        let resolved = normalize_codex_home("relative-codex");
        assert!(resolved.is_absolute(), "expected absolute, got {resolved:?}");
        assert!(resolved.ends_with("relative-codex"));
    }

    #[test]
    fn tilde_is_expanded_to_the_home_directory() {
        assert_eq!(normalize_codex_home("~"), home_dir());
        assert_eq!(normalize_codex_home("~/.codex"), home_dir().join(".codex"));
    }
}
