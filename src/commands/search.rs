use crate::commands::date::resolve_date_window;
use crate::commands::exclude::resolve_excluded_sessions;
use crate::commands::kind::{event_kinds, KindArg};
use crate::config::default_db_path;
use crate::output::{compact_whitespace, now_timestamp, preview, shell_quote};
use crate::store::{
    source_priority_for_path, SearchMode, SearchOptions, SearchResult, SearchTrace, Store,
};
use anyhow::{bail, Result};
use clap::Args;
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    #[arg(help = "Search terms")]
    pub query: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = 10, help = "Maximum sessions to print")]
    pub limit: usize,
    #[arg(long, help = "Restrict matches to a repo name")]
    pub repo: Option<String>,
    #[arg(long, help = "Search across all repos instead of auto-filtering")]
    pub all_repos: bool,
    #[arg(long, help = "Restrict matches to sessions under this cwd")]
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
        long,
        help = "Match the exact token phrase instead of AND-ing terms",
        conflicts_with = "near"
    )]
    pub phrase: bool,
    #[arg(
        long,
        value_name = "DISTANCE",
        value_parser = clap::value_parser!(u32).range(1..),
        help = "Match query terms within this FTS NEAR distance",
        conflicts_with = "phrase"
    )]
    pub near: Option<u32>,
    #[arg(
        long = "kind",
        value_enum,
        value_name = "KIND",
        help = "Restrict matches by event kind; repeatable"
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
    #[arg(long, help = "Include retrieval trace fields in JSON output")]
    pub trace: bool,
}

#[derive(Debug, Clone, Args)]
pub struct BundleArgs {
    #[arg(help = "Search terms")]
    pub query: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = 5, help = "Maximum sessions to include")]
    pub limit: usize,
    #[arg(long, help = "Restrict matches to a repo name")]
    pub repo: Option<String>,
    #[arg(long, help = "Search across all repos instead of auto-filtering")]
    pub all_repos: bool,
    #[arg(long, help = "Restrict matches to sessions under this cwd")]
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
        long,
        help = "Match the exact token phrase instead of AND-ing terms",
        conflicts_with = "near"
    )]
    pub phrase: bool,
    #[arg(
        long,
        value_name = "DISTANCE",
        value_parser = clap::value_parser!(u32).range(1..),
        help = "Match query terms within this FTS NEAR distance",
        conflicts_with = "phrase"
    )]
    pub near: Option<u32>,
    #[arg(
        long = "kind",
        value_enum,
        value_name = "KIND",
        help = "Restrict matches by event kind; repeatable"
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
}

#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    #[arg(help = "Session id or session key")]
    pub session_ref: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = 80, help = "Maximum events to print")]
    pub limit: usize,
    #[arg(
        long = "kind",
        value_enum,
        value_name = "KIND",
        help = "Restrict events by kind; repeatable"
    )]
    pub kinds: Vec<KindArg>,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

pub fn run_search(args: SearchArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let (since, from, until) = resolve_date_window(args.since, args.from, args.until, args.day)?;
    let exclude_sessions = resolve_excluded_sessions(args.exclude_sessions, args.exclude_current)?;
    let kinds = event_kinds(&args.kinds);
    let mode = resolve_search_mode(args.phrase, args.near);
    let current_repo = if args.repo.is_none() && !args.all_repos {
        detect_current_repo()
    } else {
        None
    };
    let search_limit = if args.json {
        args.limit
    } else {
        args.limit.saturating_mul(5)
    };
    let (trace, results) = store.search_with_trace(SearchOptions {
        query: args.query.clone(),
        limit: search_limit,
        repo: args.repo,
        cwd: args.cwd,
        since,
        from,
        until,
        include_duplicates: args.include_duplicates,
        exclude_sessions,
        kinds,
        current_repo,
        mode,
    })?;
    if args.json {
        print_search_json(&args.query, &results, &trace, args.trace)?;
        return Ok(());
    }

    if results.is_empty() {
        println!("no matches");
        return Ok(());
    }

    print_grouped_search_results(&results, args.limit);
    Ok(())
}

pub fn run_bundle(args: BundleArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let (since, from, until) = resolve_date_window(args.since, args.from, args.until, args.day)?;
    let exclude_sessions = resolve_excluded_sessions(args.exclude_sessions, args.exclude_current)?;
    let kinds = event_kinds(&args.kinds);
    let mode = resolve_search_mode(args.phrase, args.near);
    let current_repo = if args.repo.is_none() && !args.all_repos {
        detect_current_repo()
    } else {
        None
    };
    let results = store.search_with_options(SearchOptions {
        query: args.query.clone(),
        limit: args.limit.saturating_mul(5).max(args.limit),
        repo: args.repo.clone(),
        cwd: args.cwd.clone(),
        since: since.clone(),
        from: from.clone(),
        until: until.clone(),
        include_duplicates: args.include_duplicates,
        exclude_sessions: exclude_sessions.clone(),
        kinds: kinds.clone(),
        current_repo,
        mode,
    })?;

    let filters = BundleFilters {
        repo: &args.repo,
        cwd: &args.cwd,
        since: &since,
        from: &from,
        until: &until,
        phrase: args.phrase,
        near: args.near,
        kinds: &args.kinds,
        include_duplicates: args.include_duplicates,
        exclude_sessions: &exclude_sessions,
    };
    print_bundle(&args.query, &db_path, args.limit, filters, &results);
    Ok(())
}

