use crate::parser::{EventKind, ParsedEvent, ParsedSession, SessionMetadata, SourceKind};
use crate::redact::redact_secrets;
use crate::store::{build_session_key, Store};
use anyhow::{Context, Result};
use regex::Regex;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

const MESSAGE_TEXT_LIMIT: usize = 20_000;
const LOG_PROGRESS_ROW_INTERVAL: usize = 5_000;
const LOG_PROGRESS_SESSION_INTERVAL: usize = 25;
const TOOL_TEXT_LIMIT: usize = 4_000;

static THREAD_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"thread_id=([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})")
        .expect("thread id regex compiles")
});

static JSONL_SESSION_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
        .expect("session id regex compiles")
});

#[derive(Debug, Clone, Default)]
pub struct LogImportOptions {
    pub force: bool,
    pub known_jsonl_session_ids: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogImportPhase {
    Starting,
    Scanning,
    Indexing,
    SkippedCurrent,
    SkippedMissing,
    Done,
}

impl LogImportPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Scanning => "scanning",
            Self::Indexing => "indexing",
            Self::SkippedCurrent => "skipped-current",
            Self::SkippedMissing => "skipped-missing",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogImportProgress {
    pub sources_total: usize,
    pub sources_seen: usize,
    pub current_source: Option<PathBuf>,
    pub phase: LogImportPhase,
    pub rows_seen: usize,
    pub rows_total: Option<usize>,
    pub threads_seen: usize,
    pub skipped_duplicate_threads: usize,
    pub sessions_parsed: usize,
    pub sessions_indexed: usize,
    pub events_indexed: usize,
}

impl LogImportProgress {
    fn new(sources_total: usize) -> Self {
        Self {
            sources_total,
            sources_seen: 0,
            current_source: None,
            phase: LogImportPhase::Starting,
            rows_seen: 0,
            rows_total: None,
            threads_seen: 0,
            skipped_duplicate_threads: 0,
            sessions_parsed: 0,
            sessions_indexed: 0,
            events_indexed: 0,
        }
    }

