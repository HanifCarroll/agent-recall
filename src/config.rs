use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_LAUNCH_AGENT_LABEL: &str = "dev.agent-recall.watch";
pub const LEGACY_LAUNCH_AGENT_LABELS: &[&str] =
    &["dev.codex-recall.watch", "com.hanif.codex-recall.watch"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub index_path: PathBuf,
    pub source_roots: Vec<PathBuf>,
}

pub fn default_db_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("AGENT_RECALL_DB") {
        return Ok(PathBuf::from(path));
    }

    Ok(data_home()?.join("agent-recall").join("index.sqlite"))
}

pub fn default_state_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("AGENT_RECALL_STATE") {
        return Ok(PathBuf::from(path));
    }

    Ok(state_home()?.join("agent-recall").join("watch.json"))
}

pub fn default_pins_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("AGENT_RECALL_PINS") {
        return Ok(PathBuf::from(path));
    }

    Ok(data_home()?.join("agent-recall").join("pins.json"))
}

pub fn default_source_roots() -> Result<Vec<PathBuf>> {
    let home = home_dir()?;
    let codex_home = codex_home()?;
    let omp_home = env_path("OMP_HOME").unwrap_or_else(|| home.join(".omp"));
    let pi_home = env_path("PI_HOME").unwrap_or_else(|| home.join(".pi"));
    let claude_home = env_path("CLAUDE_HOME").unwrap_or_else(|| home.join(".claude"));
    Ok(vec![
        codex_home.join("sessions"),
        codex_home.join("archived_sessions"),
        omp_home.join("agent").join("sessions"),
        pi_home.join("agent").join("sessions"),
        claude_home.join("projects"),
    ])
}

pub fn default_codex_log_sources() -> Result<Vec<PathBuf>> {
    let root = codex_home()?.join("archived_logs");
    let mut sources = Vec::new();
    collect_log_sqlite_files(&root, &mut sources)?;
    sources.sort();
    Ok(sources)
}

fn collect_log_sqlite_files(root: &Path, sources: &mut Vec<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", path.display()))?;
        if file_type.is_dir() {
            collect_log_sqlite_files(&path, sources)?;
        } else if is_codex_log_sqlite(&path) {
            sources.push(path);
        }
    }

    Ok(())
}

fn is_codex_log_sqlite(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    file_name.starts_with("logs_") && file_name.ends_with(".sqlite")
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))
}

fn data_home() -> Result<PathBuf> {
    Ok(env_path("XDG_DATA_HOME").unwrap_or(home_dir()?.join(".local").join("share")))
}

fn state_home() -> Result<PathBuf> {
    Ok(env_path("XDG_STATE_HOME").unwrap_or(home_dir()?.join(".local").join("state")))
}

fn codex_home() -> Result<PathBuf> {
    Ok(env_path("CODEX_HOME").unwrap_or(home_dir()?.join(".codex")))
}

fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct EnvGuard {
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&'static str, Option<&str>)]) -> Self {
            let values = pairs
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect::<Vec<_>>();
            for (key, value) in pairs {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            Self { values }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.values {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn data_and_state_paths_honor_xdg_locations() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::set(&[
            ("AGENT_RECALL_DB", None),
            ("AGENT_RECALL_STATE", None),
            ("AGENT_RECALL_PINS", None),
            ("XDG_DATA_HOME", Some("/tmp/xdg-data")),
            ("XDG_STATE_HOME", Some("/tmp/xdg-state")),
            ("HOME", Some("/tmp/home")),
        ]);

        assert_eq!(
            default_db_path().unwrap(),
            PathBuf::from("/tmp/xdg-data/agent-recall/index.sqlite")
        );
        assert_eq!(
            default_state_path().unwrap(),
            PathBuf::from("/tmp/xdg-state/agent-recall/watch.json")
        );
        assert_eq!(
            default_pins_path().unwrap(),
            PathBuf::from("/tmp/xdg-data/agent-recall/pins.json")
        );
    }

    #[test]
    fn source_roots_honor_codex_home_when_set() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::set(&[
            ("CODEX_HOME", Some("/tmp/codex-home")),
            ("OMP_HOME", None),
            ("PI_HOME", None),
            ("CLAUDE_HOME", None),
            ("HOME", Some("/tmp/home")),
        ]);

        assert_eq!(
            default_source_roots().unwrap(),
            vec![
                PathBuf::from("/tmp/codex-home/sessions"),
                PathBuf::from("/tmp/codex-home/archived_sessions"),
                PathBuf::from("/tmp/home/.omp/agent/sessions"),
                PathBuf::from("/tmp/home/.pi/agent/sessions"),
                PathBuf::from("/tmp/home/.claude/projects"),
            ]
        );
    }
}
