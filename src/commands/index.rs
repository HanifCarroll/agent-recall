use crate::config::{default_db_path, default_source_roots};
use crate::indexer::{index_sources_with_filters_and_progress, IndexFilters};
use crate::output::progress_line;
use crate::refresh_lock::{acquire_refresh_lock, refresh_lock_wait_timeout};
use crate::store::Store;
use anyhow::{anyhow, Context, Result};
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, Args)]
pub struct IndexArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long = "source", help = "Session archive root to scan; repeatable")]
    pub sources: Vec<PathBuf>,
    #[arg(
        long,
        help = "Only index sessions whose session or command cwd matches this repo"
    )]
    pub repo: Option<String>,
    #[arg(long, help = "Only index sessions at or after this date/time")]
    pub since: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct RebuildArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long = "source", help = "Session archive root to scan; repeatable")]
    pub sources: Vec<PathBuf>,
    #[arg(
        long,
        help = "Only index sessions whose session or command cwd matches this repo"
    )]
    pub repo: Option<String>,
    #[arg(long, help = "Only index sessions at or after this date/time")]
    pub since: Option<String>,
}

pub fn run_index(args: IndexArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let sources = resolve_sources(args.sources)?;
    let filters = IndexFilters::new(args.repo, args.since)?;
    let Some(_refresh_lock) = acquire_refresh_lock(&db_path, refresh_lock_wait_timeout())? else {
        return Err(anyhow!("another agent-recall refresh is already active"));
    };
    let store = Store::open(&db_path)?;
    let started = Instant::now();
    let report = index_sources_with_filters_and_progress(&store, &sources, &filters, |report| {
        eprintln!("{}", progress_line(report, started.elapsed()));
    })?;
    println!(
        "indexed {} session files, {} events from {}/{} files ({} skipped: {} unchanged, {} filtered, {} missing, {} non-session) into {}",
        report.sessions_indexed,
        report.events_indexed,
        report.files_seen,
        report.files_total,
        report.files_skipped,
        report.skipped_unchanged,
        report.skipped_filtered,
        report.skipped_missing,
        report.skipped_non_session,
        db_path.display()
    );
    Ok(())
}

pub fn run_rebuild(args: RebuildArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let sources = resolve_sources(args.sources)?;
    let filters = IndexFilters::new(args.repo, args.since)?;
    let Some(_refresh_lock) = acquire_refresh_lock(&db_path, refresh_lock_wait_timeout())? else {
        return Err(anyhow!("another agent-recall refresh is already active"));
    };
    remove_db_files(&db_path)?;
    let store = Store::open(&db_path)?;
    let started = Instant::now();
    let report = index_sources_with_filters_and_progress(&store, &sources, &filters, |report| {
        eprintln!("{}", progress_line(report, started.elapsed()));
    })?;
    println!(
        "rebuilt {} session files, {} events from {}/{} files ({} skipped: {} unchanged, {} filtered, {} missing, {} non-session) into {}",
        report.sessions_indexed,
        report.events_indexed,
        report.files_seen,
        report.files_total,
        report.files_skipped,
        report.skipped_unchanged,
        report.skipped_filtered,
        report.skipped_missing,
        report.skipped_non_session,
        db_path.display()
    );
    Ok(())
}

pub fn resolve_sources(sources: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if sources.is_empty() {
        default_source_roots()
    } else {
        Ok(sources)
    }
}

fn remove_db_files(db_path: &Path) -> Result<()> {
    for path in [
        db_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", db_path.display())),
        PathBuf::from(format!("{}-shm", db_path.display())),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", path.display()));
            }
        }
    }
    Ok(())
}