    fn reset_for_source(&mut self, source: &Path) {
        self.current_source = Some(source.to_path_buf());
        self.phase = LogImportPhase::Starting;
        self.rows_seen = 0;
        self.rows_total = None;
        self.threads_seen = 0;
        self.skipped_duplicate_threads = 0;
        self.sessions_parsed = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LogSourceScanProgress {
    rows_seen: usize,
    rows_total: usize,
    threads_seen: usize,
    skipped_duplicate_threads: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogImportReport {
    pub sources_total: usize,
    pub sources_seen: usize,
    pub sources_indexed: usize,
    pub skipped_current: usize,
    pub skipped_missing: usize,
    pub skipped_duplicate_threads: usize,
    pub sessions_deleted: usize,
    pub sessions_indexed: usize,
    pub events_indexed: usize,
}

#[derive(Debug)]
struct ParsedLogSource {
    sessions: Vec<ParsedSession>,
    skipped_duplicate_threads: usize,
}

#[derive(Debug)]
struct LogThread {
    id: String,
    timestamp: String,
    events: Vec<ParsedEvent>,
    seen_events: HashSet<String>,
    delta_buffers: HashMap<String, DeltaBuffer>,
}

#[derive(Debug, Default)]
struct DeltaBuffer {
    text: String,
    source_timestamp: Option<String>,
    source_line_number: usize,
}

#[derive(Debug)]
struct RowContext<'a> {
    source_file_path: &'a Path,
    source_timestamp: Option<&'a str>,
    source_line_number: usize,
}

#[derive(Debug)]
struct FileState {
    source_file_mtime_ns: i64,
    source_file_size: i64,
}

impl FileState {
    fn from_path(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        let modified = metadata
            .modified()
            .with_context(|| format!("read mtime {}", path.display()))?;
        let source_file_mtime_ns = modified
            .duration_since(UNIX_EPOCH)
            .with_context(|| format!("mtime before unix epoch {}", path.display()))?
            .as_nanos() as i64;

        Ok(Self {
            source_file_mtime_ns,
            source_file_size: metadata.len() as i64,
        })
    }
}

impl LogThread {
    fn new(id: String, timestamp: String) -> Self {
        Self {
            id,
            timestamp,
            events: Vec::new(),
            seen_events: HashSet::new(),
            delta_buffers: HashMap::new(),
        }
    }

    fn push_text_event(
        &mut self,
        kind: EventKind,
        role: Option<&str>,
        text: &str,
        context: &RowContext<'_>,
    ) {
        let Some(text) = normalize_event_text(text, text_limit_for_kind(kind)) else {
            return;
        };
        let dedupe_key = format!(
            "{}\u{1f}{}\u{1f}{}",
            kind.as_str(),
            role.unwrap_or_default(),
            text
        );
        if !self.seen_events.insert(dedupe_key) {
            return;
        }

        self.events.push(ParsedEvent {
            session_id: self.id.clone(),
            kind,
            role: role.map(str::to_owned),
            text,
            command: None,
            cwd: None,
            exit_code: None,
            source_timestamp: context.source_timestamp.map(str::to_owned),
            source_file_path: context.source_file_path.to_path_buf(),
            source_line_number: context.source_line_number,
        });
    }

    fn append_delta(&mut self, value: &Value, context: &RowContext<'_>) {
        let Some(delta) = value.get("delta").and_then(Value::as_str) else {
            return;
        };
        if delta.is_empty() {
            return;
        }

        let key = value
            .get("item_id")
            .or_else(|| value.get("output_index"))
            .map(stable_json_key)
            .unwrap_or_else(|| "assistant-delta".to_owned());
        let buffer = self
            .delta_buffers
            .entry(key)
            .or_insert_with(|| DeltaBuffer {
                text: String::new(),
                source_timestamp: context.source_timestamp.map(str::to_owned),
                source_line_number: context.source_line_number,
            });
        buffer.text.push_str(delta);
    }

    fn into_parsed_session(mut self, source_file_path: &Path) -> Option<ParsedSession> {
        let buffers = std::mem::take(&mut self.delta_buffers);
        for buffer in buffers.into_values() {
            let context = RowContext {
                source_file_path,
                source_timestamp: buffer.source_timestamp.as_deref(),
                source_line_number: buffer.source_line_number,
            };
            self.push_text_event(
                EventKind::AssistantMessage,
                Some("assistant"),
                &buffer.text,
                &context,
            );
        }

        if self.events.is_empty() {
            return None;
        }

        Some(ParsedSession {
            session: SessionMetadata {
                id: self.id,
                timestamp: self.timestamp,
                cwd: String::new(),
                cli_version: None,
                source_file_path: source_file_path.to_path_buf(),
                source_kind: SourceKind::CodexLog,
                source_label: SourceKind::CodexLog.label().to_owned(),
            },
            events: self.events,
        })
    }
}

pub fn index_codex_log_sources(
    store: &Store,
    sources: &[PathBuf],
    options: &LogImportOptions,
) -> Result<LogImportReport> {
    index_codex_log_sources_with_progress(store, sources, options, |_| {})
}

pub fn index_codex_log_sources_with_progress<F>(
    store: &Store,
    sources: &[PathBuf],
    options: &LogImportOptions,
    mut on_progress: F,
) -> Result<LogImportReport>
where
    F: FnMut(&LogImportProgress),
{
    let mut report = LogImportReport {
        sources_total: sources.len(),
        ..LogImportReport::default()
    };
    let mut progress = LogImportProgress::new(sources.len());
    on_progress(&progress);

    for source in sources {
        report.sources_seen += 1;
        progress.sources_seen = report.sources_seen;
        progress.reset_for_source(source);
        on_progress(&progress);

        let file_state = match FileState::from_path(source) {
            Ok(file_state) => file_state,
            Err(error) if is_not_found_error(&error) => {
                report.skipped_missing += 1;
                progress.phase = LogImportPhase::SkippedMissing;
                on_progress(&progress);
                continue;
            }
            Err(error) => return Err(error),
        };

        if !options.force
            && store.is_source_current(
                source,
                file_state.source_file_mtime_ns,
                file_state.source_file_size,
            )?
        {
            report.skipped_current += 1;
            progress.phase = LogImportPhase::SkippedCurrent;
            on_progress(&progress);
            continue;
        }

        let parsed = parse_codex_log_source_with_progress(
            source,
            &options.known_jsonl_session_ids,
            |scan| {
                progress.phase = LogImportPhase::Scanning;
                progress.rows_seen = scan.rows_seen;
                progress.rows_total = Some(scan.rows_total);
                progress.threads_seen = scan.threads_seen;
                progress.skipped_duplicate_threads = scan.skipped_duplicate_threads;
                on_progress(&progress);
            },
        )?;
        report.skipped_duplicate_threads += parsed.skipped_duplicate_threads;

        let source_sessions_total = parsed.sessions.len();
        progress.phase = LogImportPhase::Indexing;
        progress.sessions_parsed = source_sessions_total;
        on_progress(&progress);

        store.begin_index_batch()?;
        let indexing_result = (|| -> Result<()> {
            let deleted =
                store.delete_sessions_for_source_kind(source, SourceKind::CodexLog.as_str())?;
            let mut source_sessions_indexed = 0usize;
            report.sessions_deleted += deleted;
            for parsed_session in parsed.sessions {
                let session_key = build_session_key(
                    &parsed_session.session.id,
                    &parsed_session.session.source_file_path,
                );
                report.events_indexed += parsed_session.events.len();
                store.index_session_in_batch(&parsed_session)?;
                report.sessions_indexed += 1;
                source_sessions_indexed += 1;
                store.mark_source_indexed(
                    source,
                    file_state.source_file_mtime_ns,
                    file_state.source_file_size,
                    Some(&parsed_session.session.id),
                    Some(&session_key),
                )?;
                if should_report_log_sessions(source_sessions_indexed, source_sessions_total) {
                    progress.sessions_indexed = report.sessions_indexed;
                    progress.events_indexed = report.events_indexed;
                    on_progress(&progress);
                }
            }
            if source_sessions_indexed == 0 {
                store.mark_source_indexed(
                    source,
                    file_state.source_file_mtime_ns,
                    file_state.source_file_size,
                    None,
                    None,
                )?;
            }
            Ok(())
        })();

        match indexing_result {
            Ok(()) => store.commit_index_batch()?,
            Err(error) => {
                let _ = store.rollback_index_batch();
                return Err(error);
            }
        }

        report.sources_indexed += 1;
        progress.phase = LogImportPhase::Done;
        progress.sessions_indexed = report.sessions_indexed;
        progress.events_indexed = report.events_indexed;
        on_progress(&progress);
    }

    Ok(report)
}

pub fn collect_codex_jsonl_session_ids(roots: &[PathBuf]) -> Result<HashSet<String>> {
    let mut session_ids = HashSet::new();
    for root in roots {
        collect_codex_jsonl_session_ids_from_path(root, &mut session_ids)?;
    }
    Ok(session_ids)
}

fn collect_codex_jsonl_session_ids_from_path(
    path: &Path,
    session_ids: &mut HashSet<String>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        collect_session_id_from_jsonl_filename(path, session_ids);
        return Ok(());
    }

    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", entry_path.display()))?;
        if file_type.is_dir() {
            collect_codex_jsonl_session_ids_from_path(&entry_path, session_ids)?;
        } else if file_type.is_file() {
            collect_session_id_from_jsonl_filename(&entry_path, session_ids);
        }
    }

