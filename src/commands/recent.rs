use crate::commands::date::resolve_date_window;
use crate::commands::exclude::resolve_excluded_sessions;
use crate::commands::kind::{event_kinds, KindArg};
use crate::config::default_db_path;
use crate::output::shell_quote;
use crate::store::{RecentOptions, RecentSession, Store};
use anyhow::Result;
use clap::Args;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Args)]
pub struct RecentArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = 20, help = "Maximum sessions to print")]
    pub limit: usize,
    #[arg(long, help = "Restrict sessions to a repo name")]
    pub repo: Option<String>,
    #[arg(
        long,
        help = "Accepted for parity with search/bundle; recent already spans all repos unless --repo is set"
    )]
    pub all_repos: bool,
    #[arg(long, help = "Restrict sessions to a cwd substring")]
    pub cwd: Option<String>,
    #[arg(long, help = "Restrict by age, for example 7d, today, or 2026-04-01")]
    pub since: Option<String>,
    #[arg(long, help = "Restrict to sessions at or after this date/time")]
    pub from: Option<String>,
    #[arg(long, help = "Restrict to sessions before this date/time")]
    pub until: Option<String>,
    #[arg(long, help = "Restrict to one local calendar day, YYYY-MM-DD")]
    pub day: Option<String>,
    #[arg(
        long = "kind",
        value_enum,
        value_name = "KIND",
        help = "Restrict sessions by contained event kind; repeatable"
    )]
    pub kinds: Vec<KindArg>,
    #[arg(long, help = "Include duplicate active/archive copies")]
    pub include_duplicates: bool,
    #[arg(
        long = "exclude-session",
        help = "Exclude a session id or session key; repeatable"
    )]
    pub exclude_sessions: Vec<String>,
    #[arg(long, help = "Exclude the current agent session from results")]
    pub exclude_current: bool,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DayArgs {
    #[arg(help = "Local calendar day as YYYY-MM-DD")]
    pub day: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = 100, help = "Maximum sessions to include")]
    pub limit: usize,
    #[arg(long, help = "Restrict sessions to a repo name")]
    pub repo: Option<String>,
    #[arg(long, help = "Restrict sessions to a cwd substring")]
    pub cwd: Option<String>,
    #[arg(
        long = "kind",
        value_enum,
        value_name = "KIND",
        help = "Restrict sessions by contained event kind; repeatable"
    )]
    pub kinds: Vec<KindArg>,
    #[arg(long, help = "Include duplicate active/archive copies")]
    pub include_duplicates: bool,
    #[arg(
        long = "exclude-session",
        help = "Exclude a session id or session key; repeatable"
    )]
    pub exclude_sessions: Vec<String>,
    #[arg(long, help = "Exclude the current agent session from results")]
    pub exclude_current: bool,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

pub fn run_recent(args: RecentArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let (since, from, until) = resolve_date_window(args.since, args.from, args.until, args.day)?;
    let exclude_sessions = resolve_excluded_sessions(args.exclude_sessions, args.exclude_current)?;
    let sessions = store.recent_sessions(RecentOptions {
        limit: args.limit,
        repo: args.repo,
        cwd: args.cwd,
        since,
        from,
        until,
        include_duplicates: args.include_duplicates,
        exclude_sessions,
        kinds: event_kinds(&args.kinds),
    })?;

    if args.json {
        print_recent_json(&sessions)?;
        return Ok(());
    }

    if sessions.is_empty() {
        println!("no recent sessions");
        return Ok(());
    }

    print_recent_sessions(&sessions);
    Ok(())
}

pub fn run_day(args: DayArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let (_, from, until) = resolve_date_window(None, None, None, Some(args.day.clone()))?;
    let exclude_sessions = resolve_excluded_sessions(args.exclude_sessions, args.exclude_current)?;
    let sessions = store.recent_sessions(RecentOptions {
        limit: args.limit,
        repo: args.repo,
        cwd: args.cwd,
        since: None,
        from: from.clone(),
        until: until.clone(),
        include_duplicates: args.include_duplicates,
        exclude_sessions,
        kinds: event_kinds(&args.kinds),
    })?;

    if args.json {
        print_day_json(&args.day, from.as_deref(), until.as_deref(), &sessions)?;
        return Ok(());
    }

    if sessions.is_empty() {
        println!("no sessions for {}", args.day);
        return Ok(());
    }

    println!("{} sessions for {}", sessions.len(), args.day);
    print_recent_sessions(&sessions);
    Ok(())
}

pub(crate) fn print_recent_sessions(sessions: &[RecentSession]) {
    for (index, session) in sessions.iter().enumerate() {
        println!(
            "{}. [{}] {}  {}  {}",
            index + 1,
            session.source_kind,
            session.session_key,
            session.session_id,
            session.repo
        );
        println!("   when: {}", session.session_timestamp);
        println!("   cwd: {}", session.cwd);
        println!(
            "   source: {} {}",
            session.source_kind,
            session.source_file_path.display()
        );
        println!(
            "   show: agent-recall show {} --limit 120",
            shell_quote(&session.session_key)
        );
    }
}

fn print_recent_json(sessions: &[RecentSession]) -> Result<()> {
    let sessions_json = sessions_json(sessions);
    let value = json!({
        "count": sessions_json.len(),
        "sessions": sessions_json,
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn print_day_json(
    day: &str,
    from: Option<&str>,
    until: Option<&str>,
    sessions: &[RecentSession],
) -> Result<()> {
    let repo_counts = count_by(sessions.iter().map(|session| session.repo.as_str()));
    let cwd_counts = count_by(sessions.iter().map(|session| session.cwd.as_str()));
    let sessions_json = sessions_json(sessions);
    let value = json!({
        "day": day,
        "from": from,
        "until": until,
        "count": sessions_json.len(),
        "repo_counts": repo_counts,
        "cwd_counts": cwd_counts,
        "sessions": sessions_json,
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn sessions_json(sessions: &[RecentSession]) -> Vec<serde_json::Value> {
    sessions
        .iter()
        .map(|session| {
            json!({
                "session_key": session.session_key,
                "session_id": session.session_id,
                "repo": session.repo,
                "source_kind": session.source_kind,
                "source_label": session.source_label,
                "cwd": session.cwd,
                "session_timestamp": session.session_timestamp,
                "source_file_path": session.source_file_path,
                "show_command": format!("agent-recall show {} --limit 120", shell_quote(&session.session_key)),
            })
        })
        .collect()
}

fn count_by<'a>(values: impl Iterator<Item = &'a str>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value.to_owned()).or_insert(0) += 1;
    }
    counts
}
