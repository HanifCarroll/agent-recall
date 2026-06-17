use crate::commands::date::{parse_date_filter, DateFilter};
use crate::memory::{extract_memories, ExtractedMemory, MemoryKind};
use crate::parser::{EventKind, ParsedSession};
use anyhow::{bail, Context, Result};
use rusqlite::{params, params_from_iter, Connection, OpenFlags};
use std::collections::{hash_map::Entry, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONTENT_VERSION: i64 = 4;
// Version 4 added source labels and new transcript parsers. Existing v3 Codex
// rows remain searchable and do not need a multi-GB reindex just to gain
// default source labels.
const MIN_COMPATIBLE_CONTENT_VERSION: i64 = 3;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    pub session_count: u64,
    pub event_count: u64,
    pub source_file_count: u64,
    pub duplicate_source_file_count: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub session_key: String,
    pub session_id: String,
    pub repo: String,
    pub source_kind: String,
    pub source_label: String,
    pub kind: EventKind,
    pub text: String,
    pub snippet: String,
    pub score: f64,
    pub session_timestamp: String,
    pub cwd: String,
    pub source_file_path: PathBuf,
    pub source_line_number: usize,
    pub source_timestamp: Option<String>,
    pub session_hit_count: usize,
    pub best_kind_weight: u8,
    pub repo_matches_current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    pub query: String,
    pub limit: usize,
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub since: Option<String>,
    pub from: Option<String>,
    pub until: Option<String>,
    pub include_duplicates: bool,
    pub exclude_sessions: Vec<String>,
    pub kinds: Vec<EventKind>,
    pub current_repo: Option<String>,
    pub mode: SearchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    AllTerms,
    Phrase,
    Near(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchStrategy {
    AllTerms,
    AnyTermFallback,
    Phrase,
    Near,
}

impl MatchStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchStrategy::AllTerms => "all_terms",
            MatchStrategy::AnyTermFallback => "any_terms_fallback",
            MatchStrategy::Phrase => "phrase",
            MatchStrategy::Near => "near",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTrace {
    pub match_strategy: MatchStrategy,
    pub query_terms: Vec<String>,
    pub fts_query: String,
    pub fetch_limit: usize,
    pub current_repo: Option<String>,
    pub include_duplicates: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryObject {
    pub id: String,
    pub kind: MemoryKind,
    pub summary: String,
    pub normalized_text: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub evidence_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEvidence {
    pub memory_id: String,
    pub session_key: String,
    pub session_id: String,
    pub repo: String,
    pub cwd: String,
    pub session_timestamp: String,
    pub source_file_path: PathBuf,
    pub source_line_number: usize,
    pub source_timestamp: Option<String>,
    pub event_kind: EventKind,
    pub evidence_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySearchOptions {
    pub query: Option<String>,
    pub limit: usize,
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub since: Option<String>,
    pub from: Option<String>,
    pub until: Option<String>,
    pub kinds: Vec<MemoryKind>,
}

impl Default for MemorySearchOptions {
    fn default() -> Self {
        Self {
            query: None,
            limit: 20,
            repo: None,
            cwd: None,
            since: None,
            from: None,
            until: None,
            kinds: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryResult {
    pub object: MemoryObject,
    pub repos: Vec<String>,
    pub session_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaItem {
    Session {
        change_id: i64,
        action: String,
        session_key: String,
        session_id: String,
        repo: String,
        cwd: String,
        session_timestamp: String,
        updated_at: String,
    },
    Memory {
        change_id: i64,
        action: String,
        object: MemoryObject,
        repos: Vec<String>,
        session_keys: Vec<String>,
    },
    Deleted {
        change_id: i64,
        object_type: String,
        object_id: String,
        action: String,
        updated_at: String,
    },
}

impl DeltaItem {
    pub fn updated_at(&self) -> &str {
        match self {
            DeltaItem::Session { updated_at, .. } => updated_at,
            DeltaItem::Memory { object, .. } => &object.updated_at,
            DeltaItem::Deleted { updated_at, .. } => updated_at,
        }
    }

    pub fn change_id(&self) -> i64 {
        match self {
            DeltaItem::Session { change_id, .. } => *change_id,
            DeltaItem::Memory { change_id, .. } => *change_id,
            DeltaItem::Deleted { change_id, .. } => *change_id,
        }
    }

    pub fn change_kind(&self) -> &str {
        match self {
            DeltaItem::Session { .. } => "session",
            DeltaItem::Memory { .. } => "memory",
            DeltaItem::Deleted { object_type, .. } => object_type,
        }
    }

    pub fn action(&self) -> &str {
        match self {
            DeltaItem::Session { action, .. } => action,
            DeltaItem::Memory { action, .. } => action,
            DeltaItem::Deleted { action, .. } => action,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRecord {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
    pub object_type: String,
    pub updated_at: String,
}

impl SearchOptions {
    pub fn new(query: impl Into<String>, limit: usize) -> Self {
        Self {
            query: query.into(),
            limit,
            repo: None,
            cwd: None,
            since: None,
            from: None,
            until: None,
            include_duplicates: false,
            exclude_sessions: Vec::new(),
            kinds: Vec::new(),
            current_repo: None,
            mode: SearchMode::AllTerms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    pub session_key: String,
    pub session_id: String,
    pub source_kind: String,
    pub source_label: String,
    pub kind: EventKind,
    pub text: String,
    pub cwd: String,
    pub source_file_path: PathBuf,
    pub source_line_number: usize,
    pub source_timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMatch {
    pub session_key: String,
    pub session_id: String,
    pub cwd: String,
    pub repo: String,
    pub source_kind: String,
    pub source_label: String,
    pub source_file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSession {
    pub session_key: String,
    pub session_id: String,
    pub repo: String,
    pub source_kind: String,
    pub source_label: String,
    pub cwd: String,
    pub session_timestamp: String,
    pub source_file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentOptions {
    pub limit: usize,
    pub repo: Option<String>,
    pub cwd: Option<String>,
    pub since: Option<String>,
    pub from: Option<String>,
    pub until: Option<String>,
    pub include_duplicates: bool,
    pub exclude_sessions: Vec<String>,
    pub kinds: Vec<EventKind>,
}

impl Default for RecentOptions {
    fn default() -> Self {
        Self {
            limit: 20,
            repo: None,
            cwd: None,
            since: None,
            from: None,
            until: None,
            include_duplicates: false,
            exclude_sessions: Vec::new(),
            kinds: Vec::new(),
        }
    }
}

struct OldSessionRow {
    session_id: String,
    session_timestamp: String,
    cwd: String,
    repo: String,
    cli_version: Option<String>,
    source_file_path: String,
}

struct OldEventRow {
    session_id: String,
    kind: String,
    role: Option<String>,
    text: String,
    command: Option<String>,
    cwd: Option<String>,
    exit_code: Option<i64>,
    source_timestamp: Option<String>,
    source_file_path: String,
    source_line_number: i64,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }

        let conn = Connection::open(path).with_context(|| format!("open db {}", path.display()))?;
        configure_write_connection(&conn)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("open db read-only {}", path.display()))?;
        configure_read_connection(&conn)?;
        Ok(Self { conn })
    }

    pub fn index_session(&self, parsed: &ParsedSession) -> Result<()> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = self.index_session_inner(parsed);
        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", [])?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    pub fn begin_index_batch(&self) -> Result<()> {
        self.conn.execute("BEGIN IMMEDIATE", [])?;
        Ok(())
    }

    pub fn commit_index_batch(&self) -> Result<()> {
        self.conn.execute("COMMIT", [])?;
        Ok(())
    }

    pub fn rollback_index_batch(&self) -> Result<()> {
        self.conn.execute("ROLLBACK", [])?;
        Ok(())
    }

    pub fn index_session_in_batch(&self, parsed: &ParsedSession) -> Result<()> {
        self.index_session_inner(parsed)
    }

    pub fn delete_sessions_for_source_kind(
        &self,
        source_file_path: &Path,
        source_kind: &str,
    ) -> Result<usize> {
        let source_file_path = source_file_path.display().to_string();
        let mut statement = self.conn.prepare(
            r#"
            SELECT session_key
            FROM sessions
            WHERE source_file_path = ? AND source_kind = ?
            ORDER BY session_key ASC
            "#,
        )?;
        let rows = statement.query_map(params![source_file_path.as_str(), source_kind], |row| {
            row.get::<_, String>(0)
        })?;
        let session_keys = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        drop(statement);

        let mut affected_memory_ids = Vec::new();
        for session_key in &session_keys {
            affected_memory_ids.extend(self.memory_ids_for_session(session_key)?);
            self.conn.execute(
                "DELETE FROM events_fts WHERE session_key = ?",
                params![session_key],
            )?;
            self.conn.execute(
                "DELETE FROM memory_evidence WHERE session_key = ?",
                params![session_key],
            )?;
            self.conn.execute(
                "DELETE FROM events WHERE session_key = ?",
                params![session_key],
            )?;
            self.conn.execute(
                "DELETE FROM session_repos WHERE session_key = ?",
                params![session_key],
            )?;
            self.conn.execute(
                "DELETE FROM sessions WHERE session_key = ?",
                params![session_key],
            )?;
            self.record_change("session", session_key, "delete")?;
        }

        affected_memory_ids.sort();
        affected_memory_ids.dedup();
        self.refresh_memory_objects(&affected_memory_ids)?;

        Ok(session_keys.len())
    }

    pub fn stats(&self) -> Result<Stats> {
        let session_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                row.get::<_, u64>(0)
            })?;
        let event_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| {
                row.get::<_, u64>(0)
            })?;
        let source_file_count =
            self.conn
                .query_row("SELECT COUNT(*) FROM ingestion_state", [], |row| {
                    row.get::<_, u64>(0)
                })?;
        let unique_ingested_sessions = self.conn.query_row(
            "SELECT COUNT(DISTINCT COALESCE(session_key, session_id)) FROM ingestion_state WHERE COALESCE(session_key, session_id) IS NOT NULL",
            [],
            |row| row.get::<_, u64>(0),
        )?;

        Ok(Stats {
            session_count,
            event_count,
            source_file_count,
            duplicate_source_file_count: source_file_count.saturating_sub(unique_ingested_sessions),
        })
    }

    pub fn quick_check(&self) -> Result<String> {
        self.conn
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .map_err(Into::into)
    }

    pub fn fts_integrity_check(&self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events_fts(events_fts) VALUES('integrity-check')",
            [],
        )?;
        Ok(())
    }

    pub fn fts_read_check(&self) -> Result<()> {
        self.conn
            .query_row("SELECT COUNT(*) FROM events_fts", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|_| ())
            .map_err(Into::into)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        self.search_with_options(SearchOptions::new(query, limit))
    }

    pub fn search_with_options(&self, options: SearchOptions) -> Result<Vec<SearchResult>> {
        self.search_with_trace(options).map(|(_, results)| results)
    }

    pub fn search_with_trace(
        &self,
        options: SearchOptions,
    ) -> Result<(SearchTrace, Vec<SearchResult>)> {
        let terms = fts_terms(&options.query);
        if terms.is_empty() {
            return Ok((
                SearchTrace {
                    match_strategy: MatchStrategy::AllTerms,
                    query_terms: Vec::new(),
                    fts_query: String::new(),
                    fetch_limit: 0,
                    current_repo: options.current_repo.clone(),
                    include_duplicates: options.include_duplicates,
                },
                Vec::new(),
            ));
        }

        let limit = options.limit.clamp(1, 100);
        let fetch_limit = limit.saturating_mul(50).clamp(200, 1_000);
        match options.mode {
            SearchMode::AllTerms => {
                let all_terms_query = and_fts_query(&terms);
                let results =
                    self.search_with_fts_query(&options, &all_terms_query, fetch_limit)?;
                if !results.is_empty() || terms.len() == 1 {
                    return Ok((
                        SearchTrace {
                            match_strategy: MatchStrategy::AllTerms,
                            query_terms: terms,
                            fts_query: all_terms_query,
                            fetch_limit,
                            current_repo: options.current_repo.clone(),
                            include_duplicates: options.include_duplicates,
                        },
                        rank_search_results(
                            results,
                            options.current_repo.as_deref(),
                            limit,
                            options.include_duplicates,
                        ),
                    ));
                }

                let any_terms_query = or_fts_query(&terms);
                let results =
                    self.search_with_fts_query(&options, &any_terms_query, fetch_limit)?;
                Ok((
                    SearchTrace {
                        match_strategy: MatchStrategy::AnyTermFallback,
                        query_terms: terms,
                        fts_query: any_terms_query,
                        fetch_limit,
                        current_repo: options.current_repo.clone(),
                        include_duplicates: options.include_duplicates,
                    },
                    rank_search_results(
                        results,
                        options.current_repo.as_deref(),
                        limit,
                        options.include_duplicates,
                    ),
                ))
            }
            SearchMode::Phrase => {
                let phrase_query = phrase_fts_query(&terms);
                let results = self.search_with_fts_query(&options, &phrase_query, fetch_limit)?;
                Ok((
                    SearchTrace {
                        match_strategy: MatchStrategy::Phrase,
                        query_terms: terms,
                        fts_query: phrase_query,
                        fetch_limit,
                        current_repo: options.current_repo.clone(),
                        include_duplicates: options.include_duplicates,
                    },
                    rank_search_results(
                        results,
                        options.current_repo.as_deref(),
                        limit,
                        options.include_duplicates,
                    ),
                ))
            }
            SearchMode::Near(distance) => {
                let near_query = near_fts_query(&terms, distance);
                let results = self.search_with_fts_query(&options, &near_query, fetch_limit)?;
                Ok((
                    SearchTrace {
                        match_strategy: MatchStrategy::Near,
                        query_terms: terms,
                        fts_query: near_query,
                        fetch_limit,
                        current_repo: options.current_repo.clone(),
                        include_duplicates: options.include_duplicates,
                    },
                    rank_search_results(
                        results,
                        options.current_repo.as_deref(),
                        limit,
                        options.include_duplicates,
                    ),
                ))
            }
        }
    }

    fn search_with_fts_query(
        &self,
        options: &SearchOptions,
        fts_query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let mut query_params = Vec::<String>::new();
        let current_repo_expr = if let Some(current_repo) = &options.current_repo {
            query_params.push(current_repo.clone());
            "EXISTS (
                    SELECT 1 FROM session_repos current_repos
                    WHERE current_repos.session_key = sessions.session_key
                      AND lower(current_repos.repo) = lower(?)
                )"
        } else {
            "0"
        };
        query_params.push(fts_query.to_owned());

        let mut sql = format!(
            r#"
            SELECT
                events.session_key,
                events.session_id,
                sessions.repo,
                sessions.source_kind,
                sessions.source_label,
                events.kind,
                events.text,
                snippet(events_fts, 4, '', '', ' ... ', 16) AS snippet,
                events_fts.rank AS score,
                sessions.session_timestamp,
                sessions.cwd,
                events.source_file_path,
                events.source_line_number,
                events.source_timestamp,
                {current_repo_expr} AS current_repo_match
            FROM events_fts
            JOIN events ON events.id = events_fts.event_id
            JOIN sessions ON sessions.session_key = events.session_key
            WHERE events_fts MATCH ?
            "#,
        );

        if let Some(repo) = &options.repo {
            sql.push_str(
                r#"
                AND EXISTS (
                    SELECT 1 FROM session_repos filter_repos
                    WHERE filter_repos.session_key = sessions.session_key
                      AND lower(filter_repos.repo) = lower(?)
                )
                "#,
            );
            query_params.push(repo.clone());
        }
        if let Some(cwd) = &options.cwd {
            sql.push_str(
                r#"
                AND (
                    sessions.cwd LIKE '%' || ? || '%'
                    OR EXISTS (
                        SELECT 1 FROM events cwd_events
                        WHERE cwd_events.session_key = sessions.session_key
                          AND cwd_events.cwd LIKE '%' || ? || '%'
                    )
                )
                "#,
            );
            query_params.push(cwd.clone());
            query_params.push(cwd.clone());
        }
        append_from_until_clauses(
            &mut sql,
            &mut query_params,
            options.since.as_ref(),
            options.from.as_ref(),
            options.until.as_ref(),
        )?;
        append_excluded_sessions_clause(&mut sql, &mut query_params, &options.exclude_sessions);
        append_event_kind_clause(&mut sql, &mut query_params, "events.kind", &options.kinds);

        sql.push_str(" ORDER BY events_fts.rank ASC, events.source_line_number ASC LIMIT ");
        sql.push_str(&limit.to_string());

        let mut statement = self.conn.prepare(&sql)?;

        let rows = statement.query_map(params_from_iter(query_params.iter()), |row| {
            let kind_text: String = row.get(5)?;
            let kind = kind_text.parse::<EventKind>().map_err(|_| {
                rusqlite::Error::InvalidColumnType(
                    5,
                    "kind".to_owned(),
                    rusqlite::types::Type::Text,
                )
            })?;
            let source_file_path: String = row.get(11)?;
            let source_line_number: i64 = row.get(12)?;

            Ok(SearchResult {
                session_key: row.get(0)?,
                session_id: row.get(1)?,
                repo: row.get(2)?,
                source_kind: row.get(3)?,
                source_label: row.get(4)?,
                kind,
                text: row.get(6)?,
                snippet: row.get(7)?,
                score: row.get(8)?,
                session_timestamp: row.get(9)?,
                cwd: row.get(10)?,
                source_file_path: PathBuf::from(source_file_path),
                source_line_number: source_line_number as usize,
                source_timestamp: row.get(13)?,
                session_hit_count: 0,
                best_kind_weight: 0,
                repo_matches_current: row.get::<_, i64>(14)? != 0,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn resolve_session_reference(&self, reference: &str) -> Result<Vec<SessionMatch>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT session_key, session_id, cwd, repo, source_kind, source_label, source_file_path
            FROM sessions
            WHERE session_key = ? OR session_id = ?
            ORDER BY session_timestamp DESC, source_file_path ASC
            "#,
        )?;
        let rows = statement.query_map(params![reference, reference], |row| {
            let source_file_path: String = row.get(6)?;
            Ok(SessionMatch {
                session_key: row.get(0)?,
                session_id: row.get(1)?,
                cwd: row.get(2)?,
                repo: row.get(3)?,
                source_kind: row.get(4)?,
                source_label: row.get(5)?,
                source_file_path: PathBuf::from(source_file_path),
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn recent_sessions(&self, options: RecentOptions) -> Result<Vec<RecentSession>> {
        let limit = options.limit.clamp(1, 100);
        let fetch_limit = if options.include_duplicates {
            limit
        } else {
            limit.saturating_mul(5).clamp(limit, 500)
        };
        let mut query_params = Vec::<String>::new();
        let mut sql = r#"
            SELECT
                sessions.session_key,
                sessions.session_id,
                sessions.repo,
                sessions.source_kind,
                sessions.source_label,
                sessions.cwd,
                sessions.session_timestamp,
                sessions.source_file_path
            FROM sessions
            WHERE 1 = 1
            "#
        .to_owned();

        if let Some(repo) = &options.repo {
            sql.push_str(
                r#"
                AND EXISTS (
                    SELECT 1 FROM session_repos filter_repos
                    WHERE filter_repos.session_key = sessions.session_key
                      AND lower(filter_repos.repo) = lower(?)
                )
                "#,
            );
            query_params.push(repo.clone());
        }
        if let Some(cwd) = &options.cwd {
            sql.push_str(
                r#"
                AND (
                    sessions.cwd LIKE '%' || ? || '%'
                    OR EXISTS (
                        SELECT 1 FROM events cwd_events
                        WHERE cwd_events.session_key = sessions.session_key
                          AND cwd_events.cwd LIKE '%' || ? || '%'
                    )
                )
                "#,
            );
            query_params.push(cwd.clone());
            query_params.push(cwd.clone());
        }
        append_from_until_clauses(
            &mut sql,
            &mut query_params,
            options.since.as_ref(),
            options.from.as_ref(),
            options.until.as_ref(),
        )?;
        append_excluded_sessions_clause(&mut sql, &mut query_params, &options.exclude_sessions);
        append_recent_event_kind_clause(&mut sql, &mut query_params, &options.kinds);

        sql.push_str(
            r#"
            ORDER BY datetime(replace(replace(sessions.session_timestamp, 'T', ' '), 'Z', '')) DESC,
                     sessions.source_file_path ASC
            LIMIT ?
            "#,
        );
        query_params.push(fetch_limit.to_string());

        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(query_params.iter()), |row| {
            let source_file_path: String = row.get(7)?;
            Ok(RecentSession {
                session_key: row.get(0)?,
                session_id: row.get(1)?,
                repo: row.get(2)?,
                source_kind: row.get(3)?,
                source_label: row.get(4)?,
                cwd: row.get(5)?,
                session_timestamp: row.get(6)?,
                source_file_path: PathBuf::from(source_file_path),
            })
        })?;

        let mut sessions = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if !options.include_duplicates {
            sessions = dedupe_recent_sessions(sessions);
        }
        sessions.truncate(limit);
        Ok(sessions)
    }

    pub fn session_events(&self, session_key: &str, limit: usize) -> Result<Vec<SessionEvent>> {
        self.session_events_with_kinds(session_key, limit, &[])
    }

    pub fn session_events_with_kinds(
        &self,
        session_key: &str,
        limit: usize,
        kinds: &[EventKind],
    ) -> Result<Vec<SessionEvent>> {
        let limit = limit.clamp(1, 500);
        let mut query_params = vec![session_key.to_owned()];
        let mut sql = r#"
            SELECT
                events.session_key,
                events.session_id,
                sessions.source_kind,
                sessions.source_label,
                events.kind,
                events.text,
                sessions.cwd,
                events.source_file_path,
                events.source_line_number,
                events.source_timestamp
            FROM events
            JOIN sessions ON sessions.session_key = events.session_key
            WHERE events.session_key = ?
            "#
        .to_owned();
        append_event_kind_clause(&mut sql, &mut query_params, "events.kind", kinds);
        sql.push_str(
            r#"
            ORDER BY events.source_line_number ASC
            LIMIT ?
            "#,
        );
        query_params.push(limit.to_string());

        let mut statement = self.conn.prepare(&sql)?;

        let rows = statement.query_map(params_from_iter(query_params.iter()), |row| {
            let kind_text: String = row.get(4)?;
            let kind = kind_text.parse::<EventKind>().map_err(|_| {
                rusqlite::Error::InvalidColumnType(
                    4,
                    "kind".to_owned(),
                    rusqlite::types::Type::Text,
                )
            })?;
            let source_file_path: String = row.get(7)?;
            let source_line_number: i64 = row.get(8)?;

            Ok(SessionEvent {
                session_key: row.get(0)?,
                session_id: row.get(1)?,
                source_kind: row.get(2)?,
                source_label: row.get(3)?,
                kind,
                text: row.get(5)?,
                cwd: row.get(6)?,
                source_file_path: PathBuf::from(source_file_path),
                source_line_number: source_line_number as usize,
                source_timestamp: row.get(9)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn session_repos(&self, session_key: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT repo
            FROM session_repos
            WHERE session_key = ?
            ORDER BY lower(repo) ASC
            "#,
        )?;
        let rows = statement.query_map(params![session_key], |row| row.get::<_, String>(0))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn memory_results_with_trace(
        &self,
        options: MemorySearchOptions,
    ) -> Result<(MatchStrategy, Vec<MemoryResult>)> {
        let limit = options.limit.clamp(1, 100);
        let Some(query) = options
            .query
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            let ids = self.list_memory_ids(&options, limit)?;
            return Ok((MatchStrategy::AllTerms, self.load_memory_results(&ids)?));
        };

        let terms = fts_terms(query);
        if terms.is_empty() {
            return Ok((MatchStrategy::AllTerms, Vec::new()));
        }

        let ids = self.search_memory_ids_with_fts(&options, &and_fts_query(&terms), limit)?;
        if !ids.is_empty() || terms.len() == 1 {
            return Ok((MatchStrategy::AllTerms, self.load_memory_results(&ids)?));
        }

        let ids = self.search_memory_ids_with_fts(&options, &or_fts_query(&terms), limit)?;
        Ok((
            MatchStrategy::AnyTermFallback,
            self.load_memory_results(&ids)?,
        ))
    }

    pub fn memory_results(&self, options: MemorySearchOptions) -> Result<Vec<MemoryResult>> {
        self.memory_results_with_trace(options)
            .map(|(_, results)| results)
    }

    pub fn memory_by_id(&self, memory_id: &str) -> Result<Option<MemoryObject>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT id, kind, summary, normalized_text, first_seen_at, last_seen_at, created_at, updated_at
            FROM memory_objects
            WHERE id = ?
            "#,
        )?;
        let mut rows = statement.query(params![memory_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let kind_text: String = row.get(1)?;
        let kind = MemoryKind::parse(&kind_text)
            .ok_or_else(|| anyhow::anyhow!("unknown memory kind `{kind_text}`"))?;
        let evidence_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_evidence WHERE memory_id = ?",
            params![memory_id],
            |count_row| count_row.get(0),
        )?;

        Ok(Some(MemoryObject {
            id: row.get(0)?,
            kind,
            summary: row.get(2)?,
            normalized_text: row.get(3)?,
            first_seen_at: row.get(4)?,
            last_seen_at: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            evidence_count: evidence_count as usize,
        }))
    }

    pub fn memory_evidence(&self, memory_id: &str, limit: usize) -> Result<Vec<MemoryEvidence>> {
        let limit = limit.clamp(1, 200);
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                memory_evidence.memory_id,
                memory_evidence.session_key,
                memory_evidence.session_id,
                sessions.repo,
                sessions.cwd,
                sessions.session_timestamp,
                memory_evidence.source_file_path,
                memory_evidence.source_line_number,
                memory_evidence.source_timestamp,
                memory_evidence.event_kind,
                memory_evidence.evidence_text
            FROM memory_evidence
            JOIN sessions ON sessions.session_key = memory_evidence.session_key
            WHERE memory_evidence.memory_id = ?
            ORDER BY datetime(replace(replace(sessions.session_timestamp, 'T', ' '), 'Z', '')) DESC,
                     memory_evidence.source_line_number ASC
            LIMIT ?
            "#,
        )?;
        let rows = statement.query_map(params![memory_id, limit.to_string()], |row| {
            let event_kind_text: String = row.get(9)?;
            let event_kind = event_kind_text.parse::<EventKind>().map_err(|_| {
                rusqlite::Error::InvalidColumnType(
                    9,
                    "event_kind".to_owned(),
                    rusqlite::types::Type::Text,
                )
            })?;
            let source_file_path: String = row.get(6)?;
            let source_line_number: i64 = row.get(7)?;
            Ok(MemoryEvidence {
                memory_id: row.get(0)?,
                session_key: row.get(1)?,
                session_id: row.get(2)?,
                repo: row.get(3)?,
                cwd: row.get(4)?,
                session_timestamp: row.get(5)?,
                source_file_path: PathBuf::from(source_file_path),
                source_line_number: source_line_number as usize,
                source_timestamp: row.get(8)?,
                event_kind,
                evidence_text: row.get(10)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn delta_items(
        &self,
        cursor: Option<&str>,
        limit: usize,
        repo: Option<&str>,
    ) -> Result<Vec<DeltaItem>> {
        if !self.table_exists("change_log")? {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 200);
        let mut next_change_id = cursor
            .and_then(parse_delta_cursor)
            .map_or(0, |item| item.change_id);
        let mut items = Vec::new();
        let batch_size = limit.saturating_mul(5).clamp(100, 500);

        while items.len() < limit {
            let rows = self.next_change_rows(next_change_id, batch_size)?;
            if rows.is_empty() {
                break;
            }
            next_change_id = rows.last().map(|row| row.seq).unwrap_or(next_change_id);
            for row in rows {
                if let Some(item) = self.resolve_delta_item(&row, repo)? {
                    items.push(item);
                    if items.len() == limit {
                        break;
                    }
                }
            }
        }

        Ok(items)
    }

    pub fn related_memories_for_session(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Result<Vec<MemoryResult>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT memory_id
            FROM memory_evidence
            WHERE session_key = ?
            GROUP BY memory_id
            ORDER BY COUNT(*) DESC, memory_id ASC
            LIMIT ?
            "#,
        )?;
        let rows = statement.query_map(params![session_key, limit.to_string()], |row| {
            row.get::<_, String>(0)
        })?;
        let ids = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_memory_results(&ids)
    }

    pub fn related_sessions_for_session(
        &self,
        session_key: &str,
        limit: usize,
    ) -> Result<Vec<RecentSession>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                sessions.session_key,
                sessions.session_id,
                sessions.repo,
                sessions.source_kind,
                sessions.source_label,
                sessions.cwd,
                sessions.session_timestamp,
                sessions.source_file_path
            FROM memory_evidence seed
            JOIN memory_evidence related ON related.memory_id = seed.memory_id
            JOIN sessions ON sessions.session_key = related.session_key
            WHERE seed.session_key = ?
              AND related.session_key != ?
            GROUP BY sessions.session_key
            ORDER BY COUNT(DISTINCT related.memory_id) DESC,
                     datetime(replace(replace(sessions.session_timestamp, 'T', ' '), 'Z', '')) DESC,
                     sessions.session_key ASC
            LIMIT ?
            "#,
        )?;
        let rows = statement.query_map(
            params![session_key, session_key, limit.to_string()],
            |row| {
                let source_file_path: String = row.get(7)?;
                Ok(RecentSession {
                    session_key: row.get(0)?,
                    session_id: row.get(1)?,
                    repo: row.get(2)?,
                    source_kind: row.get(3)?,
                    source_label: row.get(4)?,
                    cwd: row.get(5)?,
                    session_timestamp: row.get(6)?,
                    source_file_path: PathBuf::from(source_file_path),
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn related_sessions_for_memory(
        &self,
        memory_id: &str,
        limit: usize,
    ) -> Result<Vec<RecentSession>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                sessions.session_key,
                sessions.session_id,
                sessions.repo,
                sessions.source_kind,
                sessions.source_label,
                sessions.cwd,
                sessions.session_timestamp,
                sessions.source_file_path
            FROM memory_evidence
            JOIN sessions ON sessions.session_key = memory_evidence.session_key
            WHERE memory_evidence.memory_id = ?
            GROUP BY sessions.session_key
            ORDER BY datetime(replace(replace(sessions.session_timestamp, 'T', ' '), 'Z', '')) DESC,
                     sessions.session_key ASC
            LIMIT ?
            "#,
        )?;
        let rows = statement.query_map(params![memory_id, limit.to_string()], |row| {
            let source_file_path: String = row.get(7)?;
            Ok(RecentSession {
                session_key: row.get(0)?,
                session_id: row.get(1)?,
                repo: row.get(2)?,
                source_kind: row.get(3)?,
                source_label: row.get(4)?,
                cwd: row.get(5)?,
                session_timestamp: row.get(6)?,
                source_file_path: PathBuf::from(source_file_path),
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn cooccurring_memories(&self, memory_id: &str, limit: usize) -> Result<Vec<MemoryResult>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT related.memory_id
            FROM memory_evidence seed
            JOIN memory_evidence related ON related.session_key = seed.session_key
            WHERE seed.memory_id = ?
              AND related.memory_id != ?
            GROUP BY related.memory_id
            ORDER BY COUNT(DISTINCT related.session_key) DESC, related.memory_id ASC
            LIMIT ?
            "#,
        )?;
        let rows = statement
            .query_map(params![memory_id, memory_id, limit.to_string()], |row| {
                row.get::<_, String>(0)
            })?;
        let ids = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        self.load_memory_results(&ids)
    }

    pub fn session_resources(&self, limit: usize) -> Result<Vec<ResourceRecord>> {
        let sessions = self.recent_sessions(RecentOptions {
            limit,
            ..RecentOptions::default()
        })?;
        Ok(sessions
            .into_iter()
            .map(|session| ResourceRecord {
                uri: format!("agent-recall://session/{}", session.session_key),
                name: format!("session {}", session.session_id),
                description: format!("{} {}", session.repo, session.cwd),
                mime_type: "application/json".to_owned(),
                object_type: "session".to_owned(),
                updated_at: session.session_timestamp,
            })
            .collect())
    }

    pub fn memory_resources(&self, limit: usize) -> Result<Vec<ResourceRecord>> {
        let memories = self.memory_results(MemorySearchOptions {
            limit,
            ..MemorySearchOptions::default()
        })?;
        Ok(memories
            .into_iter()
            .map(|memory| ResourceRecord {
                uri: format!("agent-recall://memory/{}", memory.object.id),
                name: format!("{} {}", memory.object.kind.as_str(), memory.object.id),
                description: memory.object.summary,
                mime_type: "application/json".to_owned(),
                object_type: "memory".to_owned(),
                updated_at: memory.object.updated_at,
            })
            .collect())
    }

    fn list_memory_ids(&self, options: &MemorySearchOptions, limit: usize) -> Result<Vec<String>> {
        let mut sql = "SELECT memory_objects.id FROM memory_objects WHERE 1 = 1".to_owned();
        let mut params = Vec::<String>::new();
        append_memory_filter_clauses(&mut sql, &mut params, options)?;
        sql.push_str(
            r#"
            ORDER BY datetime(replace(replace(memory_objects.last_seen_at, 'T', ' '), 'Z', '')) DESC,
                     memory_objects.id ASC
            LIMIT ?
            "#,
        );
        params.push(limit.to_string());
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(params.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn search_memory_ids_with_fts(
        &self,
        options: &MemorySearchOptions,
        fts_query: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let fetch_limit = limit.saturating_mul(10).clamp(limit, 500);
        let mut sql = r#"
            SELECT memory_objects.id
            FROM memory_fts
            JOIN memory_objects ON memory_objects.id = memory_fts.memory_id
            WHERE memory_fts MATCH ?
        "#
        .to_owned();
        let mut params = vec![fts_query.to_owned()];
        append_memory_filter_clauses(&mut sql, &mut params, options)?;
        sql.push_str(
            r#"
            ORDER BY memory_fts.rank ASC,
                     datetime(replace(replace(memory_objects.last_seen_at, 'T', ' '), 'Z', '')) DESC,
                     memory_objects.id ASC
            LIMIT ?
            "#,
        );
        params.push(fetch_limit.to_string());
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(params.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        let mut ids = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        ids.dedup();
        if ids.len() > limit {
            ids.truncate(limit);
        }
        Ok(ids)
    }

    fn load_memory_results(&self, ids: &[String]) -> Result<Vec<MemoryResult>> {
        let mut results = Vec::new();
        for id in ids {
            let Some(object) = self.memory_by_id(id)? else {
                continue;
            };
            let repos = self.memory_repos(id)?;
            let session_keys = self.memory_session_keys(id)?;
            results.push(MemoryResult {
                object,
                repos,
                session_keys,
            });
        }
        Ok(results)
    }

    fn memory_repos(&self, memory_id: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT DISTINCT sessions.repo
            FROM memory_evidence
            JOIN sessions ON sessions.session_key = memory_evidence.session_key
            WHERE memory_evidence.memory_id = ?
              AND sessions.repo != ''
            ORDER BY lower(sessions.repo) ASC
            "#,
        )?;
        let rows = statement.query_map(params![memory_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn memory_session_keys(&self, memory_id: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT DISTINCT session_key
            FROM memory_evidence
            WHERE memory_id = ?
            ORDER BY session_key ASC
            "#,
        )?;
        let rows = statement.query_map(params![memory_id], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn memory_ids_for_session(&self, session_key: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT DISTINCT memory_id
            FROM memory_evidence
            WHERE session_key = ?
            ORDER BY memory_id ASC
            "#,
        )?;
        let rows = statement.query_map(params![session_key], |row| row.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn upsert_memory_object(
        &self,
        memory: &ExtractedMemory,
        session_timestamp: &str,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO memory_objects (
                id, kind, summary, normalized_text, first_seen_at, last_seen_at, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind,
                summary = CASE
                    WHEN length(excluded.summary) < length(memory_objects.summary) THEN excluded.summary
                    ELSE memory_objects.summary
                END,
                normalized_text = excluded.normalized_text,
                first_seen_at = MIN(memory_objects.first_seen_at, excluded.first_seen_at),
                last_seen_at = MAX(memory_objects.last_seen_at, excluded.last_seen_at),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
            "#,
            params![
                memory.id,
                memory.kind.as_str(),
                memory.summary,
                memory.normalized_text,
                session_timestamp,
                session_timestamp,
            ],
        )?;
        Ok(())
    }

    fn refresh_memory_objects(&self, memory_ids: &[String]) -> Result<()> {
        for memory_id in memory_ids {
            let evidence_count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM memory_evidence WHERE memory_id = ?",
                params![memory_id],
                |row| row.get(0),
            )?;

            if evidence_count == 0 {
                self.conn.execute(
                    "DELETE FROM memory_fts WHERE memory_id = ?",
                    params![memory_id],
                )?;
                self.conn.execute(
                    "DELETE FROM memory_objects WHERE id = ?",
                    params![memory_id],
                )?;
                self.record_change("memory", memory_id, "delete")?;
                continue;
            }

            let (first_seen_at, last_seen_at, kind, summary, normalized_text): (
                String,
                String,
                String,
                String,
                String,
            ) = self.conn.query_row(
                r#"
                SELECT
                    MIN(sessions.session_timestamp),
                    MAX(sessions.session_timestamp),
                    memory_objects.kind,
                    memory_objects.summary,
                    memory_objects.normalized_text
                FROM memory_objects
                JOIN memory_evidence ON memory_evidence.memory_id = memory_objects.id
                JOIN sessions ON sessions.session_key = memory_evidence.session_key
                WHERE memory_objects.id = ?
                GROUP BY memory_objects.id
                "#,
                params![memory_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;

            self.conn.execute(
                r#"
                UPDATE memory_objects
                SET first_seen_at = ?, last_seen_at = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                WHERE id = ?
                "#,
                params![first_seen_at, last_seen_at, memory_id],
            )?;
            self.conn.execute(
                "DELETE FROM memory_fts WHERE memory_id = ?",
                params![memory_id],
            )?;
            self.conn.execute(
                r#"
                INSERT INTO memory_fts (memory_id, kind, summary, normalized_text)
                VALUES (?, ?, ?, ?)
                "#,
                params![memory_id, kind, summary, normalized_text],
            )?;
            self.record_change("memory", memory_id, "upsert")?;
        }

        Ok(())
    }

    fn record_change(&self, object_type: &str, object_id: &str, action: &str) -> Result<i64> {
        self.conn.execute(
            r#"
            INSERT INTO change_log (object_type, object_id, action)
            VALUES (?, ?, ?)
            "#,
            params![object_type, object_id, action],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn next_change_rows(&self, after_change_id: i64, limit: usize) -> Result<Vec<ChangeLogRow>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT seq, object_type, object_id, action, recorded_at
            FROM change_log
            WHERE seq > ?
            ORDER BY seq ASC
            LIMIT ?
            "#,
        )?;
        let rows = statement.query_map(params![after_change_id, limit as i64], |row| {
            Ok(ChangeLogRow {
                seq: row.get(0)?,
                object_type: row.get(1)?,
                object_id: row.get(2)?,
                action: row.get(3)?,
                recorded_at: row.get(4)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn session_has_repo(&self, session_key: &str, repo: &str) -> Result<bool> {
        let count = self.conn.query_row(
            r#"
            SELECT COUNT(*)
            FROM session_repos
            WHERE session_key = ?
              AND lower(repo) = lower(?)
            "#,
            params![session_key, repo],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    fn memory_result_by_id(&self, memory_id: &str) -> Result<Option<MemoryResult>> {
        self.load_memory_results(&[memory_id.to_owned()])
            .map(|results| results.into_iter().next())
    }

    fn resolve_delta_item(
        &self,
        row: &ChangeLogRow,
        repo: Option<&str>,
    ) -> Result<Option<DeltaItem>> {
        match row.object_type.as_str() {
            "session" => {
                let mut statement = self.conn.prepare(
                    r#"
                    SELECT session_key, session_id, repo, cwd, session_timestamp, indexed_at
                    FROM sessions
                    WHERE session_key = ?
                    "#,
                )?;
                let mut rows = statement.query(params![row.object_id.as_str()])?;
                let Some(session_row) = rows.next()? else {
                    return Ok(None);
                };
                let session_key: String = session_row.get(0)?;
                if let Some(repo_filter) = repo {
                    if !self.session_has_repo(&session_key, repo_filter)? {
                        return Ok(None);
                    }
                }

                Ok(Some(DeltaItem::Session {
                    change_id: row.seq,
                    action: row.action.clone(),
                    session_key,
                    session_id: session_row.get(1)?,
                    repo: session_row.get(2)?,
                    cwd: session_row.get(3)?,
                    session_timestamp: session_row.get(4)?,
                    updated_at: session_row.get(5)?,
                }))
            }
            "memory" => {
                if let Some(result) = self.memory_result_by_id(&row.object_id)? {
                    if let Some(repo_filter) = repo {
                        if !result
                            .repos
                            .iter()
                            .any(|item| item.eq_ignore_ascii_case(repo_filter))
                        {
                            return Ok(None);
                        }
                    }

                    return Ok(Some(DeltaItem::Memory {
                        change_id: row.seq,
                        action: row.action.clone(),
                        object: result.object,
                        repos: result.repos,
                        session_keys: result.session_keys,
                    }));
                }

                if repo.is_some() {
                    return Ok(None);
                }

                Ok(Some(DeltaItem::Deleted {
                    change_id: row.seq,
                    object_type: row.object_type.clone(),
                    object_id: row.object_id.clone(),
                    action: row.action.clone(),
                    updated_at: row.recorded_at.clone(),
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn is_source_current(
        &self,
        source_file_path: &Path,
        source_file_mtime_ns: i64,
        source_file_size: i64,
    ) -> Result<bool> {
        let count = self.conn.query_row(
            r#"
            SELECT COUNT(*)
            FROM ingestion_state
            WHERE source_file_path = ?
              AND source_file_mtime_ns = ?
              AND source_file_size = ?
              AND content_version >= ?
            "#,
            params![
                source_file_path.display().to_string(),
                source_file_mtime_ns,
                source_file_size,
                MIN_COMPATIBLE_CONTENT_VERSION,
            ],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    pub fn mark_source_indexed(
        &self,
        source_file_path: &Path,
        source_file_mtime_ns: i64,
        source_file_size: i64,
        session_id: Option<&str>,
        session_key: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO ingestion_state (
                source_file_path, source_file_mtime_ns, source_file_size, session_id, session_key, content_version, indexed_at
            ) VALUES (?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            ON CONFLICT(source_file_path) DO UPDATE SET
                source_file_mtime_ns = excluded.source_file_mtime_ns,
                source_file_size = excluded.source_file_size,
                session_id = excluded.session_id,
                session_key = excluded.session_key,
                content_version = excluded.content_version,
                indexed_at = excluded.indexed_at
            "#,
            params![
                source_file_path.display().to_string(),
                source_file_mtime_ns,
                source_file_size,
                session_id,
                session_key,
                CONTENT_VERSION,
            ],
        )?;
        Ok(())
    }

    pub fn last_indexed_at(&self) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT MAX(indexed_at) FROM ingestion_state", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .map_err(Into::into)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            "#,
        )?;

        if self.table_exists("sessions")? && !self.table_has_column("sessions", "session_key")? {
            self.migrate_to_session_key_schema()?;
        }

        self.create_schema_objects()?;
        self.ensure_sessions_indexed_at_column()?;
        self.ensure_sessions_source_columns()?;
        self.ensure_ingestion_state_session_key_column()?;
        self.ensure_ingestion_state_content_version_column()?;
        self.backfill_session_repos()?;
        self.backfill_session_repo_memberships()?;
        self.backfill_ingestion_session_keys()?;
        Ok(())
    }

    fn create_schema_objects(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                session_key TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                session_timestamp TEXT NOT NULL,
                cwd TEXT NOT NULL,
                repo TEXT NOT NULL DEFAULT '',
                source_kind TEXT NOT NULL DEFAULT 'codex',
                source_label TEXT NOT NULL DEFAULT 'Codex',
                cli_version TEXT,
                source_file_path TEXT NOT NULL,
                indexed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            );

            CREATE INDEX IF NOT EXISTS sessions_session_id_idx ON sessions(session_id);
            CREATE INDEX IF NOT EXISTS sessions_repo_idx ON sessions(repo);

            CREATE TABLE IF NOT EXISTS session_repos (
                session_key TEXT NOT NULL,
                repo TEXT NOT NULL,
                PRIMARY KEY(session_key, repo),
                FOREIGN KEY(session_key) REFERENCES sessions(session_key) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS session_repos_repo_idx ON session_repos(repo);

            CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                role TEXT,
                text TEXT NOT NULL,
                command TEXT,
                cwd TEXT,
                exit_code INTEGER,
                source_timestamp TEXT,
                source_file_path TEXT NOT NULL,
                source_line_number INTEGER NOT NULL,
                FOREIGN KEY(session_key) REFERENCES sessions(session_key) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS events_session_key_idx ON events(session_key);
            CREATE INDEX IF NOT EXISTS events_session_id_idx ON events(session_id);
            CREATE INDEX IF NOT EXISTS events_source_idx ON events(source_file_path, source_line_number);

            CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
                event_id UNINDEXED,
                session_key UNINDEXED,
                session_id UNINDEXED,
                kind UNINDEXED,
                text,
                tokenize = 'porter unicode61'
            );

            CREATE TABLE IF NOT EXISTS ingestion_state (
                source_file_path TEXT PRIMARY KEY,
                source_file_mtime_ns INTEGER NOT NULL,
                source_file_size INTEGER NOT NULL,
                session_id TEXT,
                session_key TEXT,
                content_version INTEGER NOT NULL DEFAULT 4,
                indexed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS memory_objects (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                summary TEXT NOT NULL,
                normalized_text TEXT NOT NULL,
                first_seen_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            );

            CREATE UNIQUE INDEX IF NOT EXISTS memory_objects_kind_normalized_idx
                ON memory_objects(kind, normalized_text);
            CREATE INDEX IF NOT EXISTS memory_objects_updated_idx
                ON memory_objects(updated_at);

            CREATE TABLE IF NOT EXISTS memory_evidence (
                memory_id TEXT NOT NULL,
                session_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                source_file_path TEXT NOT NULL,
                source_line_number INTEGER NOT NULL,
                source_timestamp TEXT,
                event_kind TEXT NOT NULL,
                evidence_text TEXT NOT NULL,
                PRIMARY KEY(memory_id, source_file_path, source_line_number),
                FOREIGN KEY(memory_id) REFERENCES memory_objects(id) ON DELETE CASCADE,
                FOREIGN KEY(session_key) REFERENCES sessions(session_key) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS memory_evidence_memory_idx ON memory_evidence(memory_id);
            CREATE INDEX IF NOT EXISTS memory_evidence_session_idx ON memory_evidence(session_key);

            CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
                memory_id UNINDEXED,
                kind UNINDEXED,
                summary,
                normalized_text,
                tokenize = 'porter unicode61'
            );

            CREATE TABLE IF NOT EXISTS change_log (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                object_type TEXT NOT NULL,
                object_id TEXT NOT NULL,
                action TEXT NOT NULL,
                recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            );

            CREATE INDEX IF NOT EXISTS change_log_object_idx
                ON change_log(object_type, object_id, seq);
            "#,
        )?;
        Ok(())
    }

    fn index_session_inner(&self, parsed: &ParsedSession) -> Result<()> {
        let session_key = session_key(&parsed.session.id, &parsed.session.source_file_path);
        let repo = repo_slug(&parsed.session.cwd);
        self.conn.execute(
            r#"
            INSERT INTO sessions (
                session_key, session_id, session_timestamp, cwd, repo, source_kind, source_label,
                cli_version, source_file_path, indexed_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            ON CONFLICT(session_key) DO UPDATE SET
                session_id = excluded.session_id,
                session_timestamp = excluded.session_timestamp,
                cwd = excluded.cwd,
                repo = excluded.repo,
                source_kind = excluded.source_kind,
                source_label = excluded.source_label,
                cli_version = excluded.cli_version,
                source_file_path = excluded.source_file_path,
                indexed_at = excluded.indexed_at
            "#,
            params![
                session_key.as_str(),
                parsed.session.id,
                parsed.session.timestamp,
                parsed.session.cwd,
                repo,
                parsed.session.source_kind.as_str(),
                parsed.session.source_label,
                parsed.session.cli_version,
                parsed.session.source_file_path.display().to_string(),
            ],
        )?;
        self.record_change("session", &session_key, "upsert")?;

        let mut affected_memory_ids = self.memory_ids_for_session(&session_key)?;

        self.conn.execute(
            "DELETE FROM events_fts WHERE session_key = ?",
            params![session_key.as_str()],
        )?;
        self.conn.execute(
            "DELETE FROM events WHERE session_key = ?",
            params![session_key.as_str()],
        )?;
        self.conn.execute(
            "DELETE FROM session_repos WHERE session_key = ?",
            params![session_key.as_str()],
        )?;
        self.conn.execute(
            "DELETE FROM memory_evidence WHERE session_key = ?",
            params![session_key.as_str()],
        )?;

        for repo in session_repos(parsed) {
            self.conn.execute(
                "INSERT OR IGNORE INTO session_repos (session_key, repo) VALUES (?, ?)",
                params![session_key.as_str(), repo],
            )?;
        }

        for event in &parsed.events {
            self.conn.execute(
                r#"
                INSERT INTO events (
                    session_key, session_id, kind, role, text, command, cwd, exit_code,
                    source_timestamp, source_file_path, source_line_number
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
                params![
                    session_key.as_str(),
                    parsed.session.id,
                    event.kind.as_str(),
                    event.role,
                    event.text,
                    event.command,
                    event.cwd,
                    event.exit_code,
                    event.source_timestamp,
                    event.source_file_path.display().to_string(),
                    event.source_line_number as i64,
                ],
            )?;
            let event_id = self.conn.last_insert_rowid();
            self.conn.execute(
                "INSERT INTO events_fts (event_id, session_key, session_id, kind, text) VALUES (?, ?, ?, ?, ?)",
                params![
                    event_id,
                    session_key.as_str(),
                    parsed.session.id,
                    event.kind.as_str(),
                    event.text
                ],
            )?;
        }

        for memory in extract_memories(parsed) {
            self.upsert_memory_object(&memory, &parsed.session.timestamp)?;
            self.conn.execute(
                r#"
                INSERT OR REPLACE INTO memory_evidence (
                    memory_id, session_key, session_id, source_file_path, source_line_number,
                    source_timestamp, event_kind, evidence_text
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
                params![
                    memory.id,
                    session_key.as_str(),
                    parsed.session.id,
                    parsed.session.source_file_path.display().to_string(),
                    memory.source_line_number as i64,
                    memory.source_timestamp,
                    memory.event_kind.as_str(),
                    memory.evidence_text,
                ],
            )?;
            affected_memory_ids.push(memory.id);
        }

        affected_memory_ids.sort();
        affected_memory_ids.dedup();
        self.refresh_memory_objects(&affected_memory_ids)?;

        Ok(())
    }

    fn backfill_session_repos(&self) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("SELECT session_key, cwd FROM sessions WHERE repo = ''")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (session_key, cwd) = row?;
            self.conn.execute(
                "UPDATE sessions SET repo = ? WHERE session_key = ?",
                params![repo_slug(&cwd), session_key],
            )?;
        }
        Ok(())
    }

    fn table_exists(&self, table_name: &str) -> Result<bool> {
        let count = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
            params![table_name],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count > 0)
    }

    fn table_has_column(&self, table_name: &str, column_name: &str) -> Result<bool> {
        let mut statement = self
            .conn
            .prepare(&format!("PRAGMA table_info({table_name})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(columns.iter().any(|column| column == column_name))
    }

    fn ensure_ingestion_state_session_key_column(&self) -> Result<()> {
        if self.table_has_column("ingestion_state", "session_key")? {
            return Ok(());
        }

        match self.conn.execute(
            "ALTER TABLE ingestion_state ADD COLUMN session_key TEXT",
            [],
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.to_string().contains("duplicate column name") => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn ensure_sessions_indexed_at_column(&self) -> Result<()> {
        if self.table_has_column("sessions", "indexed_at")? {
            return Ok(());
        }

        match self.conn.execute(
            "ALTER TABLE sessions ADD COLUMN indexed_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00.000Z'",
            [],
        ) {
            Ok(_) => {
                self.conn.execute(
                    "UPDATE sessions SET indexed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE indexed_at = '1970-01-01T00:00:00.000Z'",
                    [],
                )?;
                Ok(())
            }
            Err(error) if error.to_string().contains("duplicate column name") => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn ensure_sessions_source_columns(&self) -> Result<()> {
        if !self.table_has_column("sessions", "source_kind")? {
            match self.conn.execute(
                "ALTER TABLE sessions ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'codex'",
                [],
            ) {
                Ok(_) => {}
                Err(error) if error.to_string().contains("duplicate column name") => {}
                Err(error) => return Err(error.into()),
            }
        }
        if !self.table_has_column("sessions", "source_label")? {
            match self.conn.execute(
                "ALTER TABLE sessions ADD COLUMN source_label TEXT NOT NULL DEFAULT 'Codex'",
                [],
            ) {
                Ok(_) => {}
                Err(error) if error.to_string().contains("duplicate column name") => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn ensure_ingestion_state_content_version_column(&self) -> Result<()> {
        if self.table_has_column("ingestion_state", "content_version")? {
            return Ok(());
        }

        match self.conn.execute(
            "ALTER TABLE ingestion_state ADD COLUMN content_version INTEGER NOT NULL DEFAULT 0",
            [],
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.to_string().contains("duplicate column name") => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn backfill_ingestion_session_keys(&self) -> Result<()> {
        if !self.table_exists("ingestion_state")? {
            return Ok(());
        }

        self.conn.execute(
            r#"
            UPDATE ingestion_state
            SET session_key = (
                SELECT sessions.session_key
                FROM sessions
                WHERE sessions.source_file_path = ingestion_state.source_file_path
                LIMIT 1
            )
            WHERE session_key IS NULL
              AND session_id IS NOT NULL
            "#,
            [],
        )?;
        Ok(())
    }

    fn backfill_session_repo_memberships(&self) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO session_repos (session_key, repo)
            SELECT session_key, repo
            FROM sessions
            WHERE repo != ''
            "#,
            [],
        )?;

        let mut statement = self.conn.prepare(
            r#"
            SELECT DISTINCT session_key, cwd
            FROM events
            WHERE cwd IS NOT NULL
              AND cwd != ''
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (session_key, cwd) = row?;
            let repo = repo_slug(&cwd);
            if repo.is_empty() {
                continue;
            }
            self.conn.execute(
                "INSERT OR IGNORE INTO session_repos (session_key, repo) VALUES (?, ?)",
                params![session_key, repo],
            )?;
        }

        Ok(())
    }

    fn migrate_to_session_key_schema(&self) -> Result<()> {
        let old_sessions = self.load_old_sessions()?;
        let old_events = self.load_old_events()?;
        let mut keys_by_session_id = HashMap::new();
        for session in &old_sessions {
            keys_by_session_id.insert(
                session.session_id.clone(),
                session_key(&session.session_id, Path::new(&session.source_file_path)),
            );
        }

        self.conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<()> {
            self.conn.execute_batch(
                r#"
                DROP TABLE IF EXISTS events_fts;
                DROP TABLE IF EXISTS events;
                DROP TABLE IF EXISTS sessions;
                "#,
            )?;
            self.create_schema_objects()?;

            for session in &old_sessions {
                let session_key = keys_by_session_id
                    .get(&session.session_id)
                    .expect("session key exists");
                let repo = if session.repo.is_empty() {
                    repo_slug(&session.cwd)
                } else {
                    session.repo.clone()
                };
                self.conn.execute(
                    r#"
                    INSERT INTO sessions (
                        session_key, session_id, session_timestamp, cwd, repo, source_kind, source_label,
                        cli_version, source_file_path, indexed_at
                    ) VALUES (?, ?, ?, ?, ?, 'codex', 'Codex', ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                    "#,
                    params![
                        session_key,
                        session.session_id,
                        session.session_timestamp,
                        session.cwd,
                        repo,
                        session.cli_version,
                        session.source_file_path,
                    ],
                )?;
                self.conn.execute(
                    "INSERT OR IGNORE INTO session_repos (session_key, repo) VALUES (?, ?)",
                    params![session_key, repo],
                )?;
            }

            for event in &old_events {
                let Some(session_key) = keys_by_session_id.get(&event.session_id) else {
                    continue;
                };
                self.conn.execute(
                    r#"
                    INSERT INTO events (
                        session_key, session_id, kind, role, text, command, cwd, exit_code,
                        source_timestamp, source_file_path, source_line_number
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                    params![
                        session_key,
                        event.session_id,
                        event.kind,
                        event.role,
                        event.text,
                        event.command,
                        event.cwd,
                        event.exit_code,
                        event.source_timestamp,
                        event.source_file_path,
                        event.source_line_number,
                    ],
                )?;
                let event_id = self.conn.last_insert_rowid();
                self.conn.execute(
                    "INSERT INTO events_fts (event_id, session_key, session_id, kind, text) VALUES (?, ?, ?, ?, ?)",
                    params![
                        event_id,
                        session_key,
                        event.session_id,
                        event.kind,
                        event.text
                    ],
                )?;
            }

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute("COMMIT", [])?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK", []);
                Err(error)
            }
        }
    }

    fn load_old_sessions(&self) -> Result<Vec<OldSessionRow>> {
        let has_repo = self.table_has_column("sessions", "repo")?;
        let sql = if has_repo {
            "SELECT session_id, session_timestamp, cwd, repo, cli_version, source_file_path FROM sessions"
        } else {
            "SELECT session_id, session_timestamp, cwd, '' AS repo, cli_version, source_file_path FROM sessions"
        };
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok(OldSessionRow {
                session_id: row.get(0)?,
                session_timestamp: row.get(1)?,
                cwd: row.get(2)?,
                repo: row.get(3)?,
                cli_version: row.get(4)?,
                source_file_path: row.get(5)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn load_old_events(&self) -> Result<Vec<OldEventRow>> {
        if !self.table_exists("events")? {
            return Ok(Vec::new());
        }

        let mut statement = self.conn.prepare(
            r#"
            SELECT
                session_id, kind, role, text, command, cwd, exit_code,
                source_timestamp, source_file_path, source_line_number
            FROM events
            ORDER BY id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok(OldEventRow {
                session_id: row.get(0)?,
                kind: row.get(1)?,
                role: row.get(2)?,
                text: row.get(3)?,
                command: row.get(4)?,
                cwd: row.get(5)?,
                exit_code: row.get(6)?,
                source_timestamp: row.get(7)?,
                source_file_path: row.get(8)?,
                source_line_number: row.get(9)?,
            })
        })?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

fn configure_write_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(sqlite_busy_timeout())?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        PRAGMA cache_size = -65536;
        PRAGMA mmap_size = 268435456;
        PRAGMA wal_autocheckpoint = 10000;
        "#,
    )?;
    Ok(())
}

fn configure_read_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(sqlite_busy_timeout())?;
    conn.execute_batch(
        r#"
        PRAGMA query_only = ON;
        PRAGMA temp_store = MEMORY;
        "#,
    )?;
    Ok(())
}

fn sqlite_busy_timeout() -> Duration {
    std::env::var("AGENT_RECALL_SQLITE_BUSY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(30))
}

fn append_since_clause(
    sql: &mut String,
    query_params: &mut Vec<String>,
    value: &str,
) -> Result<()> {
    append_lower_bound_clause(sql, query_params, value, "--since")
}

fn append_lower_bound_clause(
    sql: &mut String,
    query_params: &mut Vec<String>,
    value: &str,
    flag_name: &str,
) -> Result<()> {
    sql.push_str(
        " AND datetime(replace(replace(sessions.session_timestamp, 'T', ' '), 'Z', '')) >= ",
    );
    match parse_date_filter(value, flag_name)? {
        DateFilter::Absolute(value) => {
            sql.push_str("datetime(?)");
            query_params.push(value);
        }
        DateFilter::LastDays(days) => {
            sql.push_str("datetime('now', ?)");
            query_params.push(format!("-{days} days"));
        }
        DateFilter::Today => {
            sql.push_str("datetime('now', 'localtime', 'start of day', 'utc')");
        }
        DateFilter::Yesterday => {
            sql.push_str("datetime('now', 'localtime', 'start of day', '-1 day', 'utc')");
        }
    }
    Ok(())
}

fn append_until_clause(
    sql: &mut String,
    query_params: &mut Vec<String>,
    value: &str,
) -> Result<()> {
    sql.push_str(
        " AND datetime(replace(replace(sessions.session_timestamp, 'T', ' '), 'Z', '')) < ",
    );
    match parse_date_filter(value, "--until")? {
        DateFilter::Absolute(value) => {
            sql.push_str("datetime(?)");
            query_params.push(value);
        }
        DateFilter::LastDays(days) => {
            sql.push_str("datetime('now', ?)");
            query_params.push(format!("-{days} days"));
        }
        DateFilter::Today => {
            sql.push_str("datetime('now', 'localtime', 'start of day', 'utc')");
        }
        DateFilter::Yesterday => {
            sql.push_str("datetime('now', 'localtime', 'start of day', '-1 day', 'utc')");
        }
    }
    Ok(())
}

fn append_from_until_clauses(
    sql: &mut String,
    query_params: &mut Vec<String>,
    since: Option<&String>,
    from: Option<&String>,
    until: Option<&String>,
) -> Result<()> {
    if since.is_some() && from.is_some() {
        bail!("use either --since or --from, not both");
    }
    if let Some(since) = since {
        append_since_clause(sql, query_params, since)?;
    } else if let Some(from) = from {
        append_lower_bound_clause(sql, query_params, from, "--from")?;
    }
    if let Some(until) = until {
        append_until_clause(sql, query_params, until)?;
    }
    Ok(())
}

fn append_excluded_sessions_clause(
    sql: &mut String,
    query_params: &mut Vec<String>,
    excluded_sessions: &[String],
) {
    for excluded_session in excluded_sessions {
        sql.push_str(" AND sessions.session_id != ? AND sessions.session_key != ?");
        query_params.push(excluded_session.clone());
        query_params.push(excluded_session.clone());
    }
}

fn append_event_kind_clause(
    sql: &mut String,
    query_params: &mut Vec<String>,
    column_name: &str,
    kinds: &[EventKind],
) {
    if kinds.is_empty() {
        return;
    }
    sql.push_str(" AND ");
    sql.push_str(column_name);
    sql.push_str(" IN (");
    sql.push_str(&placeholders(kinds.len()));
    sql.push(')');
    append_kind_params(query_params, kinds);
}

fn append_recent_event_kind_clause(
    sql: &mut String,
    query_params: &mut Vec<String>,
    kinds: &[EventKind],
) {
    if kinds.is_empty() {
        return;
    }
    sql.push_str(
        r#"
        AND EXISTS (
            SELECT 1 FROM events kind_events
            WHERE kind_events.session_key = sessions.session_key
              AND kind_events.kind IN (
        "#,
    );
    sql.push_str(&placeholders(kinds.len()));
    sql.push_str("))");
    append_kind_params(query_params, kinds);
}

fn append_memory_filter_clauses(
    sql: &mut String,
    query_params: &mut Vec<String>,
    options: &MemorySearchOptions,
) -> Result<()> {
    append_memory_kind_clause(sql, query_params, &options.kinds);
    if let Some(repo) = &options.repo {
        sql.push_str(
            r#"
            AND EXISTS (
                SELECT 1
                FROM memory_evidence
                JOIN sessions ON sessions.session_key = memory_evidence.session_key
                WHERE memory_evidence.memory_id = memory_objects.id
                  AND lower(sessions.repo) = lower(?)
            )
            "#,
        );
        query_params.push(repo.clone());
    }
    if let Some(cwd) = &options.cwd {
        sql.push_str(
            r#"
            AND EXISTS (
                SELECT 1
                FROM memory_evidence
                JOIN sessions ON sessions.session_key = memory_evidence.session_key
                WHERE memory_evidence.memory_id = memory_objects.id
                  AND sessions.cwd LIKE '%' || ? || '%'
            )
            "#,
        );
        query_params.push(cwd.clone());
    }
    if options.since.is_some() || options.from.is_some() || options.until.is_some() {
        let mut time_clause = String::new();
        let mut time_params = Vec::new();
        append_from_until_clauses(
            &mut time_clause,
            &mut time_params,
            options.since.as_ref(),
            options.from.as_ref(),
            options.until.as_ref(),
        )?;
        if !time_clause.is_empty() {
            let clause = time_clause
                .replacen(" AND ", "", 1)
                .replace("sessions.", "related_sessions.");
            sql.push_str(
                r#"
                AND EXISTS (
                    SELECT 1
                    FROM memory_evidence
                    JOIN sessions AS related_sessions ON related_sessions.session_key = memory_evidence.session_key
                    WHERE memory_evidence.memory_id = memory_objects.id
                "#,
            );
            sql.push_str(" AND ");
            sql.push_str(&clause);
            sql.push(')');
            query_params.extend(time_params);
        }
    }
    Ok(())
}

fn append_memory_kind_clause(
    sql: &mut String,
    query_params: &mut Vec<String>,
    kinds: &[MemoryKind],
) {
    if kinds.is_empty() {
        return;
    }
    sql.push_str(" AND memory_objects.kind IN (");
    sql.push_str(&placeholders(kinds.len()));
    sql.push(')');
    query_params.extend(kinds.iter().map(|kind| kind.as_str().to_owned()));
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeltaCursor {
    change_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangeLogRow {
    seq: i64,
    object_type: String,
    object_id: String,
    action: String,
    recorded_at: String,
}

fn parse_delta_cursor(value: &str) -> Option<DeltaCursor> {
    let change_id = value.strip_prefix("chg_")?.parse::<i64>().ok()?;
    Some(DeltaCursor { change_id })
}

pub fn encode_delta_cursor(item: &DeltaItem) -> String {
    format!("chg_{}", item.change_id())
}

fn append_kind_params(query_params: &mut Vec<String>, kinds: &[EventKind]) {
    query_params.extend(kinds.iter().map(|kind| kind.as_str().to_owned()));
}

struct SessionGroup {
    session_key: String,
    session_id: String,
    source_file_path: PathBuf,
    repo_matches_current: bool,
    hit_count: usize,
    best_score: f64,
    best_kind_weight: u8,
    session_timestamp: String,
    results: Vec<SearchResult>,
    source_kind: String,
}

fn source_kind_weight(source_kind: &str) -> u8 {
    if source_kind == "codex-log" {
        1
    } else {
        0
    }
}

fn rank_search_results(
    results: Vec<SearchResult>,
    _current_repo: Option<&str>,
    limit: usize,
    include_duplicates: bool,
) -> Vec<SearchResult> {
    let mut groups = Vec::<SessionGroup>::new();

    for result in results {
        let kind_weight = event_kind_weight(result.kind);
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.session_key == result.session_key)
        {
            group.hit_count += 1;
            group.best_score = group.best_score.min(result.score);
            group.best_kind_weight = group.best_kind_weight.min(kind_weight);
            group.results.push(result);
        } else {
            groups.push(SessionGroup {
                session_key: result.session_key.clone(),
                session_id: result.session_id.clone(),
                source_file_path: result.source_file_path.clone(),
                source_kind: result.source_kind.clone(),
                repo_matches_current: result.repo_matches_current,
                hit_count: 1,
                best_score: result.score,
                best_kind_weight: kind_weight,
                session_timestamp: result.session_timestamp.clone(),
                results: vec![result],
            });
        }
    }

    if !include_duplicates {
        groups = dedupe_session_groups(groups);
    }

    groups.sort_by(|left, right| {
        right
            .repo_matches_current
            .cmp(&left.repo_matches_current)
            .then_with(|| {
                source_kind_weight(&left.source_kind).cmp(&source_kind_weight(&right.source_kind))
            })
            .then_with(|| right.hit_count.cmp(&left.hit_count))
            .then_with(|| left.best_kind_weight.cmp(&right.best_kind_weight))
            .then_with(|| {
                left.best_score
                    .partial_cmp(&right.best_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| right.session_timestamp.cmp(&left.session_timestamp))
            .then_with(|| left.session_key.cmp(&right.session_key))
    });

    let mut ranked = Vec::new();
    for mut group in groups {
        group.results.sort_by(|left, right| {
            left.score
                .partial_cmp(&right.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| event_kind_weight(left.kind).cmp(&event_kind_weight(right.kind)))
                .then_with(|| left.source_line_number.cmp(&right.source_line_number))
        });
        for result in &mut group.results {
            result.session_hit_count = group.hit_count;
            result.best_kind_weight = group.best_kind_weight;
        }
        ranked.extend(group.results);
        if ranked.len() >= limit {
            ranked.truncate(limit);
            break;
        }
    }

    ranked
}

fn dedupe_session_groups(groups: Vec<SessionGroup>) -> Vec<SessionGroup> {
    let mut selected = HashMap::<String, SessionGroup>::new();
    for group in groups {
        let dedupe_key = format!("{}\u{1f}{}", group.source_kind, group.session_id);
        match selected.entry(dedupe_key) {
            Entry::Occupied(mut entry) => {
                if is_preferred_group(&group, entry.get()) {
                    entry.insert(group);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(group);
            }
        }
    }
    selected.into_values().collect()
}

fn is_preferred_group(candidate: &SessionGroup, current: &SessionGroup) -> bool {
    let candidate_priority = source_priority(&candidate.source_file_path);
    let current_priority = source_priority(&current.source_file_path);
    candidate_priority < current_priority
        || (candidate_priority == current_priority
            && candidate.repo_matches_current
            && !current.repo_matches_current)
        || (candidate_priority == current_priority
            && candidate.repo_matches_current == current.repo_matches_current
            && candidate.session_timestamp > current.session_timestamp)
        || (candidate_priority == current_priority
            && candidate.repo_matches_current == current.repo_matches_current
            && candidate.session_timestamp == current.session_timestamp
            && candidate.session_key < current.session_key)
}

fn dedupe_recent_sessions(sessions: Vec<RecentSession>) -> Vec<RecentSession> {
    let mut selected = HashMap::<String, RecentSession>::new();
    for session in sessions {
        let dedupe_key = format!("{}\u{1f}{}", session.source_kind, session.session_id);
        match selected.entry(dedupe_key) {
            Entry::Occupied(mut entry) => {
                if is_preferred_recent_session(&session, entry.get()) {
                    entry.insert(session);
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(session);
            }
        }
    }
    let mut sessions = selected.into_values().collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        right
            .session_timestamp
            .cmp(&left.session_timestamp)
            .then_with(|| {
                source_priority(&left.source_file_path)
                    .cmp(&source_priority(&right.source_file_path))
            })
            .then_with(|| left.session_key.cmp(&right.session_key))
    });
    sessions
}

fn is_preferred_recent_session(candidate: &RecentSession, current: &RecentSession) -> bool {
    let candidate_priority = source_priority(&candidate.source_file_path);
    let current_priority = source_priority(&current.source_file_path);
    candidate_priority < current_priority
        || (candidate_priority == current_priority
            && candidate.session_timestamp > current.session_timestamp)
        || (candidate_priority == current_priority
            && candidate.session_timestamp == current.session_timestamp
            && candidate.session_key < current.session_key)
}

fn source_priority(path: &Path) -> u8 {
    if path
        .components()
        .any(|component| component.as_os_str() == "archived_logs")
    {
        return 3;
    }
    if path
        .components()
        .any(|component| component.as_os_str() == "archived_sessions")
    {
        return 2;
    }
    if path
        .components()
        .any(|component| component.as_os_str() == "sessions")
    {
        return 0;
    }
    1
}

pub fn source_priority_for_path(path: &Path) -> u8 {
    source_priority(path)
}

fn event_kind_weight(kind: EventKind) -> u8 {
    match kind {
        EventKind::UserMessage => 0,
        EventKind::AssistantMessage => 1,
        EventKind::Command => 2,
        EventKind::Tool => 3,
    }
}

fn session_repos(parsed: &ParsedSession) -> BTreeSet<String> {
    let mut repos = BTreeSet::new();
    let session_repo = repo_slug(&parsed.session.cwd);
    if !session_repo.is_empty() {
        repos.insert(session_repo);
    }

    for event in &parsed.events {
        let Some(cwd) = &event.cwd else {
            continue;
        };
        let repo = repo_slug(cwd);
        if !repo.is_empty() {
            repos.insert(repo);
        }
    }

    repos
}

fn fts_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();

    for ch in query.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            terms.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        terms.push(current);
    }

    terms
}

fn quote_fts_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn and_fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| quote_fts_term(term))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn or_fts_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| quote_fts_term(term))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn phrase_fts_query(terms: &[String]) -> String {
    quote_fts_term(&terms.join(" "))
}

fn near_fts_query(terms: &[String], distance: u32) -> String {
    if terms.len() <= 1 {
        return and_fts_query(terms);
    }

    format!(
        "NEAR({}, {distance})",
        terms
            .iter()
            .map(|term| quote_fts_term(term))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

pub fn build_session_key(session_id: &str, source_file_path: &Path) -> String {
    session_key(session_id, source_file_path)
}

fn session_key(session_id: &str, source_file_path: &Path) -> String {
    format!(
        "{}:{:016x}",
        session_id,
        fnv1a64(source_file_path.display().to_string().as_bytes())
    )
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(crate) fn repo_slug(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(cwd)
        .to_owned()
}

#[cfg(test)]
mod tests;