    Ok(())
}

fn collect_session_id_from_jsonl_filename(path: &Path, session_ids: &mut HashSet<String>) {
    if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
        return;
    }
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    if let Some(captures) = JSONL_SESSION_ID_RE.find(file_name) {
        session_ids.insert(captures.as_str().to_owned());
    }
}

fn parse_codex_log_source_with_progress<F>(
    source: &Path,
    known_jsonl_session_ids: &HashSet<String>,
    mut on_progress: F,
) -> Result<ParsedLogSource>
where
    F: FnMut(LogSourceScanProgress),
{
    let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open Codex log database {}", source.display()))?;
    let rows_total = count_log_rows(&conn)?;
    let mut statement = conn.prepare(
        r#"
        SELECT id, strftime('%Y-%m-%dT%H:%M:%SZ', ts, 'unixepoch') AS source_timestamp, feedback_log_body
        FROM logs
        WHERE feedback_log_body LIKE '%thread_id=%'
        ORDER BY ts ASC, id ASC
        "#,
    )?;
    let mut rows = statement.query([])?;
    let mut rows_seen = 0usize;
    let mut threads = HashMap::<String, LogThread>::new();
    let mut skipped_duplicate_threads = HashSet::<String>::new();
    on_progress(LogSourceScanProgress {
        rows_seen,
        rows_total,
        threads_seen: threads.len(),
        skipped_duplicate_threads: skipped_duplicate_threads.len(),
    });

    while let Some(row) = rows.next()? {
        rows_seen += 1;
        let row_id: i64 = row.get(0)?;
        let source_timestamp: Option<String> = row.get(1)?;
        let body: String = row.get(2)?;
        let thread_ids = extract_thread_ids(&body);
        if thread_ids.is_empty() {
            if should_report_log_rows(rows_seen, rows_total) {
                on_progress(LogSourceScanProgress {
                    rows_seen,
                    rows_total,
                    threads_seen: threads.len(),
                    skipped_duplicate_threads: skipped_duplicate_threads.len(),
                });
            }
            continue;
        }

        let eligible_thread_ids = thread_ids
            .into_iter()
            .filter(|thread_id| {
                if known_jsonl_session_ids.contains(thread_id) {
                    skipped_duplicate_threads.insert(thread_id.clone());
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();
        if eligible_thread_ids.is_empty() {
            if should_report_log_rows(rows_seen, rows_total) {
                on_progress(LogSourceScanProgress {
                    rows_seen,
                    rows_total,
                    threads_seen: threads.len(),
                    skipped_duplicate_threads: skipped_duplicate_threads.len(),
                });
            }
            continue;
        }

        let source_line_number = usize::try_from(row_id).unwrap_or(usize::MAX);
        let context = RowContext {
            source_file_path: source,
            source_timestamp: source_timestamp.as_deref(),
            source_line_number,
        };
        for thread_id in eligible_thread_ids {
            let thread = threads.entry(thread_id.clone()).or_insert_with(|| {
                LogThread::new(thread_id, source_timestamp.clone().unwrap_or_default())
            });
            parse_json_values_from_body(thread, &body, &context);
        }
        if should_report_log_rows(rows_seen, rows_total) {
            on_progress(LogSourceScanProgress {
                rows_seen,
                rows_total,
                threads_seen: threads.len(),
                skipped_duplicate_threads: skipped_duplicate_threads.len(),
            });
        }
    }

    let mut sessions = threads
        .into_values()
        .filter_map(|thread| thread.into_parsed_session(source))
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.session.id.cmp(&right.session.id));

    Ok(ParsedLogSource {
        sessions,
        skipped_duplicate_threads: skipped_duplicate_threads.len(),
    })
}

fn count_log_rows(conn: &Connection) -> Result<usize> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM logs WHERE feedback_log_body LIKE '%thread_id=%'",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

fn should_report_log_rows(rows_seen: usize, rows_total: usize) -> bool {
    rows_seen == 0
        || rows_seen == 1
        || rows_seen.is_multiple_of(LOG_PROGRESS_ROW_INTERVAL)
        || rows_seen == rows_total
}

fn should_report_log_sessions(sessions_indexed: usize, sessions_total: usize) -> bool {
    sessions_indexed == 1
        || sessions_indexed.is_multiple_of(LOG_PROGRESS_SESSION_INTERVAL)
        || sessions_indexed == sessions_total
}

fn parse_json_values_from_body(thread: &mut LogThread, body: &str, context: &RowContext<'_>) {
    let mut offset = 0usize;
    while let Some(relative_start) = body[offset..].find('{') {
        let start = offset + relative_start;
        let mut values = serde_json::Deserializer::from_str(&body[start..]).into_iter::<Value>();
        match values.next() {
            Some(Ok(value)) => {
                handle_json_value(thread, &value, context);
                offset = start + values.byte_offset().max(1);
            }
            Some(Err(_)) | None => {
                offset = start + 1;
            }
        }
    }
}

fn handle_json_value(thread: &mut LogThread, value: &Value, context: &RowContext<'_>) {
    match value.get("type").and_then(Value::as_str) {
        Some("response.create") => handle_response_create(thread, value, context),
        Some("response.output_item.done") => {
            if let Some(item) = value.get("item") {
                handle_output_item(thread, item, context);
            }
        }
        Some("response.completed") => handle_response_completed(thread, value, context),
        Some("response.output_text.delta") => thread.append_delta(value, context),
        _ => {}
    }
}

fn handle_response_create(thread: &mut LogThread, value: &Value, context: &RowContext<'_>) {
    let Some(input) = value.get("input") else {
        return;
    };

    match input {
        Value::Array(items) => {
            for item in items {
                handle_request_input_item(thread, item, context);
            }
        }
        Value::String(text) => {
            thread.push_text_event(EventKind::UserMessage, Some("user"), text, context);
        }
        _ => {}
    }
}

fn handle_request_input_item(thread: &mut LogThread, item: &Value, context: &RowContext<'_>) {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    if item_type == "function_call_output"
        || item.get("call_id").is_some() && item.get("output").is_some()
    {
        if let Some(output) = item.get("output").and_then(Value::as_str) {
            thread.push_text_event(EventKind::Tool, Some("tool"), output, context);
        }
        return;
    }

    let role = item.get("role").and_then(Value::as_str).unwrap_or_default();
    let kind = match role {
        "user" => EventKind::UserMessage,
        "assistant" => EventKind::AssistantMessage,
        _ => return,
    };
    let Some(text) = content_text(item.get("content").unwrap_or(&Value::Null)) else {
        return;
    };
    thread.push_text_event(kind, Some(role), &text, context);
}

fn handle_response_completed(thread: &mut LogThread, value: &Value, context: &RowContext<'_>) {
    let output = value
        .get("response")
        .and_then(|response| response.get("output"))
        .or_else(|| value.get("output"));
    let Some(Value::Array(items)) = output else {
        return;
    };
    for item in items {
        handle_output_item(thread, item, context);
    }
}

fn handle_output_item(thread: &mut LogThread, item: &Value, context: &RowContext<'_>) {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    if item_type != "message" {
        return;
    }
    let role = item
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    if role != "assistant" {
        return;
    }
    let Some(text) = content_text(item.get("content").unwrap_or(&Value::Null)) else {
        return;
    };
    thread.push_text_event(
        EventKind::AssistantMessage,
        Some("assistant"),
        &text,
        context,
    );
}

fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty_owned(text),
        Value::Array(parts) => {
            let texts = parts
                .iter()
                .filter_map(content_part_text)
                .collect::<Vec<_>>();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
        Value::Object(_) => content_part_text(value),
        _ => None,
    }
}

