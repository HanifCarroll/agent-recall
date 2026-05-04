use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

pub(crate) struct RefreshLock {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct RefreshLockStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub active: bool,
    pub stale: bool,
    pub age_ms: Option<u128>,
    pub stale_after_ms: u128,
    pub pid: Option<u32>,
    pub owner_alive: Option<bool>,
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn refresh_lock_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.refresh.lock", db_path.display()))
}

pub(crate) fn refresh_lock_wait_timeout() -> Duration {
    env_duration_ms(
        "CODEX_RECALL_REFRESH_LOCK_WAIT_MS",
        Duration::from_secs(120),
    )
}

pub(crate) fn refresh_lock_status(db_path: &Path) -> RefreshLockStatus {
    let path = refresh_lock_path(db_path);
    let stale_after = refresh_lock_stale_timeout();
    let (exists, age_ms, pid) = match fs::metadata(&path) {
        Ok(metadata) => {
            let age_ms = metadata
                .modified()
                .ok()
                .and_then(|modified_at| modified_at.elapsed().ok())
                .map(|age| age.as_millis());
            let pid = read_lock_pid(&path);
            (true, age_ms, pid)
        }
        Err(_) => (false, None, None),
    };
    let stale_after_ms = stale_after.as_millis();
    let owner_alive = pid.and_then(lock_owner_alive);
    let stale = exists
        && match owner_alive {
            Some(true) => false,
            Some(false) => true,
            None => age_ms.map(|age| age >= stale_after_ms).unwrap_or(false),
        };

    RefreshLockStatus {
        path,
        exists,
        active: exists && !stale,
        stale,
        age_ms,
        stale_after_ms,
        pid,
        owner_alive,
    }
}

pub(crate) fn acquire_refresh_lock(
    db_path: &Path,
    wait_for: Duration,
) -> Result<Option<RefreshLock>> {
    let path = refresh_lock_path(db_path);
    let started = Instant::now();
    let stale_after = refresh_lock_stale_timeout();

    loop {
        match try_create_lock(&path) {
            Ok(lock) => return Ok(Some(lock)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                remove_stale_lock_if_needed(&path, stale_after)?;
                if wait_for.is_zero() || started.elapsed() >= wait_for {
                    return Ok(None);
                }
                thread::sleep(
                    Duration::from_millis(250).min(wait_for.saturating_sub(started.elapsed())),
                );
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create refresh lock {}", path.display()))
            }
        }
    }
}

fn try_create_lock(path: &Path) -> std::io::Result<RefreshLock> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    writeln!(
        file,
        "pid={}\ncreated_at={:?}",
        std::process::id(),
        SystemTime::now()
    )?;
    Ok(RefreshLock {
        path: path.to_path_buf(),
    })
}

fn remove_stale_lock_if_needed(path: &Path, stale_after: Duration) -> Result<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    let Ok(modified_at) = metadata.modified() else {
        return Ok(());
    };
    let Ok(age) = modified_at.elapsed() else {
        return Ok(());
    };
    match read_lock_pid(path).and_then(lock_owner_alive) {
        Some(true) => return Ok(()),
        Some(false) => {}
        None if age < stale_after => return Ok(()),
        None => {}
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove stale lock {}", path.display())),
    }
}

fn read_lock_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok().and_then(|contents| {
        contents.lines().find_map(|line| {
            line.strip_prefix("pid=")
                .and_then(|value| value.parse::<u32>().ok())
        })
    })
}

fn lock_owner_alive(pid: u32) -> Option<bool> {
    if pid == std::process::id() {
        return Some(true);
    }
    let output = ProcessCommand::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .ok()?;
    if output.status.success() {
        return Some(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if stderr.contains("no such process") {
        return Some(false);
    }
    None
}

fn refresh_lock_stale_timeout() -> Duration {
    env_duration_ms(
        "CODEX_RECALL_REFRESH_LOCK_STALE_MS",
        Duration::from_secs(15 * 60),
    )
}

fn env_duration_ms(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}