pub fn run_show(args: ShowArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let matches = store.resolve_session_reference(&args.session_ref)?;
    if matches.is_empty() {
        println!("no indexed events for {}", args.session_ref);
        return Ok(());
    }
    if matches.len() > 1 {
        let choices = matches
            .iter()
            .map(|session| {
                format!(
                    "  {}  {}  {}",
                    session.session_key,
                    session.cwd,
                    session.source_file_path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "multiple indexed sessions match `{}`; use one session_key:\n{choices}",
            args.session_ref
        );
    }

    let session = &matches[0];
    let kinds = event_kinds(&args.kinds);
    let events = store.session_events_with_kinds(&session.session_key, args.limit, &kinds)?;
    if events.is_empty() {
        println!("no indexed events for {}", args.session_ref);
        return Ok(());
    }
    if args.json {
        print_show_json(session, &events)?;
        return Ok(());
    }

    println!(
        "[{}] {}  {}",
        session.source_kind, session.session_key, session.session_id
    );
    println!("{}", events[0].cwd);
    for event in events {
        println!(
            "\n[{}] {}  {}:{}",
            event.source_kind,
            event.kind.as_str(),
            event.source_file_path.display(),
            event.source_line_number
        );
        println!("{}", preview(&event.text, 900));
    }

    Ok(())
}

fn print_grouped_search_results(results: &[SearchResult], limit: usize) {
    let mut session_order = Vec::<&str>::new();
    for result in results {
        if !session_order
            .iter()
            .any(|session_id| *session_id == result.session_key)
        {
            session_order.push(&result.session_key);
        }
        if session_order.len() == limit {
            break;
        }
    }

    for (index, session_id) in session_order.iter().enumerate() {
        let session_results = results
            .iter()
            .filter(|result| &result.session_key == session_id)
            .collect::<Vec<_>>();
        let first = session_results[0];
        println!(
            "{}. [{}] {}  {}  {}",
            index + 1,
            first.source_kind,
            first.session_key,
            first.session_id,
            first.cwd
        );
        for result in session_results.iter().take(3) {
            println!(
                "   - [{}] {}  {}:{}",
                result.source_kind,
                result.kind.as_str(),
                result.source_file_path.display(),
                result.source_line_number
            );
            println!("     {}", compact_whitespace(&result.snippet));
        }
    }
}

struct BundleFilters<'a> {
    repo: &'a Option<String>,
    cwd: &'a Option<String>,
    since: &'a Option<String>,
    from: &'a Option<String>,
    until: &'a Option<String>,
    phrase: bool,
    near: Option<u32>,
    kinds: &'a [KindArg],
    include_duplicates: bool,
    exclude_sessions: &'a [String],
}

fn print_bundle(
    query: &str,
    db_path: &Path,
    limit: usize,
    filters: BundleFilters<'_>,
    results: &[SearchResult],
) {
    println!("# agent-recall bundle");
    println!();
    println!("Query: {query}");
    println!("Database: {}", db_path.display());
    let filter_labels = bundle_filters(filters);
    if !filter_labels.is_empty() {
        println!("Filters: {}", filter_labels.join(", "));
    }
    println!("Generated: {}", now_timestamp());

    if results.is_empty() {
        println!();
        println!("No matches.");
        return;
    }

    let session_keys = top_session_keys(results, limit);
    println!();
    println!("## Top Sessions");
    for (index, session_key) in session_keys.iter().enumerate() {
        let session_results = results
            .iter()
            .filter(|result| &result.session_key == session_key)
            .collect::<Vec<_>>();
        let first = session_results[0];
        println!(
            "{}. [{}] {}  {}  {}",
            index + 1,
            first.source_kind,
            first.session_key,
            first.session_id,
            first.cwd
        );
        println!("   when: {}", first.session_timestamp);
        println!(
            "   show: agent-recall show {} --limit 120",
            shell_quote(&first.session_key)
        );
        println!("   receipts: {}", session_results.len().min(3));
    }

    println!();
    println!("## Receipts");
    for session_key in &session_keys {
        println!();
        println!("### {session_key}");
        for result in results
            .iter()
            .filter(|result| &result.session_key == session_key)
            .take(3)
        {
            println!(
                "- [{}] {}  {}:{}",
                result.source_kind,
                result.kind.as_str(),
                result.source_file_path.display(),
                result.source_line_number
            );
            println!("  {}", preview(&result.text, 500));
        }
    }

    println!();
    println!("## Next Commands");
    for session_key in session_keys {
        println!(
            "- agent-recall show {} --limit 120",
            shell_quote(session_key)
        );
    }
}

fn bundle_filters(filters: BundleFilters<'_>) -> Vec<String> {
    let mut labels = Vec::new();
    if let Some(repo) = filters.repo {
        labels.push(format!("repo={repo}"));
    }
    if let Some(cwd) = filters.cwd {
        labels.push(format!("cwd={cwd}"));
    }
    if let Some(since) = filters.since {
        labels.push(format!("since={since}"));
    }
    if let Some(from) = filters.from {
        labels.push(format!("from={from}"));
    }
    if let Some(until) = filters.until {
        labels.push(format!("until={until}"));
    }
    if filters.phrase {
        labels.push("phrase=true".to_owned());
    }
    if let Some(distance) = filters.near {
        labels.push(format!("near={distance}"));
    }
    for kind in filters.kinds {
        labels.push(format!("kind={}", kind.as_str()));
    }
    if filters.include_duplicates {
        labels.push("include-duplicates=true".to_owned());
    }
    for excluded_session in filters.exclude_sessions {
        labels.push(format!("exclude-session={excluded_session}"));
    }
    labels
}

fn top_session_keys(results: &[SearchResult], limit: usize) -> Vec<&str> {
    let mut session_keys = Vec::<&str>::new();
    for result in results {
        if !session_keys
            .iter()
            .any(|session_key| *session_key == result.session_key)
        {
            session_keys.push(&result.session_key);
        }
        if session_keys.len() == limit {
            break;
        }
    }
    session_keys
}

fn print_show_json(
    session: &crate::store::SessionMatch,
    events: &[crate::store::SessionEvent],
) -> Result<()> {
    let events = events
        .iter()
        .map(|event| {
            let source = format!(
                "{}:{}",
                event.source_file_path.display(),
                event.source_line_number
            );
            json!({
                "kind": event.kind.as_str(),
                "source_kind": event.source_kind,
                "source_label": event.source_label,
                "text": event.text,
                "cwd": event.cwd,
                "source_file_path": event.source_file_path,
                "source_line_number": event.source_line_number,
                "source": source,
                "source_timestamp": event.source_timestamp,
            })
        })
        .collect::<Vec<_>>();

    let value = json!({
        "session_key": session.session_key,
        "session_id": session.session_id,
        "repo": session.repo,
        "cwd": session.cwd,
        "source_file_path": session.source_file_path,
        "count": events.len(),
        "events": events,
        "source_kind": session.source_kind,
        "source_label": session.source_label,
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn print_search_json(
    query: &str,
    results: &[SearchResult],
    trace: &SearchTrace,
    include_trace: bool,
) -> Result<()> {
    let results = results
        .iter()
        .map(|result| {
            let source = format!(
                "{}:{}",
                result.source_file_path.display(),
                result.source_line_number
            );
            let mut value = json!({
                "session_key": result.session_key,
                "session_id": result.session_id,
                "repo": result.repo,
                "source_kind": result.source_kind,
                "source_label": result.source_label,
                "kind": result.kind.as_str(),
                "cwd": result.cwd,
                "session_timestamp": result.session_timestamp,
                "source_file_path": result.source_file_path,
                "source_line_number": result.source_line_number,
                "source": source,
                "source_timestamp": result.source_timestamp,
                "score": result.score,
                "snippet": compact_whitespace(&result.snippet),
                "text_preview": preview(&result.text, 500),
            });
            if include_trace {
                value["trace"] = json!({
                    "match_strategy": trace.match_strategy.as_str(),
                    "repo_matches_current": result.repo_matches_current,
                    "session_hit_count": result.session_hit_count,
                    "best_kind_weight": result.best_kind_weight,
                    "fts_score": result.score,
                    "source_priority": source_priority_for_path(&result.source_file_path),
                    "duplicate_session_id": result.session_id.as_str(),
                });
            }
            value
        })
        .collect::<Vec<_>>();

    let mut value = json!({
        "query": query,
        "match_strategy": trace.match_strategy.as_str(),
        "count": results.len(),
        "results": results,
    });
    if include_trace {
        value["trace"] = json!({
            "match_strategy": trace.match_strategy.as_str(),
            "query_terms": &trace.query_terms,
            "fts_query": trace.fts_query.as_str(),
            "fetch_limit": trace.fetch_limit,
            "current_repo": &trace.current_repo,
            "include_duplicates": trace.include_duplicates,
        });
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn detect_current_repo() -> Option<String> {
    let mut path = std::env::current_dir().ok()?;
    loop {
        if path.join(".git").exists() {
            return path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
        }
        if !path.pop() {
            return None;
        }
    }
}

fn resolve_search_mode(phrase: bool, near: Option<u32>) -> SearchMode {
    if phrase {
        SearchMode::Phrase
    } else if let Some(distance) = near {
        SearchMode::Near(distance)
    } else {
        SearchMode::AllTerms
    }
}
