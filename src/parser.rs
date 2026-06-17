use crate::redact::redact_secrets;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const COMMAND_OUTPUT_LIMIT: usize = 4_000;
const COMMAND_OUTPUT_REDACTION_WINDOW: usize = COMMAND_OUTPUT_LIMIT + 512;
const MESSAGE_TEXT_LIMIT: usize = 20_000;
const MESSAGE_TEXT_REDACTION_WINDOW: usize = MESSAGE_TEXT_LIMIT + 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSession {
    pub session: SessionMetadata,
    pub events: Vec<ParsedEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata {
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    pub cli_version: Option<String>,
    pub source_file_path: PathBuf,
    pub source_kind: SourceKind,
    pub source_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Codex,
    CodexLog,
    Omp,
    Pi,
    Claude,
    Unknown,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Codex => "codex",
            SourceKind::CodexLog => "codex-log",
            SourceKind::Omp => "omp",
            SourceKind::Pi => "pi",
            SourceKind::Claude => "claude",
            SourceKind::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Codex => "Codex",
            SourceKind::CodexLog => "Codex Log",
            SourceKind::Omp => "OMP",
            SourceKind::Pi => "Pi",
            SourceKind::Claude => "Claude",
            SourceKind::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub session_id: String,
    pub kind: EventKind,
    pub role: Option<String>,
    pub text: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i64>,
    pub source_timestamp: Option<String>,
    pub source_file_path: PathBuf,
    pub source_line_number: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    UserMessage,
    AssistantMessage,
    Command,
    Tool,
}

#[derive(Debug, Deserialize)]
struct CompactRecord {
    #[serde(rename = "type")]
    type_name: Option<String>,
    timestamp: Option<String>,
    payload: Option<Value>,
    id: Option<String>,
    cwd: Option<String>,
    project: Option<String>,
    #[serde(rename = "sessionId")]
    session_id_camel: Option<String>,
    session_id: Option<String>,
    message: Option<CompactMessage>,
}

#[derive(Debug, Deserialize)]
struct CompactMessage {
    role: Option<String>,
    content: Option<CompactContent>,
    #[serde(rename = "toolName")]
    tool_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CompactContent {
    Text(String),
    Parts(Vec<CompactContentPart>),
}

#[derive(Debug, Deserialize)]
struct CompactContentPart {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    content: Option<Box<CompactContent>>,
    name: Option<String>,
    tool_name: Option<String>,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::UserMessage => "user_message",
            EventKind::AssistantMessage => "assistant_message",
            EventKind::Command => "command",
            EventKind::Tool => "tool",
        }
    }

    fn parse_kind(value: &str) -> Option<Self> {
        match value {
            "user_message" => Some(EventKind::UserMessage),
            "assistant_message" => Some(EventKind::AssistantMessage),
            "command" => Some(EventKind::Command),
            "tool" => Some(EventKind::Tool),
            _ => None,
        }
    }
}

impl FromStr for EventKind {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::parse_kind(value).ok_or(())
    }
}

pub fn parse_session_file(path: &Path) -> Result<Option<ParsedSession>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut session: Option<SessionMetadata> = None;
    let mut inferred_session: Option<SessionMetadata> = None;
    let mut pending_events = Vec::new();

    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line.with_context(|| format!("read {}:{line_number}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }

        let Ok(record) = serde_json::from_str::<CompactRecord>(&line) else {
            continue;
        };
        let top_type = record.type_name.as_deref().unwrap_or_default();
        let source_timestamp = record.timestamp.clone();
        let payload = record.payload.as_ref().unwrap_or(&Value::Null);

        if top_type == "session_meta" {
            session = parse_session_meta(payload, path);
            continue;
        }
        if top_type == "session" {
            session = parse_compact_session_meta(&record, path);
            continue;
        }
        if let Some(candidate) = infer_compact_session_meta(&record, path) {
            let should_replace = match &inferred_session {
                Some(session) => session.cwd.is_empty() && !candidate.cwd.is_empty(),
                None => true,
            };
            if should_replace {
                inferred_session = Some(candidate);
            }
        }

        let event = match top_type {
            "message" => record.message.as_ref().and_then(|message| {
                parse_compact_v3_message_event(
                    message,
                    record.cwd.as_deref(),
                    path,
                    line_number,
                    source_timestamp.as_deref(),
                )
            }),
            "user" | "assistant" => record.message.as_ref().and_then(|message| {
                parse_compact_claude_message_event(
                    top_type,
                    message,
                    record.cwd.as_deref(),
                    path,
                    line_number,
                    source_timestamp.as_deref(),
                )
            }),
            _ => parse_event(
                top_type,
                &Value::Null,
                payload,
                path,
                line_number,
                source_timestamp.as_deref(),
            ),
        };
        if let Some(event) = event {
            pending_events.push(event);
        }
    }

    let Some(session) = session.or(inferred_session) else {
        return Ok(None);
    };

    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for mut event in pending_events {
        let dedupe_key = format!(
            "{}\u{1f}{}\u{1f}{}",
            event.kind.as_str(),
            event.role.as_deref().unwrap_or_default(),
            event.text
        );
        if seen.insert(dedupe_key) {
            event.session_id = session.id.clone();
            events.push(event);
        }
    }

    Ok(Some(ParsedSession { session, events }))
}

