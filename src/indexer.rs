use crate::commands::date::{resolve_since, timestamp_key, CalendarDate, ResolvedSince};

use crate::parser::parse_session_file;
use crate::store::{build_session_key, repo_slug, Store};
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INDEX_TRANSACTION_FILE_BATCH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexReport {
    pub files_total: usize,
    pub files_seen: usize,
    pub files_skipped: usize,
    pub skipped_unchanged: usize,
    pub skipped_filtered: usize,
    pub skipped_missing: usize,
    pub skipped_non_session: usize,
    pub sessions_indexed: usize,
    pub events_indexed: usize,
    pub bytes_total: u64,
    pub bytes_seen: u64,
    pub current_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceScanReport {
    pub files_total: usize,
    pub pending_files: usize,
    pub pending_bytes: u64,
    pub stable_pending_files: usize,
    pub waiting_files: usize,
    pub missing_sources: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexFilters {
    repo: Option<String>,
    since: Option<ResolvedSince>,
}

impl IndexFilters {
    pub fn new(repo: Option<String>, since: Option<String>) -> Result<Self> {
        let repo = repo.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        });
        let since = since
            .as_deref()
            .map(resolve_since)
            .transpose()
            .with_context(|| "resolve watch --since bound")?;

        Ok(Self { repo, since })
    }

    pub fn cli_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(repo) = &self.repo {
            args.push("--repo".to_owned());
            args.push(repo.clone());
        }
        if let Some(since) = &self.since {
            args.push("--since".to_owned());
            args.push(since.value.clone());
        }
        args
    }

    pub fn repo(&self) -> Option<&str> {
        self.repo.as_deref()
    }

    pub fn since_value(&self) -> Option<&str> {
        self.since.as_ref().map(|since| since.value.as_str())
    }
}

pub fn index_sources(store: &Store, sources: &[PathBuf]) -> Result<IndexReport> {
    index_sources_with_progress(store, sources, |_| {})
}

pub fn scan_sources_for_pending(
    store: Option<&Store>,
    sources: &[PathBuf],
    quiet_for: Duration,
) -> Result<SourceScanReport> {
    scan_sources_for_pending_with_filters(store, sources, quiet_for, &IndexFilters::default())
}

pub fn scan_sources_for_pending_with_filters(
    store: Option<&Store>,
    sources: &[PathBuf],
    quiet_for: Duration,
    filters: &IndexFilters,
) -> Result<SourceScanReport> {
    let now = SystemTime::now();
    let mut report = SourceScanReport {
        files_total: 0,
        pending_files: 0,
        pending_bytes: 0,
        stable_pending_files: 0,
        waiting_files: 0,
        missing_sources: Vec::new(),
    };

    for source in sources {
        if !source.exists() {
            report.missing_sources.push(source.clone());
            continue;
        }

        for path in jsonl_files_with_filters(source, filters)? {
            report.files_total += 1;
            let file_state = match FileState::from_path(&path) {
                Ok(file_state) => file_state,
                Err(error) if is_not_found_error(&error) => continue,
                Err(error) => return Err(error),
            };
            let is_current = if let Some(store) = store {
                store.is_source_current(
                    &path,
                    file_state.source_file_mtime_ns,
                    file_state.source_file_size,
                )?
            } else {
                false
            };

            if is_current {
                continue;
            }

            if filters_need_parsed_session_for_path(filters, &path) {
                let parsed = match parse_session_file(&path) {
                    Ok(parsed) => parsed,
                    Err(error) if is_not_found_error(&error) => continue,
                    Err(error) => return Err(error),
                };
                if !parsed
                    .as_ref()
                    .is_some_and(|parsed| parsed_session_matches_filters(parsed, filters))
                {
                    continue;
                }
            }

            report.pending_files += 1;
            report.pending_bytes = report
                .pending_bytes
                .saturating_add(file_state.source_file_size as u64);
            if is_stable(now, file_state.modified, quiet_for) {
                report.stable_pending_files += 1;
            } else {
                report.waiting_files += 1;
            }
        }
    }

    Ok(report)
}

pub fn index_sources_with_progress<F>(
    store: &Store,
    sources: &[PathBuf],
    mut on_progress: F,
) -> Result<IndexReport>
where
    F: FnMut(&IndexReport),
{
    index_sources_with_filters_and_progress(
        store,
        sources,
        &IndexFilters::default(),
        &mut on_progress,
    )
}