fn content_part_text(part: &Value) -> Option<String> {
    if let Some(text) = part.as_str() {
        return non_empty_owned(text);
    }
    let object = part.as_object()?;
    if let Some(text) = object.get("text").and_then(Value::as_str) {
        return non_empty_owned(text);
    }
    if let Some(text) = object.get("content").and_then(Value::as_str) {
        return non_empty_owned(text);
    }
    if let Some(content) = object.get("content") {
        return content_text(content);
    }
    None
}

fn normalize_event_text(text: &str, limit: usize) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || is_codex_preamble(text) {
        return None;
    }

    let redaction_window = limit + 512;
    let capped_text = cap_text(text, redaction_window);
    non_empty_owned(&cap_text(&redact_secrets(&capped_text), limit))
}

fn text_limit_for_kind(kind: EventKind) -> usize {
    if kind == EventKind::Tool {
        TOOL_TEXT_LIMIT
    } else {
        MESSAGE_TEXT_LIMIT
    }
}

fn is_codex_preamble(text: &str) -> bool {
    text.starts_with("# AGENTS.md instructions") && text.contains("<environment_context>")
}

fn cap_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }

    let mut capped = text
        .char_indices()
        .take_while(|(index, _)| *index < limit)
        .map(|(_, ch)| ch)
        .collect::<String>();
    capped.push_str("\n[truncated]");
    capped
}