fn parse_session_meta(payload: &Value, path: &Path) -> Option<SessionMetadata> {
    let id = payload.get("id")?.as_str()?.to_owned();
    let timestamp = payload
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let cli_version = payload
        .get("cli_version")
        .and_then(Value::as_str)
        .map(str::to_owned);

    Some(session_metadata(id, timestamp, cwd, cli_version, path))
}

fn parse_compact_session_meta(record: &CompactRecord, path: &Path) -> Option<SessionMetadata> {
    Some(session_metadata(
        record.id.as_ref()?.to_owned(),
        record.timestamp.clone().unwrap_or_default(),
        record.cwd.clone().unwrap_or_default(),
        None,
        path,
    ))
}

fn infer_compact_session_meta(record: &CompactRecord, path: &Path) -> Option<SessionMetadata> {
    let id = record
        .session_id_camel
        .as_ref()
        .or(record.session_id.as_ref())?
        .to_owned();
    let cwd = record
        .cwd
        .as_ref()
        .or(record.project.as_ref())
        .cloned()
        .unwrap_or_default();

    Some(session_metadata(
        id,
        record.timestamp.clone().unwrap_or_default(),
        cwd,
        None,
        path,
    ))
}

fn session_metadata(
    id: String,
    timestamp: String,
    cwd: String,
    cli_version: Option<String>,
    path: &Path,
) -> SessionMetadata {
    let source_kind = source_kind_for_path(path);
    SessionMetadata {
        id,
        timestamp,
        cwd,
        cli_version,
        source_file_path: path.to_path_buf(),
        source_kind,
        source_label: source_kind.label().to_owned(),
    }
}

pub fn source_kind_for_path(path: &Path) -> SourceKind {
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    if has_component_pair(&components, ".claude", "projects")
        || has_component_pair(&components, ".claude", "sessions")
    {
        SourceKind::Claude
    } else if has_component_pair(&components, ".omp", "agent") {
        SourceKind::Omp
    } else if has_component_pair(&components, ".pi", "agent") {
        SourceKind::Pi
    } else if has_component_pair(&components, ".codex", "archived_logs") {
        SourceKind::CodexLog
    } else if components.contains(&".codex") {
        SourceKind::Codex
    } else {
        SourceKind::Unknown
    }
}

fn has_component_pair(components: &[&str], first: &str, second: &str) -> bool {
    components
        .windows(2)
        .any(|window| window[0] == first && window[1] == second)
}

fn parse_event(
    top_type: &str,
    record: &Value,
    payload: &Value,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    let payload_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match (top_type, payload_type) {
        ("event_msg", "user_message") => {
            let text = payload.get("message").and_then(Value::as_str)?.trim();
            non_empty_text_event(
                EventKind::UserMessage,
                Some("user"),
                text,
                path,
                source_line_number,
                source_timestamp,
            )
        }
        ("event_msg", "agent_message") => {
            let text = payload.get("message").and_then(Value::as_str)?.trim();
            non_empty_text_event(
                EventKind::AssistantMessage,
                Some("assistant"),
                text,
                path,
                source_line_number,
                source_timestamp,
            )
        }
        ("event_msg", "exec_command_end") => {
            parse_command_event(payload, path, source_line_number, source_timestamp)
        }
        ("response_item", "message") => {
            let role = payload.get("role").and_then(Value::as_str)?;
            let kind = match role {
                "user" => EventKind::UserMessage,
                "assistant" => EventKind::AssistantMessage,
                _ => return None,
            };
            let text = extract_content_text(payload.get("content")?)?;
            non_empty_text_event(
                kind,
                Some(role),
                text.trim(),
                path,
                source_line_number,
                source_timestamp,
            )
        }
        _ if top_type == "message" => {
            parse_v3_message_event(record, path, source_line_number, source_timestamp)
        }
        _ if top_type == "user" || top_type == "assistant" => {
            parse_claude_message_event(record, path, source_line_number, source_timestamp)
        }
        _ => None,
    }
}

