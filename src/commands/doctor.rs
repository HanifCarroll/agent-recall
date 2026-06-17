use crate::commands::index::resolve_sources;
use crate::commands::watch::{build_status_report, status_json};
use crate::config::{default_db_path, default_state_path, DEFAULT_LAUNCH_AGENT_LABEL};
use crate::store::Store;
use anyhow::Result;
use clap::Args;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Args)]
pub struct DoctorArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
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
    #[arg(
        long,
        default_value = DEFAULT_LAUNCH_AGENT_LABEL,
        help = "LaunchAgent label"
    )]
    pub agent_label: String,
    #[arg(long, help = "LaunchAgent plist path")]
    pub agent_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct StatsArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
}

pub fn run_doctor(args: DoctorArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let state_path = args.state.unwrap_or(default_state_path()?);
    let quiet_for = Duration::from_secs(args.quiet_for);
    let agent_path = args
        .agent_path
        .unwrap_or(default_launch_agent_path(&args.agent_label)?);
    let db_existed = db_path.exists();
    let sources = resolve_sources(args.sources)?;
    let filters = crate::indexer::IndexFilters::new(args.repo, args.since)?;
    let status_report = build_status_report(
        db_path.clone(),
        state_path,
        sources.clone(),
        filters,
        quiet_for,
        args.agent_label,
        agent_path,
    )?;
    let source_status = sources
        .iter()
        .map(|source| {
            json!({
                "path": source,
                "exists": source.exists(),
            })
        })
        .collect::<Vec<_>>();

    if !db_existed {
        if args.json {
            let value = json!({
                "ok": false,
                "db_path": db_path,
                "db_existed": false,
                "db_exists": false,
                "checks": {
                    "quick_check": "missing",
                    "fts_integrity": "missing",
                },
                "stats": {
                    "sessions": 0,
                    "events": 0,
                    "source_files": 0,
                    "duplicate_source_files": 0,
                },
                "freshness": {
                    "state": status_report.freshness.state,
                    "message": status_report.freshness.message,
                    "status": status_json(&status_report),
                },
                "sources": source_status,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }

        println!("database: {} (missing)", db_path.display());
        println!("quick_check: missing");
        println!("fts_integrity: missing");
        println!("stats: 0 sessions, 0 events, 0 source files, 0 duplicate source files");
        for source in sources {
            let status = if source.exists() { "exists" } else { "missing" };
            println!("source: {} ({status})", source.display());
        }
        println!("next: run agent-recall index");
        return Ok(());
    }

    let store = Store::open_readonly(&db_path)?;
    let stats = store.stats()?;
    let quick_check = match store.quick_check() {
        Ok(value) => value,
        Err(error) => format!("error: {error:#}"),
    };
    let fts_integrity = match store.fts_read_check() {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("error: {error:#}"),
    };
    let ok = quick_check == "ok" && fts_integrity == "ok";

    if args.json {
        let value = json!({
            "ok": ok,
            "db_path": db_path,
            "db_existed": db_existed,
            "db_exists": db_existed,
            "checks": {
                "quick_check": quick_check,
                "fts_integrity": fts_integrity,
            },
            "stats": {
                "sessions": stats.session_count,
                "events": stats.event_count,
                "source_files": stats.source_file_count,
                "duplicate_source_files": stats.duplicate_source_file_count,
            },
            "freshness": {
                "state": status_report.freshness.state,
                "message": status_report.freshness.message,
                "status": status_json(&status_report),
            },
            "sources": source_status,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    println!("database: {}", db_path.display());
    println!("quick_check: {quick_check}");
    println!("fts_integrity: {fts_integrity}");
    println!(
        "stats: {} sessions, {} events, {} source files, {} duplicate source files",
        stats.session_count,
        stats.event_count,
        stats.source_file_count,
        stats.duplicate_source_file_count
    );
    println!(
        "freshness: {} ({})",
        status_report.freshness.state, status_report.freshness.message
    );
    for source in sources {
        let status = if source.exists() { "exists" } else { "missing" };
        println!("source: {} ({status})", source.display());
    }
    Ok(())
}

fn default_launch_agent_path(label: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{label}.plist")))
}

pub fn run_stats(args: StatsArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let stats = Store::open_readonly(db_path)?.stats()?;
    println!(
        "{} sessions, {} events, {} source files, {} duplicate source files",
        stats.session_count,
        stats.event_count,
        stats.source_file_count,
        stats.duplicate_source_file_count
    );
    Ok(())
}
