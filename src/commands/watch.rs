use crate::commands::index::resolve_sources;
use crate::config::{
    default_db_path, default_state_path, DEFAULT_LAUNCH_AGENT_LABEL, LEGACY_LAUNCH_AGENT_LABELS,
};
use crate::indexer::{
    index_sources_with_filters_and_progress,
    index_stable_pending_sources_with_filters_and_progress, scan_sources_for_pending_with_filters,
    IndexFilters, SourceScanReport,
};
use crate::output::{format_bytes, now_timestamp, progress_line};
use crate::refresh_lock::{
    acquire_refresh_lock, refresh_lock_status, refresh_lock_wait_timeout, RefreshLockStatus,
};
use crate::store::Store;
use anyhow::{anyhow, Context, Result};
use clap::Args;
use rusqlite::{Error as SqliteError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Args)]
pub struct WatchArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, help = "Watch state JSON path")]
    pub state: Option<PathBuf>,
    #[arg(long = "source", help = "Session archive root to scan; repeatable")]
    pub sources: Vec<PathBuf>,
    #[arg(long, default_value_t = 10, help = "Seconds between watch scans")]
    pub interval: u64,
    #[arg(
        long,
        default_value_t = 3,
        help = "Seconds a file must be quiet before indexing"
    )]
    pub quiet_for: u64,
    #[arg(long, help = "Run one scan and exit")]
    pub once: bool,
    #[arg(
        long,
        help = "Only index sessions whose session or command cwd matches this repo"
    )]
    pub repo: Option<String>,
    #[arg(long, help = "Only index sessions at or after this date/time")]
    pub since: Option<String>,
    #[arg(long, help = "Write a macOS LaunchAgent plist for background indexing")]
    pub install_launch_agent: bool,
    #[arg(long, help = "Bootstrap and verify the LaunchAgent after writing it")]
    pub start_launch_agent: bool,
    #[arg(
        long,
        default_value = DEFAULT_LAUNCH_AGENT_LABEL,
        help = "LaunchAgent label"
    )]
    pub agent_label: String,
    #[arg(long, help = "LaunchAgent plist output path")]
    pub agent_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct StatusArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, help = "Watch state JSON path")]
    pub state: Option<PathBuf>,
    #[arg(long = "source", help = "Session archive root to scan; repeatable")]
    pub sources: Vec<PathBuf>,
    #[arg(
        long,
        default_value_t = 3,
        help = "Seconds a file must be quiet before indexing"
    )]
    pub quiet_for: u64,
    #[arg(
        long,
        help = "Only inspect sessions whose session or command cwd matches this repo"
    )]
    pub repo: Option<String>,
    #[arg(long, help = "Only inspect sessions at or after this date/time")]
    pub since: Option<String>,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
    #[arg(
        long,
        default_value = DEFAULT_LAUNCH_AGENT_LABEL,
        help = "LaunchAgent label"
    )]
    pub agent_label: String,
    #[arg(long, help = "LaunchAgent plist path")]
    pub agent_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct WatchState {
    last_run_at: Option<String>,
    last_indexed_at: Option<String>,
    last_error: Option<String>,
    last_indexed_sessions: usize,
    last_indexed_events: usize,
    last_files_seen: usize,
    last_files_total: usize,
    pending_files: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct WatchStatusReport {
    pub db_path: PathBuf,
    pub db_exists: bool,
    pub state_path: PathBuf,
    pub sources: Vec<PathBuf>,
    pub filters: IndexFilters,
    pub scan: SourceScanReport,
    pub state: WatchState,
    pub last_indexed_at: Option<String>,
    pub freshness: FreshnessVerdict,
    pub refresh_lock: RefreshLockStatus,
    pub launch_agent: LaunchAgentStatus,
}

#[derive(Debug, Clone)]
pub(crate) struct FreshnessVerdict {
    pub state: &'static str,
    pub message: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LaunchAgentStatus {
    pub label: String,
    pub path: PathBuf,
    pub supported: bool,
    pub installed: bool,
    pub running: Option<bool>,
}

struct LaunchAgentWatchConfig<'a> {
    interval: Duration,
    quiet_for: Duration,
    filters: &'a IndexFilters,
}

pub fn run_watch(args: WatchArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let state_path = args.state.unwrap_or(default_state_path()?);
    let sources = resolve_sources(args.sources)?;
    let interval = Duration::from_secs(args.interval);
    let quiet_for = Duration::from_secs(args.quiet_for);
    let filters = IndexFilters::new(args.repo.clone(), args.since.clone())?;

    if args.install_launch_agent {
        if !launch_agent_supported() {
            return Err(anyhow!(
                "watch --install-launch-agent is only supported on macOS"
            ));
        }
        let agent_path = args
            .agent_path
            .unwrap_or(default_launch_agent_path(&args.agent_label)?);
        install_launch_agent_plist(
            &agent_path,
            &args.agent_label,
            &db_path,
            &state_path,
            &sources,
            LaunchAgentWatchConfig {
                interval,
                quiet_for,
                filters: &filters,
            },
        )?;
        println!("installed launch agent: {}", agent_path.display());
        let retired = retire_legacy_launch_agents(&args.agent_label, &agent_path)?;
        for label in retired {
            println!("retired legacy launch agent: {label}");
        }
        if args.start_launch_agent {
            start_launch_agent(&args.agent_label, &agent_path)?;
            println!("started launch agent: {}", args.agent_label);
        }
        println!(
            "next: launchctl bootstrap gui/$(id -u) {}",
            agent_path.display()
        );
        return Ok(());
    }

    loop {
        match run_watch_iteration_with_lock_retries(
            &db_path,
            &state_path,
            &sources,
            quiet_for,
            &filters,
            if args.once {
                refresh_lock_wait_timeout()
            } else {
                Duration::ZERO
            },
        ) {
            Ok(()) => {
                if args.once {
                    return Ok(());
                }
            }
            Err(error) => {
                let mut state = read_watch_state(&state_path).unwrap_or_default();
                state.last_run_at = Some(now_timestamp());
                state.last_error = Some(format!("{error:#}"));
                let _ = write_watch_state(&state_path, &state);
                if args.once {
                    return Err(error);
                }
                eprintln!("watch error: {error:#}");
            }
        }
        thread::sleep(interval);
    }
}

fn run_watch_iteration_with_lock_retries(
    db_path: &Path,
    state_path: &Path,
    sources: &[PathBuf],
    quiet_for: Duration,
    filters: &IndexFilters,
    refresh_lock_wait: Duration,
) -> Result<()> {
    let Some(_refresh_lock) = acquire_refresh_lock(db_path, refresh_lock_wait)? else {
        println!("watch using current index: another refresh is already active");
        return Ok(());
    };

    let retry_delays = lock_retry_delays();
    let total_attempts = retry_delays.len() + 1;

    for attempt in 0..total_attempts {
        match run_watch_iteration(db_path, state_path, sources, quiet_for, filters) {
            Ok(()) => return Ok(()),
            Err(error) if is_database_lock_error(&error) => {
                if attempt == retry_delays.len() {
                    return use_stale_index_after_lock(
                        db_path,
                        state_path,
                        sources,
                        quiet_for,
                        filters,
                        &error,
                        total_attempts,
                    );
                }
                let delay = retry_delays[attempt];
                eprintln!(
                    "watch refresh blocked by database lock; retrying in {} ms",
                    delay.as_millis()
                );
                thread::sleep(delay);
            }
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

fn run_watch_iteration(
    db_path: &Path,
    state_path: &Path,
    sources: &[PathBuf],
    quiet_for: Duration,
    filters: &IndexFilters,
) -> Result<()> {
    let mut state = read_watch_state(state_path).unwrap_or_default();
    let (scan, last_indexed_at) = scan_status(db_path, sources, quiet_for, filters)?;
    state.last_run_at = Some(now_timestamp());
    state.pending_files = scan.pending_files;
    state.last_error = None;

    if scan.pending_files == 0 {
        state.last_indexed_at = last_indexed_at.or(state.last_indexed_at);
        write_watch_state(state_path, &state)?;
        println!("watch idle: no pending files");
        return Ok(());
    }

    if scan.stable_pending_files == 0 {
        state.last_indexed_at = last_indexed_at.or(state.last_indexed_at);
        write_watch_state(state_path, &state)?;
        println!(
            "watch waiting: {} pending files, {} still within quiet window",
            scan.pending_files, scan.waiting_files
        );
        return Ok(());
    }

    let store = Store::open(db_path)?;
    let started = Instant::now();
    let report = if scan.waiting_files > 0 {
        index_stable_pending_sources_with_filters_and_progress(
            &store,
            sources,
            quiet_for,
            filters,
            |report| eprintln!("{}", progress_line(report, started.elapsed())),
        )?
    } else {
        index_sources_with_filters_and_progress(&store, sources, filters, |report| {
            eprintln!("{}", progress_line(report, started.elapsed()));
        })?
    };
    let remaining =
        scan_sources_for_pending_with_filters(Some(&store), sources, quiet_for, filters)?;

    state.last_run_at = Some(now_timestamp());
    state.last_indexed_at = store
        .last_indexed_at()?
        .or_else(|| state.last_run_at.clone());
    state.last_indexed_sessions = report.sessions_indexed;
    state.last_indexed_events = report.events_indexed;
    state.last_files_seen = report.files_seen;
    state.last_files_total = report.files_total;
    state.pending_files = remaining.pending_files;
    state.last_error = None;
    write_watch_state(state_path, &state)?;

    let remaining_message = if remaining.waiting_files > 0 {
        format!(
            "{} pending files remain ({} still within quiet window)",
            remaining.pending_files, remaining.waiting_files
        )
    } else {
        format!("{} pending files remain", remaining.pending_files)
    };
    println!(
        "watch indexed {} session files, {} events from {}/{} files; {}",
        report.sessions_indexed,
        report.events_indexed,
        report.files_seen,
        report.files_total,
        remaining_message
    );
    Ok(())
}

fn use_stale_index_after_lock(
    db_path: &Path,
    state_path: &Path,
    sources: &[PathBuf],
    quiet_for: Duration,
    filters: &IndexFilters,
    error: &anyhow::Error,
    attempts: usize,
) -> Result<()> {
    let mut state = read_watch_state(state_path).unwrap_or_default();
    let (scan, last_indexed_at) =
        scan_status(db_path, sources, quiet_for, filters).unwrap_or_else(|_| {
            (
                scan_sources_for_pending_with_filters(None, sources, quiet_for, filters)
                    .unwrap_or_else(|_| SourceScanReport {
                        files_total: 0,
                        pending_files: state.pending_files,
                        pending_bytes: 0,
                        stable_pending_files: 0,
                        waiting_files: 0,
                        missing_sources: Vec::new(),
                    }),
                state.last_indexed_at.clone(),
            )
        });
    let message = stale_index_lock_message(error, attempts);
    state.last_run_at = Some(now_timestamp());
    state.last_indexed_at = last_indexed_at.or(state.last_indexed_at);
    state.pending_files = scan.pending_files;
    state.last_error = Some(message.clone());
    write_watch_state(state_path, &state)?;

    println!("watch using stale index: {message}");
    Ok(())
}

pub fn run_status(args: StatusArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let state_path = args.state.unwrap_or(default_state_path()?);
    let sources = resolve_sources(args.sources)?;
    let quiet_for = Duration::from_secs(args.quiet_for);
    let filters = IndexFilters::new(args.repo, args.since)?;
    let agent_path = args
        .agent_path
        .unwrap_or(default_launch_agent_path(&args.agent_label)?);
    let report = build_status_report(
        db_path,
        state_path,
        sources,
        filters,
        quiet_for,
        args.agent_label,
        agent_path,
    )?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&status_json(&report))?);
        return Ok(());
    }

    let db_status = if report.db_exists {
        "exists"
    } else {
        "missing"
    };
    println!("database: {} ({db_status})", report.db_path.display());
    println!("state: {}", report.state_path.display());
    if let Some(filters) = format_filters(&report.filters) {
        println!("filters: {filters}");
    }
    println!(
        "freshness: {} ({})",
        report.freshness.state, report.freshness.message
    );
    println!(
        "pending: {} files, {} stable, {} waiting, {}",
        report.scan.pending_files,
        report.scan.stable_pending_files,
        report.scan.waiting_files,
        format_bytes(report.scan.pending_bytes)
    );
    println!(
        "last_indexed_at: {}",
        report
            .last_indexed_at
            .clone()
            .unwrap_or_else(|| "never".to_owned())
    );
    println!(
        "last_error: {}",
        report
            .state
            .last_error
            .clone()
            .unwrap_or_else(|| "none".to_owned())
    );
    println!(
        "refresh_lock: {} (active: {}, stale: {})",
        report.refresh_lock.path.display(),
        report.refresh_lock.active,
        report.refresh_lock.stale
    );
    if report.launch_agent.supported {
        println!(
            "launch_agent: {} (installed: {}, running: {})",
            report.launch_agent.label,
            report.launch_agent.installed,
            report
                .launch_agent
                .running
                .map(|running| running.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        );
    } else {
        println!(
            "launch_agent: unsupported on this platform (label: {}, path: {})",
            report.launch_agent.label,
            report.launch_agent.path.display()
        );
    }
    for source in report.sources {
        let status = if source.exists() { "exists" } else { "missing" };
        println!("source: {} ({status})", source.display());
    }
    Ok(())
}

pub(crate) fn build_status_report(
    db_path: PathBuf,
    state_path: PathBuf,
    sources: Vec<PathBuf>,
    filters: IndexFilters,
    quiet_for: Duration,
    agent_label: String,
    agent_path: PathBuf,
) -> Result<WatchStatusReport> {
    let state = read_watch_state(&state_path).unwrap_or_default();
    let (scan, last_indexed_at) = scan_status(&db_path, &sources, quiet_for, &filters)?;
    let last_indexed_at = last_indexed_at.or(state.last_indexed_at.clone());
    let db_exists = db_path.exists();
    let launch_agent = launch_agent_status(agent_label, agent_path);
    let refresh_lock = refresh_lock_status(&db_path);
    let freshness = freshness_verdict(db_exists, &scan, &state, &refresh_lock, &launch_agent);

    Ok(WatchStatusReport {
        db_path,
        db_exists,
        state_path,
        sources,
        filters,
        scan,
        state,
        last_indexed_at,
        freshness,
        refresh_lock,
        launch_agent,
    })
}

pub(crate) fn status_json(report: &WatchStatusReport) -> Value {
    json!({
        "db_path": report.db_path,
        "db_exists": report.db_exists,
        "state_path": report.state_path,
        "sources": report.sources,
        "filters": {
            "repo": report.filters.repo(),
            "since": report.filters.since_value(),
        },
        "files_total": report.scan.files_total,
        "pending_files": report.scan.pending_files,
        "pending_bytes": report.scan.pending_bytes,
        "stable_pending_files": report.scan.stable_pending_files,
        "waiting_files": report.scan.waiting_files,
        "missing_sources": report.scan.missing_sources,
        "freshness": report.freshness.state,
        "freshness_message": report.freshness.message,
        "last_run_at": report.state.last_run_at,
        "last_indexed_at": report.last_indexed_at,
        "last_error": report.state.last_error,
        "last_indexed_sessions": report.state.last_indexed_sessions,
        "last_indexed_events": report.state.last_indexed_events,
        "refresh_lock": {
            "path": report.refresh_lock.path,
            "exists": report.refresh_lock.exists,
            "active": report.refresh_lock.active,
            "stale": report.refresh_lock.stale,
            "age_ms": report.refresh_lock.age_ms,
            "stale_after_ms": report.refresh_lock.stale_after_ms,
            "pid": report.refresh_lock.pid,
            "owner_alive": report.refresh_lock.owner_alive,
        },
        "launch_agent": {
            "label": report.launch_agent.label,
            "path": report.launch_agent.path,
            "supported": report.launch_agent.supported,
            "installed": report.launch_agent.installed,
            "running": report.launch_agent.running,
        },
    })
}

fn freshness_verdict(
    db_exists: bool,
    scan: &SourceScanReport,
    state: &WatchState,
    refresh_lock: &RefreshLockStatus,
    launch_agent: &LaunchAgentStatus,
) -> FreshnessVerdict {
    if refresh_lock.active {
        let owner = match launch_agent.running {
            Some(true) => "background watcher",
            _ => "another refresh",
        };
        return FreshnessVerdict {
            state: "refreshing",
            message: format!("{owner} is active; current index remains usable"),
        };
    }

    if refresh_lock.stale {
        return FreshnessVerdict {
            state: "stale",
            message: "refresh lock appears stale; run a bounded refresh to clear it".to_owned(),
        };
    }

    if let Some(error) = &state.last_error {
        if is_database_lock_text(error) {
            return FreshnessVerdict {
                state: "using-stale-index",
                message: format!(
                    "refresh failed because database is locked; using stale index: {error}"
                ),
            };
        }
        return FreshnessVerdict {
            state: "stale",
            message: format!("watcher last failed: {error}"),
        };
    }

    if scan.pending_files > 0 && state.last_run_at.is_none() {
        return FreshnessVerdict {
            state: "watcher-not-running",
            message: format!("watcher has no state; {}", pending_backlog_message(scan)),
        };
    }

    if scan.stable_pending_files > 0 {
        return FreshnessVerdict {
            state: "stale",
            message: pending_backlog_message(scan),
        };
    }

    if !db_exists && scan.pending_files > 0 {
        return FreshnessVerdict {
            state: "stale",
            message: format!("database is missing; {}", pending_backlog_message(scan)),
        };
    }

    if scan.waiting_files > 0 {
        return FreshnessVerdict {
            state: "pending-live-writes",
            message: format!(
                "{} files are still within the quiet window",
                scan.waiting_files
            ),
        };
    }

    if !scan.missing_sources.is_empty() {
        return FreshnessVerdict {
            state: "stale",
            message: format!(
                "{} configured sources are missing",
                scan.missing_sources.len()
            ),
        };
    }

    FreshnessVerdict {
        state: "fresh",
        message: "index is current".to_owned(),
    }
}

fn pending_backlog_message(scan: &SourceScanReport) -> String {
    match (scan.stable_pending_files, scan.waiting_files) {
        (stable, waiting) if stable > 0 && waiting > 0 => format!(
            "{stable} stable files are ready to index; {waiting} files are still within the quiet window"
        ),
        (stable, _) if stable > 0 => format!("{stable} stable files are ready to index"),
        (_, waiting) if waiting > 0 => {
            format!("{waiting} files are still within the quiet window")
        }
        _ => "index is current".to_owned(),
    }
}

fn format_filters(filters: &IndexFilters) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(repo) = filters.repo() {
        parts.push(format!("repo={repo}"));
    }
    if let Some(since) = filters.since_value() {
        parts.push(format!("since={since}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn stale_index_lock_message(error: &anyhow::Error, attempts: usize) -> String {
    format!("database is locked; using stale index after {attempts} refresh attempts: {error:#}")
}

fn lock_retry_delays() -> Vec<Duration> {
    let retries = std::env::var("AGENT_RECALL_WATCH_LOCK_RETRIES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2);
    let base_ms = std::env::var("AGENT_RECALL_WATCH_LOCK_RETRY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(250);

    (0..retries)
        .map(|attempt| {
            let multiplier = 1_u64.checked_shl(attempt.min(16) as u32).unwrap_or(1);
            Duration::from_millis(base_ms.saturating_mul(multiplier))
        })
        .collect()
}

fn is_database_lock_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(sqlite_error) = cause.downcast_ref::<SqliteError>() {
            return matches!(
                sqlite_error,
                SqliteError::SqliteFailure(db_error, _)
                    if matches!(db_error.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
            );
        }
        is_database_lock_text(&cause.to_string())
    })
}

fn is_database_lock_text(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("database is locked")
        || lower.contains("database table is locked")
        || lower.contains("database is busy")
}

fn launch_agent_status(label: String, path: PathBuf) -> LaunchAgentStatus {
    let supported = launch_agent_supported();
    let installed = supported && path.exists();
    let running = if installed {
        Some(is_launch_agent_running(&label))
    } else {
        None
    };

    LaunchAgentStatus {
        label,
        path,
        supported,
        installed,
        running,
    }
}

fn is_launch_agent_running(label: &str) -> bool {
    let Ok(domain) = launch_agent_domain() else {
        return false;
    };
    ProcessCommand::new(launchctl_executable())
        .args(["print", &format!("{domain}/{label}")])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn start_launch_agent(label: &str, path: &Path) -> Result<()> {
    if !launch_agent_supported() {
        return Err(anyhow!("launchctl integration is only supported on macOS"));
    }
    let domain = launch_agent_domain()?;
    let bootstrap = ProcessCommand::new(launchctl_executable())
        .args(["bootstrap", &domain])
        .arg(path)
        .output()
        .context("run launchctl bootstrap")?;

    if !bootstrap.status.success()
        && !launchctl_already_loaded(&bootstrap)
        && !is_launch_agent_running(label)
    {
        return Err(anyhow!(
            "launchctl bootstrap failed: {}{}",
            String::from_utf8_lossy(&bootstrap.stdout),
            String::from_utf8_lossy(&bootstrap.stderr)
        ));
    }

    if !bootstrap.status.success() {
        kickstart_launch_agent(label)?;
    }

    let print = ProcessCommand::new(launchctl_executable())
        .args(["print", &format!("{domain}/{label}")])
        .output()
        .context("run launchctl print")?;
    if !print.status.success() {
        return Err(anyhow!(
            "launchctl print failed after bootstrap: {}{}",
            String::from_utf8_lossy(&print.stdout),
            String::from_utf8_lossy(&print.stderr)
        ));
    }

    Ok(())
}

fn kickstart_launch_agent(label: &str) -> Result<()> {
    let domain = launch_agent_domain()?;
    let kickstart = ProcessCommand::new(launchctl_executable())
        .args(["kickstart", "-k", &format!("{domain}/{label}")])
        .output()
        .context("run launchctl kickstart")?;
    if !kickstart.status.success() {
        return Err(anyhow!(
            "launchctl kickstart failed: {}{}",
            String::from_utf8_lossy(&kickstart.stdout),
            String::from_utf8_lossy(&kickstart.stderr)
        ));
    }
    Ok(())
}

fn retire_legacy_launch_agents(active_label: &str, active_path: &Path) -> Result<Vec<String>> {
    if active_label != DEFAULT_LAUNCH_AGENT_LABEL || !launch_agent_supported() {
        return Ok(Vec::new());
    }

    let mut retired = Vec::new();
    for legacy_label in LEGACY_LAUNCH_AGENT_LABELS {
        if *legacy_label == active_label {
            continue;
        }
        let legacy_path = default_launch_agent_path(legacy_label)?;
        if legacy_path == active_path {
            continue;
        }

        let mut changed = false;
        if is_launch_agent_running(legacy_label) {
            bootout_launch_agent(legacy_label)?;
            changed = true;
        }
        if legacy_path.exists() {
            fs::remove_file(&legacy_path)
                .with_context(|| format!("remove legacy launch agent {}", legacy_path.display()))?;
            changed = true;
        }
        if changed {
            retired.push((*legacy_label).to_owned());
        }
    }

    Ok(retired)
}

fn bootout_launch_agent(label: &str) -> Result<()> {
    let domain = launch_agent_domain()?;
    let output = ProcessCommand::new(launchctl_executable())
        .args(["bootout", &format!("{domain}/{label}")])
        .output()
        .context("run launchctl bootout")?;
    if output.status.success() || launchctl_not_loaded(&output) {
        return Ok(());
    }

    Err(anyhow!(
        "launchctl bootout failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn launchctl_already_loaded(output: &std::process::Output) -> bool {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    text.contains("already") && (text.contains("loaded") || text.contains("bootstrapped"))
}

fn launchctl_not_loaded(output: &std::process::Output) -> bool {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    text.contains("could not find service")
        || text.contains("no such process")
        || text.contains("not loaded")
}

fn launch_agent_domain() -> Result<String> {
    if let Ok(uid) = std::env::var("AGENT_RECALL_UID") {
        return Ok(format!("gui/{uid}"));
    }
    if let Ok(uid) = std::env::var("UID") {
        return Ok(format!("gui/{uid}"));
    }

    let output = ProcessCommand::new("id")
        .arg("-u")
        .output()
        .context("run id -u")?;
    if !output.status.success() {
        return Err(anyhow!("id -u failed"));
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

fn launchctl_executable() -> OsString {
    std::env::var_os("AGENT_RECALL_LAUNCHCTL").unwrap_or_else(|| OsString::from("launchctl"))
}

fn launch_agent_supported() -> bool {
    cfg!(target_os = "macos")
}

fn scan_status(
    db_path: &Path,
    sources: &[PathBuf],
    quiet_for: Duration,
    filters: &IndexFilters,
) -> Result<(SourceScanReport, Option<String>)> {
    if !db_path.exists() {
        return Ok((
            scan_sources_for_pending_with_filters(None, sources, quiet_for, filters)?,
            None,
        ));
    }

    let store = Store::open_readonly(db_path)?;
    let last_indexed_at = store.last_indexed_at()?;
    let scan = scan_sources_for_pending_with_filters(Some(&store), sources, quiet_for, filters)?;
    Ok((scan, last_indexed_at))
}

fn read_watch_state(path: &Path) -> Result<WatchState> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WatchState::default());
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn write_watch_state(path: &Path, state: &WatchState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(state)?;
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn default_launch_agent_path(label: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{label}.plist")))
}

fn install_launch_agent_plist(
    path: &Path,
    label: &str,
    db_path: &Path,
    state_path: &Path,
    sources: &[PathBuf],
    watch: LaunchAgentWatchConfig<'_>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    let executable = std::env::current_exe().context("resolve current executable")?;
    let stdout_path = state_path.with_extension("out.log");
    let stderr_path = state_path.with_extension("err.log");
    let mut args = vec![
        executable.display().to_string(),
        "watch".to_owned(),
        "--db".to_owned(),
        db_path.display().to_string(),
        "--state".to_owned(),
        state_path.display().to_string(),
        "--interval".to_owned(),
        watch.interval.as_secs().to_string(),
        "--quiet-for".to_owned(),
        watch.quiet_for.as_secs().to_string(),
    ];
    for source in sources {
        args.push("--source".to_owned());
        args.push(source.display().to_string());
    }
    args.extend(watch.filters.cli_args());

    let program_arguments = args
        .iter()
        .map(|arg| format!("    <string>{}</string>", xml_escape(arg)))
        .collect::<Vec<_>>()
        .join("\n");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
{}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(label),
        program_arguments,
        xml_escape(&stdout_path.display().to_string()),
        xml_escape(&stderr_path.display().to_string())
    );
    fs::write(path, plist).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