fn parse_compact_v3_message_event(
    message: &CompactMessage,
    cwd: Option<&str>,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    let role = message.role.as_deref()?;
    match role {
        "user" => {
            let text = compact_content_text(message.content.as_ref()?)?;
            non_empty_text_event(
                EventKind::UserMessage,
                Some("user"),
                text.trim(),
                path,
                source_line_number,
                source_timestamp,
            )
        }
        "assistant" => {
            if let Some(text) = message.content.as_ref().and_then(compact_content_text) {
                return non_empty_text_event(
                    EventKind::AssistantMessage,
                    Some("assistant"),
                    text.trim(),
                    path,
                    source_line_number,
                    source_timestamp,
                );
            }
            let tool_name = message.content.as_ref().and_then(compact_tool_call_name)?;
            parse_tool_event(
                &tool_name,
                None,
                cwd,
                path,
                source_line_number,
                source_timestamp,
            )
        }
        "toolResult" => {
            let tool_name = message.tool_name.as_deref().unwrap_or("tool");
            let output = message.content.as_ref().and_then(compact_content_text);
            parse_tool_event(
                tool_name,
                output.as_deref(),
                cwd,
                path,
                source_line_number,
                source_timestamp,
            )
        }
        _ => None,
    }
}

fn parse_compact_claude_message_event(
    top_type: &str,
    message: &CompactMessage,
    cwd: Option<&str>,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    let role = message.role.as_deref().unwrap_or(top_type);
    let content = message.content.as_ref()?;
    if role == "assistant" {
        if let Some(text) = compact_content_text(content) {
            return non_empty_text_event(
                EventKind::AssistantMessage,
                Some("assistant"),
                text.trim(),
                path,
                source_line_number,
                source_timestamp,
            );
        }
        let tool_name = compact_tool_call_name(content)?;
        return parse_tool_event(
            &tool_name,
            None,
            cwd,
            path,
            source_line_number,
            source_timestamp,
        );
    }

    if let Some((tool_name, output)) = compact_claude_tool_result(content) {
        return parse_tool_event(
            &tool_name,
            output.as_deref(),
            cwd,
            path,
            source_line_number,
            source_timestamp,
        );
    }

    let text = compact_content_text(content)?;
    non_empty_text_event(
        EventKind::UserMessage,
        Some("user"),
        text.trim(),
        path,
        source_line_number,
        source_timestamp,
    )
}

fn parse_v3_message_event(
    record: &Value,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    let message = record.get("message")?;
    let role = message.get("role").and_then(Value::as_str)?;
    match role {
        "user" => {
            let text = extract_content_text(message.get("content")?)?;
            non_empty_text_event(
                EventKind::UserMessage,
                Some("user"),
                text.trim(),
                path,
                source_line_number,
                source_timestamp,
            )
        }
        "assistant" => {
            if let Some(text) = extract_content_text(message.get("content")?) {
                return non_empty_text_event(
                    EventKind::AssistantMessage,
                    Some("assistant"),
                    text.trim(),
                    path,
                    source_line_number,
                    source_timestamp,
                );
            }
            let tool_name = extract_tool_call_name(message.get("content")?)?;
            parse_tool_event(
                &tool_name,
                None,
                record.get("cwd").and_then(Value::as_str),
                path,
                source_line_number,
                source_timestamp,
            )
        }
        "toolResult" => {
            let tool_name = message
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let output = message.get("content").and_then(extract_content_text);
            parse_tool_event(
                tool_name,
                output.as_deref(),
                record.get("cwd").and_then(Value::as_str),
                path,
                source_line_number,
                source_timestamp,
            )
        }
        _ => None,
    }
}

