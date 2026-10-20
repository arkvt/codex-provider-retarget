use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CodexConfig {
    model_provider: Option<String>,
}

/// Read the provider Codex is currently configured to use.
///
/// A missing `config.toml` or a missing `model_provider` key is not an error —
/// the user simply has to type a target by hand.
pub fn read_config_provider(codex_home: &Path) -> Result<Option<String>, String> {
    let path = codex_home.join("config.toml");
    if !path.is_file() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let parsed: CodexConfig = toml::from_str(&content)
        .map_err(|error| format!("无法解析 {}：{error}", path.display()))?;

    Ok(parsed
        .model_provider
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("config.toml"), body).expect("write config");
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    #[test]
    fn reads_the_configured_provider() {
        let (_dir, home) = write_config("model_provider = \"ccswitch\"\nother = 1\n");
        assert_eq!(
            read_config_provider(&home).expect("parsed"),
            Some("ccswitch".to_string())
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let (_dir, home) = write_config("model_provider = \"  ccswitch  \"\n");
        assert_eq!(
            read_config_provider(&home).expect("parsed"),
            Some("ccswitch".to_string())
        );
    }

    #[test]
    fn missing_file_and_missing_key_are_not_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_config_provider(dir.path()).expect("missing file"), None);

        let (_dir2, home) = write_config("approval_policy = \"never\"\n");
        assert_eq!(read_config_provider(&home).expect("missing key"), None);

        let (_dir3, home3) = write_config("model_provider = \"\"\n");
        assert_eq!(read_config_provider(&home3).expect("blank key"), None);
    }

    #[test]
    fn malformed_toml_is_reported() {
        let (_dir, home) = write_config("model_provider = \n");
        assert!(read_config_provider(&home).is_err());
    }
}