fn non_empty_owned(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

fn extract_thread_ids(body: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    THREAD_ID_RE
        .captures_iter(body)
        .filter_map(|captures| captures.get(1).map(|match_| match_.as_str().to_owned()))
        .filter(|thread_id| seen.insert(thread_id.clone()))
        .collect()
}

fn stable_json_key(value: &Value) -> String {
    match value {
        Value::String(value) => value.to_owned(),
        Value::Number(value) => value.to_string(),
        _ => value.to_string(),
    }
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use rusqlite::params;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agent-recall-{name}-{}-{nanos}.sqlite",
            std::process::id()
        ))
    }

    fn create_log_db(path: &Path, thread_id: &str) {
        let conn = Connection::open(path).expect("open temp log db");
        conn.execute_batch(
            r#"
            CREATE TABLE logs (
                id INTEGER PRIMARY KEY,
                ts INTEGER NOT NULL,
                feedback_log_body TEXT NOT NULL
            );
            "#,
        )
        .expect("create logs table");

        let request = json!({
            "type": "response.create",
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "Do not index developer instructions"}]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Unique archived log question"}]
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "Unique archived tool output"
                }
            ]
        });
        let done = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Unique archived log answer"}]
            }
        });

        conn.execute(
            "INSERT INTO logs (id, ts, feedback_log_body) VALUES (?, ?, ?)",
            params![
                1_i64,
                1_777_000_000_i64,
                format!("session_loop{{thread_id={thread_id}}}: request: {request}")
            ],
        )
        .expect("insert request row");
        conn.execute(
            "INSERT INTO logs (id, ts, feedback_log_body) VALUES (?, ?, ?)",
            params![
                2_i64,
                1_777_000_001_i64,
                format!("session_loop{{thread_id={thread_id}}}: SSE event: {done}")
            ],
        )
        .expect("insert done row");
    }

    #[test]
    fn reports_progress_while_scanning_and_indexing_codex_logs() {
        let log_path = temp_path("progress-logs-source");
        let db_path = temp_path("progress-index");
        let thread_id = "019dc0ec-1aae-7ea2-bd62-6536ca7f8e2f";
        create_log_db(&log_path, thread_id);
        let store = Store::open(&db_path).expect("open store");
        let mut progress_events = Vec::new();

        let report = index_codex_log_sources_with_progress(
            &store,
            std::slice::from_ref(&log_path),
            &LogImportOptions::default(),
            |progress| progress_events.push(progress.clone()),
        )
        .expect("index logs with progress");

        assert_eq!(report.sessions_indexed, 1);
        assert!(progress_events.iter().any(|progress| {
            progress.phase == LogImportPhase::Scanning
                && progress.rows_seen == 2
                && progress.rows_total == Some(2)
                && progress.threads_seen == 1
        }));
        assert!(progress_events.iter().any(|progress| {
            progress.phase == LogImportPhase::Indexing
                && progress.sessions_parsed == 1
                && progress.sessions_indexed == 1
        }));
        assert!(progress_events
            .iter()
            .any(|progress| progress.phase == LogImportPhase::Done));

        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(log_path);
    }

    #[test]
    fn imports_recoverable_codex_log_events_with_source_label() {
        let log_path = temp_path("logs-source");
        let db_path = temp_path("index");
        let thread_id = "019dc0ec-1aae-7ea2-bd62-6536ca7f8e2f";
        create_log_db(&log_path, thread_id);
        let store = Store::open(&db_path).expect("open store");

        let report = index_codex_log_sources(
            &store,
            std::slice::from_ref(&log_path),
            &LogImportOptions::default(),
        )
        .expect("index logs");

        assert_eq!(report.sources_indexed, 1);
        assert_eq!(report.sessions_indexed, 1);
        assert_eq!(report.events_indexed, 3);

        let results = store
            .search("Unique archived log question", 5)
            .expect("search imported log");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, thread_id);
        assert_eq!(results[0].source_kind, "codex-log");
        assert_eq!(results[0].source_label, "Codex Log");
        assert_eq!(results[0].source_file_path, log_path);
        assert_eq!(results[0].source_line_number, 1);

        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(log_path);
    }

    #[test]
    fn skips_threads_already_present_as_jsonl_sessions() {
        let log_path = temp_path("duplicate-logs-source");
        let db_path = temp_path("duplicate-index");
        let thread_id = "019dc0ec-1aae-7ea2-bd62-6536ca7f8e2f";
        create_log_db(&log_path, thread_id);
        let store = Store::open(&db_path).expect("open store");
        let mut known_jsonl_session_ids = HashSet::new();
        known_jsonl_session_ids.insert(thread_id.to_owned());

        let report = index_codex_log_sources(
            &store,
            std::slice::from_ref(&log_path),
            &LogImportOptions {
                force: false,
                known_jsonl_session_ids,
            },
        )
        .expect("index logs");

        assert_eq!(report.sources_indexed, 1);
        assert_eq!(report.sessions_indexed, 0);
        assert_eq!(report.skipped_duplicate_threads, 1);
        assert!(store
            .search("Unique archived log question", 5)
            .expect("search skipped log")
            .is_empty());

        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(log_path);
    }
}
