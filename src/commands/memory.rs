use crate::commands::date::resolve_date_window;
use crate::config::default_db_path;
use crate::memory::MemoryKind;
use crate::output::{compact_whitespace, preview};
use crate::store::{
    encode_delta_cursor, DeltaItem, MatchStrategy, MemoryEvidence, MemoryObject, MemoryResult,
    MemorySearchOptions, RecentSession, ResourceRecord, SearchOptions, Store,
};
use anyhow::{bail, Result};
use clap::{Args, ValueEnum};
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MemoryKindArg {
    Decision,
    Task,
    Fact,
    OpenQuestion,
    Blocker,
}

impl MemoryKindArg {
    fn memory_kind(self) -> MemoryKind {
        match self {
            Self::Decision => MemoryKind::Decision,
            Self::Task => MemoryKind::Task,
            Self::Fact => MemoryKind::Fact,
            Self::OpenQuestion => MemoryKind::OpenQuestion,
            Self::Blocker => MemoryKind::Blocker,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ResourceKindArg {
    Session,
    Memory,
    All,
}

#[derive(Debug, Clone, Args)]
pub struct MemoriesArgs {
    #[arg(help = "Optional query over extracted memory objects")]
    pub query: Option<String>,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = 20, help = "Maximum memory objects to print")]
    pub limit: usize,
    #[arg(long, help = "Restrict memories to a repo name")]
    pub repo: Option<String>,
    #[arg(long, help = "Restrict memories to a cwd substring")]
    pub cwd: Option<String>,
    #[arg(long, help = "Restrict by age, for example 7d, today, or 2026-04-01")]
    pub since: Option<String>,
    #[arg(long, help = "Restrict to memories seen at or after this date/time")]
    pub from: Option<String>,
    #[arg(long, help = "Restrict to memories seen before this date/time")]
    pub until: Option<String>,
    #[arg(long, help = "Restrict to one local calendar day, YYYY-MM-DD")]
    pub day: Option<String>,
    #[arg(
        long = "kind",
        value_enum,
        value_name = "KIND",
        help = "Restrict memories by kind; repeatable"
    )]
    pub kinds: Vec<MemoryKindArg>,
    #[arg(long, help = "Include retrieval trace fields in JSON output")]
    pub trace: bool,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct MemoryShowArgs {
    #[arg(help = "Memory object id")]
    pub memory_id: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = 20, help = "Maximum evidence rows to print")]
    pub limit: usize,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct DeltaArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, help = "Opaque cursor returned by a previous delta call")]
    pub cursor: Option<String>,
    #[arg(long, default_value_t = 50, help = "Maximum changed items to return")]
    pub limit: usize,
    #[arg(long, help = "Restrict changed items to a repo name")]
    pub repo: Option<String>,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct RelatedArgs {
    #[arg(help = "Session id/session key or memory id")]
    pub reference: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(
        long,
        default_value_t = 10,
        help = "Maximum related sessions and memories"
    )]
    pub limit: usize,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct EvalArgs {
    #[arg(help = "Path to a JSON eval fixture")]
    pub fixture: PathBuf,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ResourcesArgs {
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        default_value = "all",
        help = "Resource type to list"
    )]
    pub kind: ResourceKindArg,
    #[arg(long, default_value_t = 20, help = "Maximum resources to print")]
    pub limit: usize,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ReadResourceArgs {
    #[arg(help = "Resource URI to read")]
    pub uri: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct EvalFixture {
    cases: Vec<EvalCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    name: String,
    command: String,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    expected: EvalExpected,
}

#[derive(Debug, Default, Deserialize)]
struct EvalExpected {
    session_id: Option<String>,
    top_memory_kind: Option<String>,
    top_memory_summary_contains: Option<String>,
    contains_session_id: Option<String>,
    contains_memory_kind: Option<String>,
    next_cursor_present: Option<bool>,
    kind: Option<String>,
    summary_contains: Option<String>,
}

pub fn run_memories(args: MemoriesArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let (since, from, until) = resolve_date_window(args.since, args.from, args.until, args.day)?;
    let options = MemorySearchOptions {
        query: args.query.clone(),
        limit: args.limit,
        repo: args.repo,
        cwd: args.cwd,
        since,
        from,
        until,
        kinds: args.kinds.iter().map(|kind| kind.memory_kind()).collect(),
    };
    let (match_strategy, memories) = store.memory_results_with_trace(options)?;
    if args.json {
        print_memories_json(&memories, match_strategy, args.trace)?;
        return Ok(());
    }

    if memories.is_empty() {
        println!("no memories");
        return Ok(());
    }

    for (index, memory) in memories.iter().enumerate() {
        println!(
            "{}. {}  {}",
            index + 1,
            memory.object.id,
            memory.object.kind.as_str()
        );
        println!("   summary: {}", memory.object.summary);
        println!("   evidence: {}", memory.object.evidence_count);
        println!("   last_seen: {}", memory.object.last_seen_at);
    }
    Ok(())
}

pub fn run_memory_show(args: MemoryShowArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let Some(memory) = store.memory_by_id(&args.memory_id)? else {
        println!("no memory {}", args.memory_id);
        return Ok(());
    };
    let evidence = store.memory_evidence(&args.memory_id, args.limit)?;
    if args.json {
        print_memory_json(&memory, &evidence)?;
        return Ok(());
    }

    println!("{}  {}", memory.id, memory.kind.as_str());
    println!("{}", memory.summary);
    for item in evidence {
        println!(
            "- {}  {}:{}",
            item.session_id,
            item.source_file_path.display(),
            item.source_line_number
        );
        println!("  {}", preview(&item.evidence_text, 160));
    }
    Ok(())
}

pub fn run_delta(args: DeltaArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let items = store.delta_items(args.cursor.as_deref(), args.limit, args.repo.as_deref())?;
    let next_cursor = items.last().map(encode_delta_cursor);
    if args.json {
        let items = items.iter().map(delta_item_json).collect::<Vec<_>>();
        let value = json!({
            "object": "delta_page",
            "cursor": args.cursor,
            "count": items.len(),
            "next_cursor": next_cursor,
            "items": items,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if items.is_empty() {
        println!("no changed items");
        return Ok(());
    }

    for item in &items {
        match item {
            DeltaItem::Session {
                session_key,
                session_id,
                updated_at,
                ..
            } => println!("session  {session_key}  {session_id}  {updated_at}"),
            DeltaItem::Memory { object, .. } => {
                println!(
                    "memory   {}  {}  {}",
                    object.id,
                    object.kind.as_str(),
                    object.updated_at
                )
            }
            DeltaItem::Deleted {
                object_type,
                object_id,
                updated_at,
                ..
            } => println!("deleted  {object_type}  {object_id}  {updated_at}"),
        }
    }
    if let Some(cursor) = next_cursor {
        println!("next_cursor: {cursor}");
    }
    Ok(())
}

pub fn run_related(args: RelatedArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    if let Some(memory) = store.memory_by_id(&args.reference)? {
        let sessions = store.related_sessions_for_memory(&args.reference, args.limit)?;
        let memories = store.cooccurring_memories(&args.reference, args.limit)?;
        if args.json {
            let value = json!({
                "object": "related_context",
                "reference": memory_json_value(&memory, &[], false, MatchStrategy::AllTerms),
                "sessions": sessions.iter().map(session_json_value).collect::<Vec<_>>(),
                "memories": memories.iter().map(|item| memory_result_json(item, false, MatchStrategy::AllTerms)).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
        println!("related memory {}", args.reference);
        for session in sessions {
            println!("- session {} {}", session.session_key, session.session_id);
        }
        return Ok(());
    }

    let matches = store.resolve_session_reference(&args.reference)?;
    if matches.is_empty() {
        println!("no related context for {}", args.reference);
        return Ok(());
    }
    if matches.len() > 1 {
        let choices = matches
            .iter()
            .map(|session| {
                format!(
                    "  {}  {}",
                    session.session_key,
                    session.source_file_path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "multiple indexed sessions match `{}`; use one session_key:\n{choices}",
            args.reference
        );
    }
    let session = &matches[0];
    let sessions = store.related_sessions_for_session(&session.session_key, args.limit)?;
    let memories = store.related_memories_for_session(&session.session_key, args.limit)?;
    if args.json {
        let value = json!({
            "object": "related_context",
            "reference": {
                "object": "session",
                "session_key": session.session_key,
                "session_id": session.session_id,
                "repo": session.repo,
                "cwd": session.cwd,
                "resource_uri": format!("agent-recall://session/{}", session.session_key),
            },
            "sessions": sessions.iter().map(session_json_value).collect::<Vec<_>>(),
            "memories": memories.iter().map(|item| memory_result_json(item, false, MatchStrategy::AllTerms)).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    println!("related session {}", session.session_key);
    for related in sessions {
        println!("- session {} {}", related.session_key, related.session_id);
    }
    Ok(())
}

pub fn run_eval(args: EvalArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let fixture: EvalFixture = serde_json::from_slice(&fs::read(&args.fixture)?)?;
    let mut passed = 0_u64;
    let mut failed = 0_u64;
    let mut cases_json = Vec::new();

    for case in fixture.cases {
        let (status, details) = match case.command.as_str() {
            "search" => {
                let query = case
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("eval search case `{}` is missing `query`", case.name)
                    })?;
                let (_, results) = store.search_with_trace(SearchOptions::new(
                    query.to_owned(),
                    case.limit.unwrap_or(1),
                ))?;
                let actual = results.first().map(|result| result.session_id.clone());
                if actual == case.expected.session_id {
                    passed += 1;
                    ("pass", actual.unwrap_or_default())
                } else {
                    failed += 1;
                    ("fail", actual.unwrap_or_else(|| "no result".to_owned()))
                }
            }
            "memories" => {
                let query = case
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("eval memories case `{}` is missing `query`", case.name)
                    })?;
                let (_, results) = store.memory_results_with_trace(MemorySearchOptions {
                    query: Some(query.to_owned()),
                    limit: case.limit.unwrap_or(1),
                    ..MemorySearchOptions::default()
                })?;
                let actual = results.first();
                let kind_ok = case
                    .expected
                    .top_memory_kind
                    .as_deref()
                    .or(case.expected.kind.as_deref())
                    .map(|kind| actual.map(|item| item.object.kind.as_str()) == Some(kind))
                    .unwrap_or(true);
                let summary_ok = case
                    .expected
                    .top_memory_summary_contains
                    .as_deref()
                    .or(case.expected.summary_contains.as_deref())
                    .map(|fragment| {
                        actual
                            .map(|item| item.object.summary.contains(fragment))
                            .unwrap_or(false)
                    })
                    .unwrap_or(true);
                if kind_ok && summary_ok {
                    passed += 1;
                    (
                        "pass",
                        actual
                            .map(|item| item.object.id.clone())
                            .unwrap_or_default(),
                    )
                } else {
                    failed += 1;
                    (
                        "fail",
                        actual
                            .map(|item| item.object.summary.clone())
                            .unwrap_or_else(|| "no result".to_owned()),
                    )
                }
            }
            "delta" => {
                let items =
                    store.delta_items(case.cursor.as_deref(), case.limit.unwrap_or(20), None)?;
                let next_cursor = items.last().map(encode_delta_cursor);
                let session_ok = case
                    .expected
                    .contains_session_id
                    .as_deref()
                    .map(|session_id| {
                        items.iter().any(|item| {
                            matches!(
                                item,
                                DeltaItem::Session { session_id: item_session_id, .. }
                                    if item_session_id == session_id
                            )
                        })
                    })
                    .unwrap_or(true);
                let memory_ok = case
                    .expected
                    .contains_memory_kind
                    .as_deref()
                    .map(|kind| {
                        items.iter().any(|item| {
                            matches!(
                                item,
                                DeltaItem::Memory { object, .. }
                                    if object.kind.as_str() == kind
                            )
                        })
                    })
                    .unwrap_or(true);
                let cursor_ok = case
                    .expected
                    .next_cursor_present
                    .map(|expected| next_cursor.is_some() == expected)
                    .unwrap_or(true);

                if session_ok && memory_ok && cursor_ok {
                    passed += 1;
                    (
                        "pass",
                        next_cursor.unwrap_or_else(|| format!("{} items", items.len())),
                    )
                } else {
                    failed += 1;
                    (
                        "fail",
                        format!(
                            "items={} next_cursor={}",
                            items.len(),
                            next_cursor.unwrap_or_default()
                        ),
                    )
                }
            }
            other => {
                failed += 1;
                ("fail", format!("unsupported command `{other}`"))
            }
        };

        cases_json.push(json!({
            "name": case.name,
            "command": case.command,
            "status": status,
            "details": details,
        }));
    }

    if args.json {
        let value = json!({
            "object": "eval_result",
            "passed": passed,
            "failed": failed,
            "cases": cases_json,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    println!("passed: {passed}");
    println!("failed: {failed}");
    Ok(())
}

pub fn run_resources(args: ResourcesArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    let mut resources = Vec::new();
    match args.kind {
        ResourceKindArg::Session => resources.extend(store.session_resources(args.limit)?),
        ResourceKindArg::Memory => resources.extend(store.memory_resources(args.limit)?),
        ResourceKindArg::All => {
            resources.extend(store.memory_resources(args.limit)?);
            resources.extend(store.session_resources(args.limit)?);
            resources.sort_by(|left, right| {
                right
                    .updated_at
                    .cmp(&left.updated_at)
                    .then_with(|| left.uri.cmp(&right.uri))
            });
            resources.truncate(args.limit);
        }
    }

    if args.json {
        let value = json!({
            "object": "resource_list",
            "count": resources.len(),
            "resources": resources.iter().map(resource_json_value).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    for resource in &resources {
        println!("{}  {}", resource.object_type, resource.uri);
    }
    Ok(())
}

pub fn run_read_resource(args: ReadResourceArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let store = Store::open_readonly(&db_path)?;
    if let Some(memory_id) = args.uri.strip_prefix("agent-recall://memory/") {
        let Some(memory) = store.memory_by_id(memory_id)? else {
            bail!("unknown memory resource `{}`", args.uri);
        };
        let evidence = store.memory_evidence(memory_id, 50)?;
        let value = memory_json_value(&memory, &evidence, false, MatchStrategy::AllTerms);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if let Some(session_key) = args.uri.strip_prefix("agent-recall://session/") {
        let matches = store.resolve_session_reference(session_key)?;
        if matches.is_empty() {
            bail!("unknown session resource `{}`", args.uri);
        }
        let session = &matches[0];
        let events = store.session_events(session_key, 120)?;
        let value = json!({
            "object": "session",
            "session_key": session.session_key,
            "session_id": session.session_id,
            "repo": session.repo,
            "cwd": session.cwd,
            "resource_uri": args.uri,
            "events": events.iter().map(|event| json!({
                "kind": event.kind.as_str(),
                "text": event.text,
                "cwd": event.cwd,
                "source_file_path": event.source_file_path,
                "source_line_number": event.source_line_number,
                "source_timestamp": event.source_timestamp,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    bail!("unsupported resource uri `{}`", args.uri)
}

fn print_memories_json(
    memories: &[MemoryResult],
    match_strategy: MatchStrategy,
    include_trace: bool,
) -> Result<()> {
    let value = json!({
        "object": "list",
        "type": "memory",
        "count": memories.len(),
        "match_strategy": match_strategy.as_str(),
        "results": memories
            .iter()
            .map(|memory| memory_result_json(memory, include_trace, match_strategy))
            .collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn print_memory_json(memory: &MemoryObject, evidence: &[MemoryEvidence]) -> Result<()> {
    let value = memory_json_value(memory, evidence, false, MatchStrategy::AllTerms);
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn memory_json_value(
    memory: &MemoryObject,
    evidence: &[MemoryEvidence],
    include_trace: bool,
    match_strategy: MatchStrategy,
) -> serde_json::Value {
    let mut value = json!({
        "object": "memory",
        "id": memory.id,
        "kind": memory.kind.as_str(),
        "summary": memory.summary,
        "normalized_text": memory.normalized_text,
        "first_seen_at": memory.first_seen_at,
        "last_seen_at": memory.last_seen_at,
        "created_at": memory.created_at,
        "updated_at": memory.updated_at,
        "evidence_count": memory.evidence_count,
        "resource_uri": format!("agent-recall://memory/{}", memory.id),
        "evidence": evidence.iter().map(evidence_json_value).collect::<Vec<_>>(),
    });
    if include_trace {
        value["trace"] = json!({
            "match_strategy": match_strategy.as_str(),
            "evidence_count": memory.evidence_count,
        });
    }
    value
}

fn memory_result_json(
    memory: &MemoryResult,
    include_trace: bool,
    match_strategy: MatchStrategy,
) -> serde_json::Value {
    let mut value = json!({
        "object": "memory",
        "id": memory.object.id,
        "kind": memory.object.kind.as_str(),
        "summary": memory.object.summary,
        "normalized_text": memory.object.normalized_text,
        "first_seen_at": memory.object.first_seen_at,
        "last_seen_at": memory.object.last_seen_at,
        "created_at": memory.object.created_at,
        "updated_at": memory.object.updated_at,
        "evidence_count": memory.object.evidence_count,
        "repos": memory.repos,
        "session_keys": memory.session_keys,
        "resource_uri": format!("agent-recall://memory/{}", memory.object.id),
    });
    if include_trace {
        value["trace"] = json!({
            "match_strategy": match_strategy.as_str(),
            "evidence_count": memory.object.evidence_count,
            "repo_count": value["repos"].as_array().map(|repos| repos.len()).unwrap_or_default(),
            "session_count": value["session_keys"].as_array().map(|sessions| sessions.len()).unwrap_or_default(),
        });
    }
    value
}

fn evidence_json_value(evidence: &MemoryEvidence) -> serde_json::Value {
    json!({
        "session_key": evidence.session_key,
        "session_id": evidence.session_id,
        "repo": evidence.repo,
        "cwd": evidence.cwd,
        "session_timestamp": evidence.session_timestamp,
        "source_file_path": evidence.source_file_path,
        "source_line_number": evidence.source_line_number,
        "source_timestamp": evidence.source_timestamp,
        "event_kind": evidence.event_kind.as_str(),
        "evidence_text": compact_whitespace(&evidence.evidence_text),
        "resource_uri": format!("agent-recall://session/{}", evidence.session_key),
    })
}

fn delta_item_json(item: &DeltaItem) -> serde_json::Value {
    match item {
        DeltaItem::Session {
            change_id,
            action,
            session_key,
            session_id,
            repo,
            cwd,
            session_timestamp,
            updated_at,
        } => json!({
            "object": "session",
            "change_id": change_id,
            "change_kind": item.change_kind(),
            "action": action,
            "session_key": session_key,
            "session_id": session_id,
            "repo": repo,
            "cwd": cwd,
            "session_timestamp": session_timestamp,
            "updated_at": updated_at,
            "resource_uri": format!("agent-recall://session/{}", session_key),
        }),
        DeltaItem::Memory {
            change_id,
            action,
            object,
            repos,
            session_keys,
        } => json!({
            "object": "memory",
            "change_id": change_id,
            "change_kind": item.change_kind(),
            "action": action,
            "id": object.id,
            "kind": object.kind.as_str(),
            "summary": object.summary,
            "updated_at": object.updated_at,
            "evidence_count": object.evidence_count,
            "repos": repos,
            "session_keys": session_keys,
            "resource_uri": format!("agent-recall://memory/{}", object.id),
        }),
        DeltaItem::Deleted {
            change_id,
            object_type,
            object_id,
            action,
            updated_at,
        } => json!({
            "object": "deleted",
            "change_id": change_id,
            "change_kind": item.change_kind(),
            "action": action,
            "object_type": object_type,
            "id": object_id,
            "updated_at": updated_at,
        }),
    }
}

fn resource_json_value(resource: &ResourceRecord) -> serde_json::Value {
    json!({
        "uri": resource.uri,
        "name": resource.name,
        "description": resource.description,
        "mime_type": resource.mime_type,
        "object_type": resource.object_type,
        "updated_at": resource.updated_at,
    })
}

fn session_json_value(session: &RecentSession) -> serde_json::Value {
    json!({
        "object": "session",
        "session_key": session.session_key,
        "session_id": session.session_id,
        "repo": session.repo,
        "cwd": session.cwd,
        "session_timestamp": session.session_timestamp,
        "resource_uri": format!("agent-recall://session/{}", session.session_key),
        "source_kind": session.source_kind,
        "source_label": session.source_label,
    })
}