pub fn index_sources_with_filters_and_progress<F>(
    store: &Store,
    sources: &[PathBuf],
    filters: &IndexFilters,
    mut on_progress: F,
) -> Result<IndexReport>
where
    F: FnMut(&IndexReport),
{
    let mut files = Vec::new();
    for source in sources {
        files.extend(jsonl_files_with_filters(source, filters)?);
    }
    files.sort();

    index_source_files_with_filters_and_progress(store, files, filters, &mut on_progress)
}

pub fn index_stable_pending_sources_with_filters_and_progress<F>(
    store: &Store,
    sources: &[PathBuf],
    quiet_for: Duration,
    filters: &IndexFilters,
    mut on_progress: F,
) -> Result<IndexReport>
where
    F: FnMut(&IndexReport),
{
    let mut files =
        pending_source_files_with_filters(Some(store), sources, quiet_for, filters, true)?;
    files.sort();

    index_source_files_with_filters_and_progress(store, files, filters, &mut on_progress)
}

fn index_source_files_with_filters_and_progress<F>(
    store: &Store,
    files: Vec<PathBuf>,
    filters: &IndexFilters,
    mut on_progress: F,
) -> Result<IndexReport>
where
    F: FnMut(&IndexReport),
{
    let mut report = IndexReport {
        files_total: files.len(),
        files_seen: 0,
        files_skipped: 0,
        skipped_unchanged: 0,
        skipped_filtered: 0,
        skipped_missing: 0,
        skipped_non_session: 0,
        sessions_indexed: 0,
        events_indexed: 0,
        bytes_total: total_known_bytes(&files),
        bytes_seen: 0,
        current_file: None,
    };

    on_progress(&report);

    let mut files_in_batch = 0usize;
    let indexing_result = (|| -> Result<()> {
        for path in files {
            report.current_file = Some(path.clone());

            let file_state = match FileState::from_path(&path) {
                Ok(file_state) => file_state,
                Err(error) if is_not_found_error(&error) => {
                    report.files_seen += 1;
                    report.files_skipped += 1;
                    report.skipped_missing += 1;
                    if should_report_after_file(&report) {
                        on_progress(&report);
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
            report.bytes_seen = report
                .bytes_seen
                .saturating_add(file_state.source_file_size as u64);

            if store.is_source_current(
                &path,
                file_state.source_file_mtime_ns,
                file_state.source_file_size,
            )? {
                report.files_seen += 1;
                report.files_skipped += 1;
                report.skipped_unchanged += 1;
                if should_report_after_file(&report) {
                    on_progress(&report);
                }
                continue;
            }

            on_progress(&report);

            let parsed = match parse_session_file(&path) {
                Ok(parsed) => parsed,
                Err(error) if is_not_found_error(&error) => {
                    report.files_seen += 1;
                    report.files_skipped += 1;
                    report.skipped_missing += 1;
                    if should_report_after_file(&report) {
                        on_progress(&report);
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };

            if let Some(parsed) = parsed {
                if !parsed_session_matches_filters(&parsed, filters) {
                    report.files_seen += 1;
                    report.files_skipped += 1;
                    report.skipped_filtered += 1;
                    if should_report_after_file(&report) {
                        on_progress(&report);
                    }
                    continue;
                }
                let session_key =
                    build_session_key(&parsed.session.id, &parsed.session.source_file_path);
                report.events_indexed += parsed.events.len();
                ensure_open_index_batch(store, &mut files_in_batch)?;
                files_in_batch += 1;
                store.index_session_in_batch(&parsed)?;
                store.mark_source_indexed(
                    &path,
                    file_state.source_file_mtime_ns,
                    file_state.source_file_size,
                    Some(&parsed.session.id),
                    Some(&session_key),
                )?;
                report.sessions_indexed += 1;
            } else {
                ensure_open_index_batch(store, &mut files_in_batch)?;
                files_in_batch += 1;
                store.mark_source_indexed(
                    &path,
                    file_state.source_file_mtime_ns,
                    file_state.source_file_size,
                    None,
                    None,
                )?;
                report.files_skipped += 1;
                report.skipped_non_session += 1;
            }

            if files_in_batch >= INDEX_TRANSACTION_FILE_BATCH {
                commit_open_index_batch(store, &mut files_in_batch)?;
            }

            report.files_seen += 1;
            if should_report_after_file(&report) {
                on_progress(&report);
            }
        }
        Ok(())
    })();

    if let Err(error) = indexing_result {
        rollback_open_index_batch(store, files_in_batch);
        return Err(error);
    }
    commit_open_index_batch(store, &mut files_in_batch)?;

    Ok(report)
}

fn ensure_open_index_batch(store: &Store, files_in_batch: &mut usize) -> Result<()> {
    if *files_in_batch == 0 {
        store.begin_index_batch()?;
    }
    Ok(())
}

fn commit_open_index_batch(store: &Store, files_in_batch: &mut usize) -> Result<()> {
    if *files_in_batch == 0 {
        return Ok(());
    }
    match store.commit_index_batch() {
        Ok(()) => {
            *files_in_batch = 0;
            Ok(())
        }
        Err(error) => {
            rollback_open_index_batch(store, *files_in_batch);
            Err(error)
        }
    }
}

fn rollback_open_index_batch(store: &Store, files_in_batch: usize) {
    if files_in_batch > 0 {
        let _ = store.rollback_index_batch();
    }
}

fn pending_source_files_with_filters(
    store: Option<&Store>,
    sources: &[PathBuf],
    quiet_for: Duration,
    filters: &IndexFilters,
    stable_only: bool,
) -> Result<Vec<PathBuf>> {
    let now = SystemTime::now();
    let mut pending = Vec::new();

    for source in sources {
        if !source.exists() {
            continue;
        }

        for path in jsonl_files_with_filters(source, filters)? {
            let file_state = match FileState::from_path(&path) {
                Ok(file_state) => file_state,
                Err(error) if is_not_found_error(&error) => continue,
                Err(error) => return Err(error),
            };
            let is_current = if let Some(store) = store {
                store.is_source_current(
                    &path,
                    file_state.source_file_mtime_ns,
                    file_state.source_file_size,
                )?
            } else {
                false
            };

            if is_current {
                continue;
            }

            if filters_need_parsed_session_for_path(filters, &path) {
                let parsed = match parse_session_file(&path) {
                    Ok(parsed) => parsed,
                    Err(error) if is_not_found_error(&error) => continue,
                    Err(error) => return Err(error),
                };
                if !parsed
                    .as_ref()
                    .is_some_and(|parsed| parsed_session_matches_filters(parsed, filters))
                {
                    continue;
                }
            }

            if stable_only && !is_stable(now, file_state.modified, quiet_for) {
                continue;
            }

            pending.push(path);
        }
    }

    Ok(pending)
}

fn should_report_after_file(report: &IndexReport) -> bool {
    report.files_seen == 1
        || report.files_seen.is_multiple_of(25)
        || report.files_seen == report.files_total
}

fn total_known_bytes(files: &[PathBuf]) -> u64 {
    files
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum()
}

fn jsonl_files_with_filters(root: &Path, filters: &IndexFilters) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_jsonl_files(root, filters, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_jsonl_files(
    path: &Path,
    filters: &IndexFilters,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    if path.is_file() {
        if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            && path_may_match_since(path, filters)
        {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }

    if directory_is_before_since(path, filters) {
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, filters, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            && path_may_match_since(&path, filters)
        {
            files.push(path);
        }
    }

    Ok(())
}

fn parsed_session_matches_filters(
    parsed: &crate::parser::ParsedSession,
    filters: &IndexFilters,
) -> bool {
    if let Some(since) = &filters.since {
        let Some(session_key) = timestamp_key(&parsed.session.timestamp) else {
            return false;
        };
        if session_key < since.timestamp_key {
            return false;
        }
    }

    if let Some(repo) = &filters.repo {
        if !parsed_session_has_repo(parsed, repo) {
            return false;
        }
    }

    true
}

fn parsed_session_has_repo(parsed: &crate::parser::ParsedSession, repo: &str) -> bool {
    repo_slug(&parsed.session.cwd).eq_ignore_ascii_case(repo)
        || parsed
            .events
            .iter()
            .filter_map(|event| event.cwd.as_deref())
            .map(repo_slug)
            .any(|candidate| candidate.eq_ignore_ascii_case(repo))
}

fn directory_is_before_since(path: &Path, filters: &IndexFilters) -> bool {
    let Some(since) = &filters.since else {
        return false;
    };
    archive_date_prefix(path).is_some_and(|prefix| prefix.is_before(since.date))
}

fn path_may_match_since(path: &Path, filters: &IndexFilters) -> bool {
    let Some(since) = &filters.since else {
        return true;
    };
    !archive_date_prefix(path).is_some_and(|prefix| prefix.is_before(since.date))
}
fn filters_need_parsed_session_for_path(filters: &IndexFilters, path: &Path) -> bool {
    if filters.repo.is_some() {
        return true;
    }
    let Some(since) = &filters.since else {
        return false;
    };
    !archive_date_prefix(path).is_some_and(|prefix| prefix.is_after(since.date))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveDatePrefix {
    Year(i32),
    Month(i32, u32),
    Day(CalendarDate),
}

impl ArchiveDatePrefix {
    fn is_before(self, date: CalendarDate) -> bool {
        match self {
            Self::Year(year) => year < date.year,
            Self::Month(year, month) => (year, month) < (date.year, date.month),
            Self::Day(day) => day < date,
        }
    }

    fn is_after(self, date: CalendarDate) -> bool {
        match self {
            Self::Year(year) => year > date.year,
            Self::Month(year, month) => (year, month) > (date.year, date.month),
            Self::Day(day) => day > date,
        }
    }
}

fn archive_date_prefix(path: &Path) -> Option<ArchiveDatePrefix> {
    let parts = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    let mut prefix = None;

    for index in 0..parts.len() {
        if let Some(day) = parse_iso_date_prefix(parts[index]) {
            prefix = Some(ArchiveDatePrefix::Day(day));
            continue;
        }
        let Some(year) = parse_year(parts[index]) else {
            continue;
        };
        if index + 1 == parts.len() {
            prefix = Some(ArchiveDatePrefix::Year(year));
            continue;
        }
        let Some(month) = parts.get(index + 1).copied().and_then(parse_month) else {
            continue;
        };
        if index + 2 == parts.len() {
            prefix = Some(ArchiveDatePrefix::Month(year, month));
            continue;
        }
        let Some(day) = parts.get(index + 2).copied().and_then(parse_day_component) else {
            continue;
        };
        prefix = Some(ArchiveDatePrefix::Day(CalendarDate { year, month, day }));
    }

    prefix
}

fn parse_iso_date_prefix(value: &str) -> Option<CalendarDate> {
    if value.len() < 10 {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let year = value.get(0..4).and_then(parse_year)?;
    let month = value.get(5..7).and_then(parse_month)?;
    let day = value.get(8..10).and_then(parse_day_component)?;
    Some(CalendarDate { year, month, day })
}

fn parse_year(value: &str) -> Option<i32> {
    if value.len() == 4 && value.as_bytes().iter().all(|byte| byte.is_ascii_digit()) {
        value.parse().ok()
    } else {
        None
    }
}

fn parse_month(value: &str) -> Option<u32> {
    parse_two_digit_component(OsStr::new(value)).filter(|month| (1..=12).contains(month))
}

fn parse_day_component(value: &str) -> Option<u32> {
    parse_two_digit_component(OsStr::new(value)).filter(|day| (1..=31).contains(day))
}

fn parse_two_digit_component(value: &OsStr) -> Option<u32> {
    let value = value.to_str()?;
    if value.len() == 2 && value.as_bytes().iter().all(|byte| byte.is_ascii_digit()) {
        value.parse().ok()
    } else {
        None
    }
}

struct FileState {
    source_file_mtime_ns: i64,
    source_file_size: i64,
    modified: SystemTime,
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
        let source_file_size = metadata.len() as i64;

        Ok(Self {
            source_file_mtime_ns,
            source_file_size,
            modified,
        })
    }
}

fn is_stable(now: SystemTime, modified: SystemTime, quiet_for: Duration) -> bool {
    quiet_for.is_zero()
        || now
            .duration_since(modified)
            .is_ok_and(|age| age >= quiet_for)
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == ErrorKind::NotFound)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-recall-indexer-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_session(path: &Path, message: &str) {
        std::fs::write(
            path,
            format!(
                r#"{{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{{"id":"session-1","timestamp":"2026-04-13T01:00:00Z","cwd":"/tmp"}}}}
{{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{{"type":"user_message","message":"{message}"}}}}
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn archive_date_prefix_ignores_unrelated_year_named_parent_dirs() {
        assert_eq!(archive_date_prefix(Path::new("/Users/2025/sessions")), None);
        assert_eq!(
            archive_date_prefix(Path::new("/Users/2025/sessions/2026/04/13/file.jsonl")),
            Some(ArchiveDatePrefix::Day(CalendarDate {
                year: 2026,
                month: 4,
                day: 13
            }))
        );
        assert_eq!(
            archive_date_prefix(Path::new(
                "/Users/me/.omp/agent/sessions/-projects/2026-06-13T21-22-36-081Z_session.jsonl"
            )),
            Some(ArchiveDatePrefix::Day(CalendarDate {
                year: 2026,
                month: 6,
                day: 13
            }))
        );
        assert_eq!(
            archive_date_prefix(Path::new("/sessions/2025")),
            Some(ArchiveDatePrefix::Year(2025))
        );
        assert_eq!(
            archive_date_prefix(Path::new("/sessions/2025/12")),
            Some(ArchiveDatePrefix::Month(2025, 12))
        );
    }

    #[test]
    fn skips_unchanged_files_on_second_index_run() {
        let root = temp_root("incremental");
        let source = root.join("sessions");
        std::fs::create_dir_all(&source).unwrap();
        let session_file = source.join("session.jsonl");
        write_session(&session_file, "First message");
        let store = Store::open(root.join("index.sqlite")).unwrap();

        let first = index_sources(&store, std::slice::from_ref(&source)).unwrap();
        let second = index_sources(&store, &[source]).unwrap();

        assert_eq!(first.files_seen, 1);
        assert_eq!(first.files_total, 1);
        assert!(first.bytes_total > 0);
        assert_eq!(first.bytes_seen, first.bytes_total);
        assert_eq!(first.current_file, Some(session_file));
        assert_eq!(first.files_skipped, 0);
        assert_eq!(first.sessions_indexed, 1);
        assert_eq!(second.files_seen, 1);
        assert_eq!(second.files_total, 1);
        assert_eq!(second.files_skipped, 1);
        assert_eq!(second.skipped_unchanged, 1);
        assert_eq!(second.skipped_missing, 0);
        assert_eq!(second.skipped_non_session, 0);
        assert_eq!(second.sessions_indexed, 0);
    }

    #[test]
    fn skips_files_removed_after_scan() {
        let root = temp_root("removed-after-scan");
        let source = root.join("sessions");
        std::fs::create_dir_all(&source).unwrap();
        for index in 0..99 {
            write_session(
                &source.join(format!("a-{index:03}.jsonl")),
                &format!("Message {index}"),
            );
        }
        let disappearing = source.join("z-disappearing.jsonl");
        write_session(&disappearing, "This file disappears during indexing");
        let store = Store::open(root.join("index.sqlite")).unwrap();

        let mut removed = false;
        let report = index_sources_with_progress(&store, &[source], |report| {
            if report.files_seen == 0 && !removed {
                std::fs::remove_file(&disappearing).unwrap();
                removed = true;
            }
        })
        .unwrap();

        assert_eq!(report.files_seen, 100);
        assert_eq!(report.files_total, 100);
        assert_eq!(report.files_skipped, 1);
        assert_eq!(report.skipped_missing, 1);
        assert_eq!(report.sessions_indexed, 99);
    }

    #[test]
    fn reports_current_file_before_processing() {
        let root = temp_root("current-before-processing");
        let source = root.join("sessions");
        std::fs::create_dir_all(&source).unwrap();
        let session_file = source.join("session.jsonl");
        write_session(&session_file, "Current file");
        let store = Store::open(root.join("index.sqlite")).unwrap();

        let mut saw_current_before_index = false;
        index_sources_with_progress(&store, &[source], |report| {
            if report.current_file.as_ref() == Some(&session_file) && report.sessions_indexed == 0 {
                saw_current_before_index = true;
            }
        })
        .unwrap();

        assert!(saw_current_before_index);
    }
}
