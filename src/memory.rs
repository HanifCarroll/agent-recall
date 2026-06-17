use crate::output::compact_whitespace;
use crate::parser::{EventKind, ParsedSession};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryKind {
    Decision,
    Task,
    Fact,
    OpenQuestion,
    Blocker,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryKind::Decision => "decision",
            MemoryKind::Task => "task",
            MemoryKind::Fact => "fact",
            MemoryKind::OpenQuestion => "open_question",
            MemoryKind::Blocker => "blocker",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "decision" => Some(Self::Decision),
            "task" => Some(Self::Task),
            "fact" => Some(Self::Fact),
            "open_question" => Some(Self::OpenQuestion),
            "blocker" => Some(Self::Blocker),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedMemory {
    pub id: String,
    pub kind: MemoryKind,
    pub summary: String,
    pub normalized_text: String,
    pub event_kind: EventKind,
    pub source_line_number: usize,
    pub source_timestamp: Option<String>,
    pub evidence_text: String,
}

pub fn extract_memories(parsed: &ParsedSession) -> Vec<ExtractedMemory> {
    let mut extracted = Vec::new();
    let mut seen = HashSet::new();

    for event in &parsed.events {
        if matches!(event.kind, EventKind::Command | EventKind::Tool) {
            continue;
        }

        for line in event.text.lines() {
            let Some((kind, summary, normalized_text)) = classify_line(line) else {
                continue;
            };
            let dedupe_key = format!(
                "{}\u{1f}{}\u{1f}{}",
                kind.as_str(),
                normalized_text,
                event.source_line_number
            );
            if !seen.insert(dedupe_key) {
                continue;
            }

            let evidence_text = compact_whitespace(line);
            extracted.push(ExtractedMemory {
                id: build_memory_id(kind, &normalized_text),
                kind,
                summary,
                normalized_text,
                event_kind: event.kind,
                source_line_number: event.source_line_number,
                source_timestamp: event.source_timestamp.clone(),
                evidence_text,
            });
        }
    }

    extracted
}

pub fn build_memory_id(kind: MemoryKind, normalized_text: &str) -> String {
    format!(
        "mem_{}_{:016x}",
        kind.as_str(),
        fnv1a64(normalized_text.as_bytes())
    )
}

fn classify_line(line: &str) -> Option<(MemoryKind, String, String)> {
    let line = compact_whitespace(line);
    let line = line.trim();
    if line.is_empty() || line.starts_with('$') || line == "[truncated]" {
        return None;
    }

    if let Some(summary) = strip_prefix_ci(line, "decision:") {
        return memory_from_summary(MemoryKind::Decision, summary);
    }
    if let Some(summary) = strip_prefix_ci(line, "task:") {
        return memory_from_summary(MemoryKind::Task, summary);
    }
    if let Some(summary) = strip_prefix_ci(line, "fact:") {
        return memory_from_summary(MemoryKind::Fact, summary);
    }
    if let Some(summary) = strip_prefix_ci(line, "question:") {
        return memory_from_summary(MemoryKind::OpenQuestion, summary);
    }
    if let Some(summary) = strip_prefix_ci(line, "blocked:") {
        return memory_from_summary(MemoryKind::Blocker, summary);
    }
    if let Some(summary) = strip_prefix_ci(line, "blocker:") {
        return memory_from_summary(MemoryKind::Blocker, summary);
    }
    if let Some(summary) = strip_prefix_ci(line, "next step:") {
        return memory_from_summary(MemoryKind::Task, summary);
    }
    if let Some(summary) = strip_prefix_ci(line, "confirmed:") {
        return memory_from_summary(MemoryKind::Fact, summary);
    }

    let lower = line.to_ascii_lowercase();
    if lower.ends_with('?') {
        return memory_from_summary(MemoryKind::OpenQuestion, line);
    }
    if lower.contains("blocked by")
        || lower.contains("waiting on")
        || lower.contains("login required")
        || lower.contains("no token found")
        || lower.contains("cannot ")
        || lower.contains("can't ")
    {
        return memory_from_summary(MemoryKind::Blocker, line);
    }
    if lower.contains("next step")
        || lower.contains("need to ")
        || lower.contains("needs to ")
        || lower.contains("follow up")
        || lower.contains("todo")
    {
        return memory_from_summary(MemoryKind::Task, line);
    }
    if lower.contains("we decided")
        || lower.contains("decided to")
        || lower.contains(" will stay ")
        || lower.contains(" should remain ")
        || lower.starts_with("keep ")
        || lower.contains(" stays ")
        || lower.contains(" prefer ")
        || lower.contains(" the fix is ")
    {
        return memory_from_summary(MemoryKind::Decision, line);
    }
    if lower.contains("confirmed")
        || lower.contains(" is currently ")
        || lower.contains(" still ")
        || lower.contains(" passed")
        || lower.contains(" passes")
        || lower.contains(" failed")
        || lower.contains(" returns ")
        || lower.contains(" shows ")
    {
        return memory_from_summary(MemoryKind::Fact, line);
    }

    None
}

fn memory_from_summary(kind: MemoryKind, summary: &str) -> Option<(MemoryKind, String, String)> {
    let summary = summary.trim().trim_matches('-').trim();
    if summary.is_empty() {
        return None;
    }

    let normalized_text = normalize_memory_text(summary);
    if normalized_text.is_empty() {
        return None;
    }

    Some((kind, summary.to_owned(), normalized_text))
}

fn normalize_memory_text(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_space = false;
    for ch in value.chars().flat_map(|ch| ch.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch);
            last_was_space = false;
        } else if !last_was_space {
            normalized.push(' ');
            last_was_space = true;
        }
    }
    normalized.trim().to_owned()
}

fn strip_prefix_ci<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() < prefix.len() {
        return None;
    }
    let head = value.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        return Some(value.get(prefix.len()..).unwrap_or_default().trim());
    }
    None
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{ParsedEvent, SessionMetadata, SourceKind};
    use std::path::PathBuf;

    #[test]
    fn extracts_prefixed_memory_objects() {
        let source = PathBuf::from("/tmp/session.jsonl");
        let parsed = ParsedSession {
            session: SessionMetadata {
                id: "session-1".to_owned(),
                timestamp: "2026-04-13T01:00:00Z".to_owned(),
                cwd: "/Users/me/project".to_owned(),
                cli_version: None,
                source_file_path: source.clone(),
                source_kind: SourceKind::Codex,
                source_label: "Codex".to_owned(),
            },
            events: vec![ParsedEvent {
                session_id: "session-1".to_owned(),
                kind: EventKind::AssistantMessage,
                role: Some("assistant".to_owned()),
                text: "Decision: Keep MCP resources JSON-only.\nNext step: wire delta cursors."
                    .to_owned(),
                command: None,
                cwd: None,
                exit_code: None,
                source_timestamp: Some("2026-04-13T01:00:01Z".to_owned()),
                source_file_path: source,
                source_line_number: 2,
            }],
        };

        let extracted = extract_memories(&parsed);
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].kind, MemoryKind::Decision);
        assert_eq!(extracted[0].summary, "Keep MCP resources JSON-only.");
        assert!(extracted[0].id.starts_with("mem_decision_"));
        assert_eq!(extracted[1].kind, MemoryKind::Task);
    }

    #[test]
    fn ignores_non_matching_multibyte_prefix_safely() {
        assert_eq!(strip_prefix_ci("Here’s a note", "note:"), None);
    }
}