fn parse_claude_message_event(
    record: &Value,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    let message = record.get("message")?;
    let role = message.get("role").and_then(Value::as_str)?;
    let content = message.get("content")?;
    if role == "assistant" {
        if let Some(text) = extract_content_text(content) {
            return non_empty_text_event(
                EventKind::AssistantMessage,
                Some("assistant"),
                text.trim(),
                path,
                source_line_number,
                source_timestamp,
            );
        }
        let tool_name = extract_tool_call_name(content)?;
        return parse_tool_event(
            &tool_name,
            None,
            record.get("cwd").and_then(Value::as_str),
            path,
            source_line_number,
            source_timestamp,
        );
    }

    if let Some((tool_name, output)) = extract_claude_tool_result(content) {
        return parse_tool_event(
            &tool_name,
            output.as_deref(),
            record.get("cwd").and_then(Value::as_str),
            path,
            source_line_number,
            source_timestamp,
        );
    }

    let text = extract_content_text(content)?;
    non_empty_text_event(
        EventKind::UserMessage,
        Some("user"),
        text.trim(),
        path,
        source_line_number,
        source_timestamp,
    )
}

fn parse_tool_event(
    tool_name: &str,
    output: Option<&str>,
    cwd: Option<&str>,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    let tool_name = tool_name.trim();
    if tool_name.is_empty() {
        return None;
    }
    let redacted_tool_name = redact_secrets(tool_name);
    let mut text = format!("$ {redacted_tool_name}");
    if let Some(output) = output.filter(|value| !value.trim().is_empty()) {
        text.push('\n');
        let capped_output = cap_text(output, COMMAND_OUTPUT_REDACTION_WINDOW);
        text.push_str(&cap_text(
            &redact_secrets(&capped_output),
            COMMAND_OUTPUT_LIMIT,
        ));
    }

    Some(ParsedEvent {
        session_id: String::new(),
        kind: EventKind::Tool,
        role: None,
        text,
        command: Some(redacted_tool_name),
        cwd: cwd.map(str::to_owned),
        exit_code: None,
        source_timestamp: source_timestamp.map(str::to_owned),
        source_file_path: path.to_path_buf(),
        source_line_number,
    })
}

fn extract_tool_call_name(content: &Value) -> Option<String> {
    let parts = content.as_array()?;
    parts
        .iter()
        .find_map(|part| match part.get("type").and_then(Value::as_str) {
            Some("toolCall") | Some("tool_call") | Some("tool_use") => {
                part.get("name").and_then(Value::as_str).map(str::to_owned)
            }
            _ => None,
        })
}

