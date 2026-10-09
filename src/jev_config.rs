//! The stored Jev configuration: where `contextleleo setup` keeps a key so a
//! user who installed the CLI does not have to export it in every shell.
//!
//! The file is `KEY=value` lines (`JEV_API_KEY`, optionally `JEV_API_URL` and
//! `JEV_MODEL`) in the user's config directory, readable by that user only on
//! Unix. The environment always wins over the file, so an exported variable or
//! a CI secret overrides whatever `setup` stored. The key is never printed,
//! logged, or put in an error.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::jev_api::{API_KEY_ALIAS_ENV, API_KEY_ENV};

/// Overrides the config file location (used by tests and unusual setups).
pub const CONFIG_PATH_ENV: &str = "CONTEXTLELEO_CONFIG";

/// Where the config file lives: `$CONTEXTLELEO_CONFIG`, else
/// `$XDG_CONFIG_HOME/contextleleo/config`, else `~/.config/contextleleo/config`
/// (`%APPDATA%\contextleleo\config` on Windows). `None` when no home can be
/// found.
#[must_use]
pub fn path() -> Option<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(explicit) = set(CONFIG_PATH_ENV) {
        return Some(PathBuf::from(explicit));
    }
    let base = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| set("APPDATA").map(PathBuf::from))
        .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("contextleleo").join("config"))
}

/// The `KEY=value` pairs in `file`; blank lines and `#` comments are skipped,
/// a missing or unreadable file is an empty map.
#[must_use]
pub fn load_from(file: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (name, value) = line.split_once('=')?;
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            (!value.is_empty()).then(|| (name.trim().to_string(), value.to_string()))
        })
        .collect()
}

/// [`load_from`] the default [`path`].
#[must_use]
pub fn load() -> HashMap<String, String> {
    path().map(|file| load_from(&file)).unwrap_or_default()
}

/// Where a usable key comes from, for `setup --status`. Never carries the
/// key itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// An environment variable (its name).
    Env(&'static str),
    /// The stored config file.
    File(PathBuf),
    /// No key anywhere.
    None,
}

/// Where the key would come from right now: the environment first, then the
/// config file at `file`.
#[must_use]
pub fn key_source_in(file: Option<&Path>) -> KeySource {
    let set = |name: &str| std::env::var(name).is_ok_and(|value| !value.trim().is_empty());
    if set(API_KEY_ENV) {
        return KeySource::Env(API_KEY_ENV);
    }
    if set(API_KEY_ALIAS_ENV) {
        return KeySource::Env(API_KEY_ALIAS_ENV);
    }
    match file {
        Some(file) if load_from(file).contains_key(API_KEY_ENV) => {
            KeySource::File(file.to_path_buf())
        }
        _ => KeySource::None,
    }
}

/// [`key_source_in`] the default [`path`].
#[must_use]
pub fn key_source() -> KeySource {
    key_source_in(path().as_deref())
}

/// Store `key` as `JEV_API_KEY` in `file` (created with owner-only
/// permissions on Unix), keeping any other settings already there.
///
/// # Errors
/// I/O failures creating the directory or writing the file; a blank key is
/// refused.
pub fn save_key_to(file: &Path, key: &str) -> std::io::Result<()> {
    let key = key.trim();
    if key.is_empty() || key.contains(['\n', '\r']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the key is empty or spans several lines",
        ));
    }
    let mut kept: Vec<String> = std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim_start().starts_with(&format!("{API_KEY_ENV}=")))
        .map(str::to_string)
        .collect();
    kept.push(format!("{API_KEY_ENV}={key}"));
    write_private(file, &(kept.join("\n") + "\n"))
}

/// Remove the stored key from `file`. Returns whether there was one.
///
/// # Errors
/// I/O failures rewriting the file.
pub fn remove_key_from(file: &Path) -> std::io::Result<bool> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Ok(false);
    };
    let prefix = format!("{API_KEY_ENV}=");
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with(&prefix))
        .collect();
    if kept.len() == text.lines().count() {
        return Ok(false);
    }
    if kept.iter().all(|line| line.trim().is_empty()) {
        std::fs::remove_file(file)?;
    } else {
        write_private(file, &(kept.join("\n") + "\n"))?;
    }
    Ok(true)
}

fn write_private(file: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut handle = options.open(file)?;
    #[cfg(unix)]
    {
        // An existing file keeps its old mode through `open`; tighten it.
        use std::os::unix::fs::PermissionsExt as _;
        handle.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    handle.write_all(contents.as_bytes())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("nested").join("config");
        (dir, file)
    }

    #[test]
    fn a_saved_key_round_trips_and_keeps_other_settings() {
        let (_dir, file) = scratch();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "# mine\nJEV_MODEL=jev-latest\nJEV_API_KEY=old\n").unwrap();
        save_key_to(&file, "  new-key-value \n").unwrap();
        let loaded = load_from(&file);
        assert_eq!(
            loaded.get("JEV_API_KEY").map(String::as_str),
            Some("new-key-value")
        );
        assert_eq!(
            loaded.get("JEV_MODEL").map(String::as_str),
            Some("jev-latest")
        );
        assert_eq!(
            std::fs::read_to_string(&file)
                .unwrap()
                .matches("JEV_API_KEY")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_config_file_is_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, file) = scratch();
        save_key_to(&file, "k1234567890").unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // Saving again over a loosened file tightens it back.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_key_to(&file, "k1234567890").unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn blank_and_multiline_keys_are_refused() {
        let (_dir, file) = scratch();
        assert!(save_key_to(&file, "   ").is_err());
        assert!(save_key_to(&file, "a\nb").is_err());
        assert!(!file.exists());
    }

    #[test]
    fn removing_the_key_reports_whether_there_was_one() {
        let (_dir, file) = scratch();
        assert!(!remove_key_from(&file).unwrap(), "no file, nothing removed");
        save_key_to(&file, "k1234567890").unwrap();
        assert!(remove_key_from(&file).unwrap());
        assert!(!file.exists(), "a file with nothing else left is deleted");
        assert!(!remove_key_from(&file).unwrap());
    }

    #[test]
    fn load_skips_comments_blanks_and_strips_quotes() {
        let (_dir, file) = scratch();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "# c\n\nJEV_API_KEY=\"quoted\"\nbad line\nEMPTY=\n").unwrap();
        let loaded = load_from(&file);
        assert_eq!(
            loaded.get("JEV_API_KEY").map(String::as_str),
            Some("quoted")
        );
        assert_eq!(loaded.len(), 1);
    }
}
