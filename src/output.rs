use crate::indexer::IndexReport;
use crate::log_importer::LogImportProgress;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn progress_line(report: &IndexReport, elapsed: Duration) -> String {
    let percent = if report.files_total == 0 {
        100.0
    } else {
        (report.files_seen as f64 / report.files_total as f64) * 100.0
    };
    let eta = estimate_eta(elapsed, report.files_seen, report.files_total)
        .map(format_duration)
        .unwrap_or_else(|| "unknown".to_owned());
    let current = report
        .current_file
        .as_ref()
        .map(|path| shorten_path(path, 96))
        .unwrap_or_else(|| "-".to_owned());

    format!(
        "progress: {}/{} files ({percent:.1}%), bytes {}/{}, indexed {}, skipped {} (unchanged {}, filtered {}, missing {}, non-session {}), elapsed {}, eta {}, current {}",
        report.files_seen,
        report.files_total,
        format_bytes(report.bytes_seen),
        format_bytes(report.bytes_total),
        report.sessions_indexed,
        report.files_skipped,
        report.skipped_unchanged,
        report.skipped_filtered,
        report.skipped_missing,
        report.skipped_non_session,
        format_duration(elapsed),
        eta,
        current
    )
}
pub fn log_import_progress_line(progress: &LogImportProgress, elapsed: Duration) -> String {
    let percent = match progress.rows_total {
        Some(0) => 100.0,
        Some(total) => (progress.rows_seen as f64 / total as f64) * 100.0,
        None => 0.0,
    };
    let eta = progress
        .rows_total
        .and_then(|total| estimate_eta(elapsed, progress.rows_seen, total))
        .map(format_duration)
        .unwrap_or_else(|| "unknown".to_owned());
    let current = progress
        .current_source
        .as_ref()
        .map(|path| shorten_path(path, 96))
        .unwrap_or_else(|| "-".to_owned());
    let rows = progress
        .rows_total
        .map(|total| format!("{}/{} ({percent:.1}%)", progress.rows_seen, total))
        .unwrap_or_else(|| "-".to_owned());

    format!(
        "progress: {}/{} log dbs, phase {}, rows {}, threads {}, duplicate threads {}, parsed {} sessions, indexed {} sessions/{} events, elapsed {}, eta {}, current {}",
        progress.sources_seen,
        progress.sources_total,
        progress.phase.as_str(),
        rows,
        progress.threads_seen,
        progress.skipped_duplicate_threads,
        progress.sessions_parsed,
        progress.sessions_indexed,
        progress.events_indexed,
        format_duration(elapsed),
        eta,
        current
    )
}

fn estimate_eta(elapsed: Duration, seen: usize, total: usize) -> Option<Duration> {
    if seen == 0 || total == 0 || seen >= total {
        return None;
    }
    let elapsed_secs = elapsed.as_secs_f64();
    if elapsed_secs <= 0.0 {
        return None;
    }
    let per_file = elapsed_secs / seen as f64;
    Some(Duration::from_secs_f64(per_file * (total - seen) as f64))
}

fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn shorten_path(path: &Path, max_chars: usize) -> String {
    let value = path.display().to_string();
    if value.chars().count() <= max_chars {
        return value;
    }

    let tail_len = max_chars.saturating_sub(3);
    let tail = value
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

pub fn now_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_unix_timestamp(duration.as_secs() as i64, duration.subsec_millis())
}

fn format_unix_timestamp(seconds: i64, millis: u32) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u32, day as u32)
}

pub fn compact_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn preview(value: &str, limit: usize) -> String {
    let compact = compact_whitespace(value);
    if compact.len() <= limit {
        return compact;
    }

    let mut output = compact
        .char_indices()
        .take_while(|(index, _)| *index < limit)
        .map(|(_, ch)| ch)
        .collect::<String>();
    output.push_str(" ...");
    output
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