fn extract_claude_tool_result(content: &Value) -> Option<(String, Option<String>)> {
    let parts = content.as_array()?;
    let part = parts
        .iter()
        .find(|part| part.get("type").and_then(Value::as_str) == Some("tool_result"))?;
    let tool_name = part
        .get("tool_name")
        .or_else(|| part.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_owned();
    let output = part.get("content").and_then(extract_content_text);
    Some((tool_name, output))
}
fn compact_tool_call_name(content: &CompactContent) -> Option<String> {
    let CompactContent::Parts(parts) = content else {
        return None;
    };
    parts.iter().find_map(|part| match part.kind.as_deref() {
        Some("toolCall") | Some("tool_call") | Some("tool_use") => part.name.clone(),
        _ => None,
    })
}

fn compact_claude_tool_result(content: &CompactContent) -> Option<(String, Option<String>)> {
    let CompactContent::Parts(parts) = content else {
        return None;
    };
    let part = parts
        .iter()
        .find(|part| part.kind.as_deref() == Some("tool_result"))?;
    let tool_name = part
        .tool_name
        .as_ref()
        .or(part.name.as_ref())
        .cloned()
        .unwrap_or_else(|| "tool".to_owned());
    let output = part.content.as_deref().and_then(compact_content_text);
    Some((tool_name, output))
}

fn compact_content_text(content: &CompactContent) -> Option<String> {
    match content {
        CompactContent::Text(text) => non_empty_owned(text),
        CompactContent::Parts(parts) => {
            let text = parts
                .iter()
                .filter_map(compact_content_part_text)
                .collect::<Vec<_>>()
                .join("\n");
            non_empty_owned(&text)
        }
    }
}

fn compact_content_part_text(part: &CompactContentPart) -> Option<String> {
    if let Some(text) = &part.text {
        return non_empty_owned(text);
    }
    if let Some(content) = &part.content {
        return compact_content_text(content);
    }
    None
}

fn non_empty_text_event(
    kind: EventKind,
    role: Option<&str>,
    text: &str,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    if text.is_empty() {
        return None;
    }
    if is_codex_preamble(text) {
        return None;
    }

    let capped_text = cap_text(text, MESSAGE_TEXT_REDACTION_WINDOW);
    let redacted_text = cap_text(&redact_secrets(&capped_text), MESSAGE_TEXT_LIMIT);

    Some(ParsedEvent {
        session_id: String::new(),
        kind,
        role: role.map(str::to_owned),
        text: redacted_text,
        command: None,
        cwd: None,
        exit_code: None,
        source_timestamp: source_timestamp.map(str::to_owned),
        source_file_path: path.to_path_buf(),
        source_line_number,
    })
}

fn is_codex_preamble(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("# AGENTS.md instructions") || trimmed.contains("<environment_context>")
}

fn parse_command_event(
    payload: &Value,
    path: &Path,
    source_line_number: usize,
    source_timestamp: Option<&str>,
) -> Option<ParsedEvent> {
    let command = extract_command(payload.get("command")?)?;
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let redacted_command = redact_secrets(command);

    let stdout = payload.get("stdout").and_then(Value::as_str).unwrap_or("");
    let stderr = payload.get("stderr").and_then(Value::as_str).unwrap_or("");
    let mut text = format!("$ {redacted_command}");
    let output = payload
        .get("aggregated_output")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| join_command_output(stdout, stderr));
    if !output.is_empty() {
        text.push('\n');
        let capped_output = cap_text(&output, COMMAND_OUTPUT_REDACTION_WINDOW);
        text.push_str(&cap_text(
            &redact_secrets(&capped_output),
            COMMAND_OUTPUT_LIMIT,
        ));
    }

    Some(ParsedEvent {
        session_id: String::new(),
        kind: EventKind::Command,
        role: None,
        text,
        command: Some(redacted_command),
        cwd: payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_owned),
        exit_code: payload.get("exit_code").and_then(Value::as_i64),
        source_timestamp: source_timestamp.map(str::to_owned),
        source_file_path: path.to_path_buf(),
        source_line_number,
    })
}

fn extract_command(value: &Value) -> Option<String> {
    if let Some(command) = value.as_str() {
        return Some(command.to_owned());
    }

    let argv = value.as_array()?;
    let args = argv.iter().filter_map(Value::as_str).collect::<Vec<_>>();
    if args.len() >= 3 && (args[1] == "-lc" || args[1] == "-c") {
        return Some(args[2].to_owned());
    }

    if args.is_empty() {
        None
    } else {
        Some(args.join(" "))
    }
}

fn extract_content_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return non_empty_owned(text);
    }

    let parts = content.as_array()?;
    let text = parts
        .iter()
        .filter_map(content_part_text)
        .collect::<Vec<_>>()
        .join("\n");

    non_empty_owned(&text)
}

fn content_part_text(part: &Value) -> Option<String> {
    if let Some(text) = part.as_str() {
        return non_empty_owned(text);
    }
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return non_empty_owned(text);
    }
    if let Some(content) = part.get("content") {
        return extract_content_text(content);
    }
    None
}

