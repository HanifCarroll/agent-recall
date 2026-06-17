use crate::config::{default_codex_log_sources, default_db_path, default_source_roots};
use crate::log_importer::{
    collect_codex_jsonl_session_ids, index_codex_log_sources_with_progress, LogImportOptions,
};
use crate::output::log_import_progress_line;
use crate::refresh_lock::{acquire_refresh_lock, refresh_lock_wait_timeout};
use crate::store::Store;
use anyhow::{anyhow, Result};
use clap::Args;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Args)]
pub struct IndexLogsArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(
        long = "source",
        help = "Archived Codex logs_*.sqlite database to import; repeatable"
    )]
    pub sources: Vec<PathBuf>,
    #[arg(long, help = "Re-import unchanged log databases")]
    pub force: bool,
    #[arg(
        long,
        help = "Import log threads even when a matching Codex JSONL session exists"
    )]
    pub include_duplicates: bool,
}

pub fn run_index_logs(args: IndexLogsArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let sources = resolve_log_sources(args.sources)?;
    let known_jsonl_session_ids = if args.include_duplicates {
        HashSet::new()
    } else {
        collect_codex_jsonl_session_ids(&codex_jsonl_source_roots()?)?
    };
    let Some(_refresh_lock) = acquire_refresh_lock(&db_path, refresh_lock_wait_timeout())? else {
        return Err(anyhow!("another agent-recall refresh is already active"));
    };
    let store = Store::open(&db_path)?;
    let started = Instant::now();
    let report = index_codex_log_sources_with_progress(
        &store,
        &sources,
        &LogImportOptions {
            force: args.force,
            known_jsonl_session_ids,
        },
        |progress| {
            eprintln!("{}", log_import_progress_line(progress, started.elapsed()));
        },
    )?;

    println!(
        "indexed {} codex-log sessions, {} events from {}/{} log databases ({} current, {} missing, {} duplicate threads skipped, {} stale sessions deleted) into {}",
        report.sessions_indexed,
        report.events_indexed,
        report.sources_indexed,
        report.sources_total,
        report.skipped_current,
        report.skipped_missing,
        report.skipped_duplicate_threads,
        report.sessions_deleted,
        db_path.display()
    );
    Ok(())
}

fn resolve_log_sources(sources: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if sources.is_empty() {
        default_codex_log_sources()
    } else {
        Ok(sources)
    }
}

fn codex_jsonl_source_roots() -> Result<Vec<PathBuf>> {
    Ok(default_source_roots()?.into_iter().take(2).collect())
}