fn non_empty_owned(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

fn join_command_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_owned(),
        (true, false) => stderr.to_owned(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_jsonl(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-recall-parser-test-{}-{}",
            std::process::id(),
            name
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn skips_malformed_json_lines_inside_transcripts() {
        let path = temp_jsonl(
            "malformed-line",
            r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"session-malformed","timestamp":"2026-04-13T01:00:00Z","cwd":"/tmp"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Before malformed line"}}
{"timestamp":"bad \u"}
{"timestamp":"2026-04-13T01:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"After malformed line"}}
"#,
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.events[0].text, "Before malformed line");
        assert_eq!(parsed.events[1].text, "After malformed line");
    }

    #[test]
    fn parses_session_metadata_and_high_signal_events() {
        let path = temp_jsonl(
            "basic",
            r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"session-1","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/project","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Find the Sentry issue","text_elements":[],"images":[],"local_images":[]}}
{"timestamp":"2026-04-13T01:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I found the Sentry root cause."}]}}
{"timestamp":"2026-04-13T01:00:03Z","type":"event_msg","payload":{"type":"exec_command_end","command":"rg SENTRY","cwd":"/Users/me/project","exit_code":0,"stdout":"SENTRY_DSN=redacted\n","stderr":""}}
"#,
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.session.id, "session-1");
        assert_eq!(parsed.session.cwd, "/Users/me/project");
        assert_eq!(parsed.session.source_file_path, path);
        assert_eq!(parsed.events.len(), 3);
        assert_eq!(parsed.events[0].kind, EventKind::UserMessage);
        assert_eq!(parsed.events[0].text, "Find the Sentry issue");
        assert_eq!(parsed.events[0].source_line_number, 2);
        assert_eq!(parsed.events[1].kind, EventKind::AssistantMessage);
        assert_eq!(parsed.events[1].text, "I found the Sentry root cause.");
        assert_eq!(parsed.events[2].kind, EventKind::Command);
        assert_eq!(parsed.events[2].command.as_deref(), Some("rg SENTRY"));
        assert!(parsed.events[2].text.contains("rg SENTRY"));
        assert!(parsed.events[2].text.contains("SENTRY_DSN"));
    }

    #[test]
    fn skips_events_without_indexable_text() {
        let path = temp_jsonl(
            "noise",
            r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"session-2","timestamp":"2026-04-13T01:00:00Z","cwd":"/tmp"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10}}}}
{"timestamp":"2026-04-13T01:00:02Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"call-1"}}
"#,
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.events.len(), 0);
    }

    #[test]
    fn removes_exact_duplicate_transcript_events() {
        let path = temp_jsonl(
            "duplicates",
            r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"session-3","timestamp":"2026-04-13T01:00:00Z","cwd":"/tmp"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"agent_message","message":"Same assistant answer."}}
{"timestamp":"2026-04-13T01:00:01Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Same assistant answer."}]}}
"#,
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].source_line_number, 2);
    }

    #[test]
    fn skips_codex_instruction_preamble_messages() {
        let path = temp_jsonl(
            "preamble",
            r##"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"session-4","timestamp":"2026-04-13T01:00:00Z","cwd":"/tmp"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"# AGENTS.md instructions for /tmp\n\n<environment_context>\n  <cwd>/tmp</cwd>\n</environment_context>"}}
{"timestamp":"2026-04-13T01:00:02Z","type":"event_msg","payload":{"type":"user_message","message":"What did we decide about Sentry?"}}
"##,
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].text, "What did we decide about Sentry?");
    }

    #[test]
    fn parses_exec_command_end_with_argv_and_aggregated_output() {
        let path = temp_jsonl(
            "argv-command",
            r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"session-5","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/notes-vault"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"exec_command_end","command":["/bin/zsh","-lc","cargo test"],"cwd":"/Users/me/projects/agent-recall","exit_code":0,"aggregated_output":"test result: ok"}}
"#,
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].kind, EventKind::Command);
        assert_eq!(parsed.events[0].command.as_deref(), Some("cargo test"));
        assert_eq!(
            parsed.events[0].cwd.as_deref(),
            Some("/Users/me/projects/agent-recall")
        );
        assert!(parsed.events[0].text.contains("test result: ok"));
    }

    #[test]
    fn redacts_secrets_before_events_are_indexed() {
        let github_pat = ["github", "_pat_", "1234567890abcdefghijklmnop"].concat();
        let path = temp_jsonl(
            "redaction",
            &format!(
                r#"{{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{{"id":"session-6","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/project"}}}}
{{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{{"type":"user_message","message":"Use API_KEY=abc123456789 and Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ"}}}}
{{"timestamp":"2026-04-13T01:00:02Z","type":"event_msg","payload":{{"type":"exec_command_end","command":"curl -H 'Authorization: Bearer supersecrettoken123456' https://example.com","cwd":"/Users/me/project","exit_code":0,"stdout":"{github_pat}\n","stderr":""}}}}
"#
            ),
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.events.len(), 2);
        assert!(parsed.events[0].text.contains("API_KEY=[REDACTED]"));
        assert!(parsed.events[0]
            .text
            .contains("Authorization: Bearer [REDACTED]"));
        assert!(parsed.events[1]
            .command
            .as_deref()
            .unwrap()
            .contains("Authorization: Bearer [REDACTED]"));
        assert!(parsed.events[1].text.contains("[REDACTED]"));
        assert!(!parsed.events.iter().any(|event| {
            event.text.contains("abc123456789")
                || event.text.contains("supersecrettoken123456")
                || event.text.contains(&github_pat)
        }));
    }

    #[test]
    fn caps_large_message_events_before_indexing() {
        let long_text = "alpha ".repeat(10_000);
        let escaped = serde_json::to_string(&long_text).unwrap();
        let path = temp_jsonl(
            "large-message",
            &format!(
                r#"{{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{{"id":"session-7","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/project"}}}}
{{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{{"type":"user_message","message":{escaped}}}}}
"#
            ),
        );

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.events.len(), 1);
        assert!(parsed.events[0].text.len() <= MESSAGE_TEXT_LIMIT + "[truncated]".len() + 1);
        assert!(parsed.events[0].text.contains("[truncated]"));
    }

    #[test]
    fn parses_omp_v3_messages_and_tool_results() {
        let root = std::env::temp_dir()
            .join(format!(
                "agent-recall-parser-test-{}-omp",
                std::process::id()
            ))
            .join(".omp")
            .join("agent")
            .join("sessions")
            .join("-tmp");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        fs::write(
            &path,
            r#"{"type":"session","version":3,"id":"omp-1","timestamp":"2026-06-13T21:22:36.081Z","cwd":"/Users/me/project"}
{"type":"message","timestamp":"2026-06-13T21:22:54.923Z","message":{"role":"user","content":[{"type":"text","text":"Check OMP recall"}]}}
{"type":"message","timestamp":"2026-06-13T21:23:03.779Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"do not index this"},{"type":"text","text":"I will check it."}]}}
{"type":"message","timestamp":"2026-06-13T21:23:08.132Z","message":{"role":"toolResult","toolName":"read","content":[{"type":"text","text":"session file contents"}]}}
"#,
        )
        .unwrap();

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.session.source_kind, SourceKind::Omp);
        assert_eq!(parsed.events.len(), 3);
        assert_eq!(parsed.events[0].kind, EventKind::UserMessage);
        assert_eq!(parsed.events[1].kind, EventKind::AssistantMessage);
        assert_eq!(parsed.events[1].text, "I will check it.");
        assert_eq!(parsed.events[2].kind, EventKind::Tool);
        assert_eq!(parsed.events[2].command.as_deref(), Some("read"));
        assert!(parsed.events[2].text.contains("session file contents"));
    }

    #[test]
    fn parses_claude_project_messages_and_tool_results() {
        let root = std::env::temp_dir()
            .join(format!(
                "agent-recall-parser-test-{}-claude",
                std::process::id()
            ))
            .join(".claude")
            .join("projects")
            .join("-Users-me-project");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("claude-1.jsonl");
        fs::write(
            &path,
            r#"{"type":"user","message":{"role":"user","content":"Audit the report"},"timestamp":"2026-06-01T17:41:46.894Z","cwd":"/Users/me/project","sessionId":"claude-1","version":"2.1.160"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"do not index this"},{"type":"text","text":"I found the issue."}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"tool output"}]},"timestamp":"2026-06-01T17:41:56.209Z","cwd":"/Users/me/project","sessionId":"claude-1"}
"#,
        )
        .unwrap();

        let parsed = parse_session_file(&path).unwrap().unwrap();

        assert_eq!(parsed.session.source_kind, SourceKind::Claude);
        assert_eq!(parsed.session.id, "claude-1");
        assert_eq!(parsed.events.len(), 3);
        assert_eq!(parsed.events[0].kind, EventKind::UserMessage);
        assert_eq!(parsed.events[1].kind, EventKind::AssistantMessage);
        assert_eq!(parsed.events[1].text, "I found the issue.");
        assert_eq!(parsed.events[2].kind, EventKind::Tool);
        assert!(parsed.events[2].text.contains("tool output"));
    }

    #[test]
    fn classifies_pi_paths_separately_from_omp() {
        let pi_path =
            PathBuf::from("/Users/me/.pi/agent/sessions/project/2026-06-01_session.jsonl");
        let omp_path =
            PathBuf::from("/Users/me/.omp/agent/sessions/project/2026-06-01_session.jsonl");

        assert_eq!(source_kind_for_path(&pi_path), SourceKind::Pi);
        assert_eq!(source_kind_for_path(&omp_path), SourceKind::Omp);
    }
}
