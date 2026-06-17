use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use rusqlite::Connection;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-recall-cli-test-{}-{}",
        std::process::id(),
        name
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_sample_session(root: &std::path::Path) {
    let session_dir = root.join("2026/04/13");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("rollout-2026-04-13T01-00-00-session-1.jsonl"),
        r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"session-1","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/project","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Debug RevenueCat Stripe webhook"}}
{"timestamp":"2026-04-13T01:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"The production webhook secret was missing."}}
"#,
    )
    .unwrap();
}

fn write_session(root: &std::path::Path, id: &str, cwd: &str, timestamp: &str) {
    write_session_file(
        root,
        &format!("{id}.jsonl"),
        id,
        cwd,
        timestamp,
        "The production webhook secret was missing.",
    );
}

fn write_session_file(
    root: &std::path::Path,
    file_name: &str,
    id: &str,
    cwd: &str,
    timestamp: &str,
    message: &str,
) {
    let session_dir = root.join("2026/04/13");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join(file_name),
        format!(
            r#"{{"timestamp":"{timestamp}","type":"session_meta","payload":{{"id":"{id}","timestamp":"{timestamp}","cwd":"{cwd}","cli_version":"0.1.0"}}}}
{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"agent_message","message":"{message}"}}}}
"#
        ),
    )
    .unwrap();
}

fn write_mixed_session_file(
    root: &std::path::Path,
    file_name: &str,
    id: &str,
    cwd: &str,
    timestamp: &str,
) {
    let session_dir = root.join("2026/04/13");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join(file_name),
        format!(
            r#"{{"timestamp":"{timestamp}","type":"session_meta","payload":{{"id":"{id}","timestamp":"{timestamp}","cwd":"{cwd}","cli_version":"0.1.0"}}}}
{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"user_message","message":"Find the webhook secret bug"}}}}
{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"agent_message","message":"The webhook secret was missing in production."}}}}
{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"exec_command_end","command":["/bin/zsh","-lc","rg webhook"],"cwd":"{cwd}","exit_code":0,"aggregated_output":"webhook command receipt"}}}}
"#
        ),
    )
    .unwrap();
}

fn write_memory_session_file(
    root: &std::path::Path,
    file_name: &str,
    id: &str,
    cwd: &str,
    timestamp: &str,
    events: &[(&str, &str)],
) {
    let session_dir = root.join("2026/04/13");
    fs::create_dir_all(&session_dir).unwrap();
    let mut contents = format!(
        r#"{{"timestamp":"{timestamp}","type":"session_meta","payload":{{"id":"{id}","timestamp":"{timestamp}","cwd":"{cwd}","cli_version":"0.1.0"}}}}
"#
    );
    for (offset, (event_type, message)) in events.iter().enumerate() {
        contents.push_str(&format!(
            r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"{event_type}","message":"{message}"}}}}
"#
        ));
        if offset + 1 == events.len() {
            contents.push('\n');
        }
    }
    fs::write(session_dir.join(file_name), contents).unwrap();
}

fn index_sources(db: &std::path::Path, sources: &[&std::path::Path]) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-recall"));
    command.args(["index", "--db"]).arg(db);
    for source in sources {
        command.args(["--source"]).arg(source);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_many_sessions(root: &std::path::Path, count: usize) {
    let session_dir = root.join("2026/04/13");
    fs::create_dir_all(&session_dir).unwrap();
    for index in 0..count {
        let id = format!("session-{index}");
        fs::write(
            session_dir.join(format!("{id}.jsonl")),
            format!(
                r#"{{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{{"id":"{id}","timestamp":"2026-04-13T01:00:00Z","cwd":"/tmp/project"}}}}
{{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{{"type":"user_message","message":"Need progress output {index}"}}}}
"#
            ),
        )
        .unwrap();
    }
}

#[test]
fn search_does_not_create_missing_database() {
    let temp = temp_dir("search-missing-db");
    let db = temp.join("missing").join("index.sqlite");

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "webhook", "--db"])
        .arg(&db)
        .output()
        .unwrap();

    assert!(!search.status.success());
    assert!(!db.exists(), "search should not create the database");
    assert!(
        !db.parent().unwrap().exists(),
        "search should not create the database directory"
    );
}

#[test]
fn index_then_search_outputs_ranked_receipts() {
    let temp = temp_dir("index-search");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_sample_session(&source);

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );
    assert!(String::from_utf8_lossy(&index.stdout).contains("indexed 1 session files"));

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "webhook secret", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let stdout = String::from_utf8_lossy(&search.stdout);
    assert!(stdout.contains("session-1"));
    assert!(stdout.contains("/Users/me/project"));
    assert!(stdout.contains(":3"));
    assert!(stdout.contains("production webhook secret"));
}

#[test]
fn cli_supports_top_level_version_flag() {
    let version = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        version.status.success(),
        "version failed: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    assert!(
        String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")),
        "{}",
        String::from_utf8_lossy(&version.stdout)
    );
}

#[test]
fn subcommand_help_lists_typed_flags() {
    let help = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["watch", "--help"])
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "help failed: {}",
        String::from_utf8_lossy(&help.stderr)
    );
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("--quiet-for"), "{stdout}");
    assert!(stdout.contains("--install-launch-agent"), "{stdout}");
}

#[test]
fn help_uses_generic_launch_agent_defaults() {
    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["watch", "--help"])
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch help failed: {}",
        String::from_utf8_lossy(&watch.stderr)
    );
    let watch_stdout = String::from_utf8_lossy(&watch.stdout);
    assert!(
        watch_stdout.contains("[default: dev.agent-recall.watch]"),
        "{watch_stdout}"
    );
    assert!(
        !watch_stdout.contains("com.hanif.agent-recall.watch"),
        "{watch_stdout}"
    );

    let doctor = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["doctor", "--help"])
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "doctor help failed: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let doctor_stdout = String::from_utf8_lossy(&doctor.stdout);
    assert!(
        doctor_stdout.contains("[default: dev.agent-recall.watch]"),
        "{doctor_stdout}"
    );
    assert!(
        !doctor_stdout.contains("com.hanif.agent-recall.watch"),
        "{doctor_stdout}"
    );
}

#[test]
fn watch_once_indexes_archived_session_with_curly_apostrophe() {
    let temp = temp_dir("watch-curly-apostrophe");
    let source = temp.join("archived_sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch.json");
    write_session_file(
        &source,
        "curly-apostrophe.jsonl",
        "session-curly",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        "Here’s a note from an archived session.",
    );

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["watch", "--once", "--quiet-for", "0", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&watch.stderr)
    );
    assert!(
        String::from_utf8_lossy(&watch.stdout).contains("watch indexed 1 session files"),
        "{}",
        String::from_utf8_lossy(&watch.stdout)
    );

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "archived session", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let stdout = String::from_utf8_lossy(&search.stdout);
    assert!(stdout.contains("session-curly"), "{stdout}");
    assert!(stdout.contains("Here’s a note"), "{stdout}");
}

#[test]
fn watch_once_supports_repo_and_since_bounds_without_scanning_older_archives() {
    let temp = temp_dir("watch-bounded-repo-since");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch.json");
    let old_dir = source.join("2026/04/12");
    fs::create_dir_all(&old_dir).unwrap();
    fs::write(old_dir.join("broken-old.jsonl"), "{not-json\n").unwrap();
    write_session_file(
        &source,
        "matching.jsonl",
        "matching-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        "Bounded watcher should index this agent recall session.",
    );
    write_session_file(
        &source,
        "other-repo.jsonl",
        "other-session",
        "/Users/me/projects/other-tool",
        "2026-04-13T01:00:00Z",
        "Bounded watcher should skip this other repo session.",
    );

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "watch",
            "--once",
            "--quiet-for",
            "0",
            "--repo",
            "agent-recall",
            "--since",
            "2026-04-13",
            "--db",
        ])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&watch.stderr)
    );
    assert!(
        String::from_utf8_lossy(&watch.stdout).contains("watch indexed 1 session files"),
        "{}",
        String::from_utf8_lossy(&watch.stdout)
    );

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "Bounded watcher", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{json:#}");
    assert_eq!(results[0]["session_id"], "matching-session");
}

#[test]
fn index_supports_since_bounds_for_iso_named_session_files() {
    let temp = temp_dir("index-bounded-since");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("2026-04-12T00-00-00-000Z_old.jsonl"),
        "{not-json\n",
    )
    .unwrap();
    write_session_file(
        &source,
        "2026-04-13T01-00-00-000Z_matching.jsonl",
        "matching-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        "Bounded index should include this session.",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--since", "2026-04-13", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        index.status.success(),
        "index failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&index.stdout),
        String::from_utf8_lossy(&index.stderr)
    );

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "Bounded index", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    let results = json["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{json:#}");
    assert_eq!(results[0]["session_id"], "matching-session");
}

#[test]
fn search_json_outputs_machine_readable_receipts() {
    let temp = temp_dir("search-json");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_sample_session(&source);

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "webhook secret", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(json["query"], "webhook secret");
    assert!(json["results"][0]["session_key"]
        .as_str()
        .unwrap()
        .starts_with("session-1:"));
    assert_eq!(json["results"][0]["session_id"], "session-1");
    assert_eq!(json["results"][0]["source_line_number"], 3);
    assert_eq!(json["results"][0]["kind"], "assistant_message");
    assert!(json["results"][0]["source"]
        .as_str()
        .unwrap()
        .contains(":3"));
    assert!(json["results"][0].get("text").is_none());
    assert!(json["results"][0]["text_preview"]
        .as_str()
        .unwrap()
        .contains("production webhook secret"));
}

#[test]
fn show_disambiguates_duplicate_session_ids_and_accepts_session_key() {
    let temp = temp_dir("show-duplicate");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_session_file(
        &source,
        "active.jsonl",
        "session-dup",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
        "The active webhook secret was missing.",
    );
    write_session_file(
        &source,
        "archived.jsonl",
        "session-dup",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
        "The archived webhook secret was missing.",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let ambiguous_show = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["show", "session-dup", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!ambiguous_show.status.success());
    assert!(
        String::from_utf8_lossy(&ambiguous_show.stderr).contains("multiple indexed sessions match")
    );

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "archived webhook", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(search.status.success());
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    let key = json["results"][0]["session_key"].as_str().unwrap();

    let show = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["show", key, "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "show failed: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains(key));
    assert!(stdout.contains("archived webhook secret"));
}

#[test]
fn show_prints_session_events_with_line_receipts() {
    let temp = temp_dir("show");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_sample_session(&source);

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let show = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["show", "session-1", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "show failed: {}",
        String::from_utf8_lossy(&show.stderr)
    );

    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains("session-1"));
    assert!(stdout.contains("user_message"));
    assert!(stdout.contains(":2"));
    assert!(stdout.contains("Debug RevenueCat Stripe webhook"));
    assert!(stdout.contains("assistant_message"));
    assert!(stdout.contains(":3"));
}

#[test]
fn search_filters_by_repo_cwd_and_since_flags() {
    let temp = temp_dir("search-filters");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_session(
        &source,
        "old-acme-api",
        "/Users/me/projects/acme-api",
        "2026-04-01T01:00:00Z",
    );
    write_session(
        &source,
        "new-acme-api",
        "/Users/me/projects/acme-api",
        "2026-04-13T01:00:00Z",
    );
    write_session(
        &source,
        "ops-tool",
        "/Users/me/projects/ops-tool",
        "2026-04-13T01:00:00Z",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "search",
            "webhook secret",
            "--repo",
            "acme-api",
            "--cwd",
            "projects/acme-api",
            "--since",
            "2026-04-10",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let stdout = String::from_utf8_lossy(&search.stdout);
    assert!(stdout.contains("new-acme-api"));
    assert!(!stdout.contains("old-acme-api"));
    assert!(!stdout.contains("ops-tool"));
}

#[test]
fn search_filters_by_from_until_and_excluded_session() {
    let temp = temp_dir("search-range-exclude");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_session(
        &source,
        "too-old",
        "/Users/me/projects/project",
        "2026-04-01T01:00:00Z",
    );
    write_session(
        &source,
        "in-range",
        "/Users/me/projects/project",
        "2026-04-13T01:00:00Z",
    );
    write_session(
        &source,
        "excluded",
        "/Users/me/projects/project",
        "2026-04-13T02:00:00Z",
    );
    write_session(
        &source,
        "too-new",
        "/Users/me/projects/project",
        "2026-04-15T01:00:00Z",
    );
    index_sources(&db, &[&source]);

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "search",
            "webhook secret",
            "--from",
            "2026-04-10",
            "--until",
            "2026-04-14",
            "--exclude-session",
            "excluded",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let stdout = String::from_utf8_lossy(&search.stdout);
    assert!(stdout.contains("in-range"), "{stdout}");
    assert!(!stdout.contains("too-old"), "{stdout}");
    assert!(!stdout.contains("excluded"), "{stdout}");
    assert!(!stdout.contains("too-new"), "{stdout}");
}

#[test]
fn search_rejects_since_and_from_together() {
    let temp = temp_dir("search-since-from");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_sample_session(&source);
    index_sources(&db, &[&source]);

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "search",
            "webhook secret",
            "--since",
            "2026-04-01",
            "--from",
            "2026-04-01",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();

    assert!(!search.status.success());
    assert!(String::from_utf8_lossy(&search.stderr).contains("use either --since or --from"));
}

#[test]
fn search_dedupes_active_and_archived_session_copies_by_default() {
    let temp = temp_dir("search-dedupe");
    let active = temp.join("sessions");
    let archived = temp.join("archived_sessions");
    let db = temp.join("index.sqlite");
    write_session_file(
        &active,
        "active.jsonl",
        "session-dup",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
        "The active webhook secret was missing.",
    );
    write_session_file(
        &archived,
        "archived.jsonl",
        "session-dup",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
        "The archived webhook secret was missing.",
    );
    index_sources(&db, &[&active, &archived]);

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "webhook secret", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(json["count"], 1);
    assert!(json["results"][0]["source_file_path"]
        .as_str()
        .unwrap()
        .contains("/sessions/"));

    let duplicate_search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "search",
            "webhook secret",
            "--include-duplicates",
            "--json",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(duplicate_search.status.success());
    let json: serde_json::Value = serde_json::from_slice(&duplicate_search.stdout).unwrap();
    assert_eq!(json["count"], 2);
}

#[test]
fn search_supports_day_kind_json_and_exclude_current() {
    let temp = temp_dir("search-day-kind-current");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_mixed_session_file(
        &source,
        "current.jsonl",
        "current-session",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
    );
    write_mixed_session_file(
        &source,
        "kept.jsonl",
        "kept-session",
        "/Users/me/project",
        "2026-04-13T02:00:00Z",
    );
    write_mixed_session_file(
        &source,
        "other-day.jsonl",
        "other-day",
        "/Users/me/project",
        "2026-04-14T01:00:00Z",
    );
    index_sources(&db, &[&source]);

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("CODEX_THREAD_ID", "current-session")
        .args([
            "search",
            "webhook",
            "--day",
            "2026-04-13",
            "--kind",
            "assistant",
            "--exclude-current",
            "--json",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(json["count"], 1);
    assert_eq!(json["results"][0]["session_id"], "kept-session");
    assert_eq!(json["results"][0]["kind"], "assistant_message");
}

#[test]
fn search_rejects_day_with_other_date_filters() {
    let temp = temp_dir("search-day-conflict");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_sample_session(&source);
    index_sources(&db, &[&source]);

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "search",
            "webhook",
            "--day",
            "2026-04-13",
            "--since",
            "7d",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();

    assert!(!search.status.success());
    assert!(String::from_utf8_lossy(&search.stderr).contains("use --day by itself"));
}

#[test]
fn search_accepts_relative_since_days() {
    let temp = temp_dir("relative-since");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_session(
        &source,
        "ancient",
        "/Users/me/projects/acme-api",
        "1970-01-01T01:00:00Z",
    );
    write_session(
        &source,
        "future",
        "/Users/me/projects/acme-api",
        "2999-01-01T01:00:00Z",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "webhook secret", "--since", "7d", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let stdout = String::from_utf8_lossy(&search.stdout);
    assert!(stdout.contains("future"));
    assert!(!stdout.contains("ancient"));
}

#[test]
fn search_prioritizes_current_repo_by_default() {
    let temp = temp_dir("current-repo");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let current_repo = temp.join("project");
    fs::create_dir_all(current_repo.join(".git")).unwrap();

    write_session(
        &source,
        "other",
        "/Users/me/projects/other",
        "2026-04-13T01:00:00Z",
    );
    write_session_file(
        &source,
        "project.jsonl",
        "project",
        current_repo.to_str().unwrap(),
        "2026-04-01T01:00:00Z",
        "The production webhook secret was missing.",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .current_dir(&current_repo)
        .args(["search", "webhook secret", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let first_line = String::from_utf8_lossy(&search.stdout)
        .lines()
        .next()
        .unwrap()
        .to_owned();
    assert!(first_line.contains("project"), "{first_line}");
}

#[test]
fn doctor_json_reports_database_checks() {
    let temp = temp_dir("doctor-json");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_sample_session(&source);

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let doctor = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["doctor", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["db_exists"], true);
    assert_eq!(json["checks"]["quick_check"], "ok");
    assert_eq!(json["checks"]["fts_integrity"], "ok");
}

#[test]
fn doctor_does_not_create_missing_database() {
    let temp = temp_dir("doctor-missing");
    let db = temp.join("missing").join("index.sqlite");

    let doctor = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["doctor", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["db_exists"], false);
    assert_eq!(json["checks"]["quick_check"], "missing");
    assert!(!db.exists(), "doctor should not create the database");
    assert!(
        !db.parent().unwrap().exists(),
        "doctor should not create the database directory"
    );
}

#[test]
fn rebuild_recreates_index_from_current_sources() {
    let temp = temp_dir("rebuild");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_sample_session(&source);

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    fs::remove_dir_all(&source).unwrap();
    fs::create_dir_all(&source).unwrap();

    let rebuild = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["rebuild", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "rebuild failed: {}",
        String::from_utf8_lossy(&rebuild.stderr)
    );

    let stats = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["stats", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(stats.status.success());
    assert!(String::from_utf8_lossy(&stats.stdout).starts_with("0 sessions, 0 events"));
}

#[test]
fn index_reports_progress_for_larger_sources() {
    let temp = temp_dir("progress");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_many_sessions(&source, 101);

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let stderr = String::from_utf8_lossy(&index.stderr);
    assert!(stderr.contains("progress: 100/101 files"), "{stderr}");
    assert!(stderr.contains("bytes"), "{stderr}");
    assert!(stderr.contains("elapsed"), "{stderr}");
    assert!(stderr.contains("eta"), "{stderr}");
    assert!(stderr.contains("current "), "{stderr}");
}

#[test]
fn memories_json_extracts_stable_objects_and_consolidates_evidence() {
    let temp = temp_dir("memories-json");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_memory_session_file(
        &source,
        "first.jsonl",
        "memory-session-1",
        "/Users/me/projects/palabruno",
        "2026-04-13T01:00:00Z",
        &[
            (
                "agent_message",
                "Decision: Teacher stays web-only and mobile billing is Premium-only.",
            ),
            (
                "agent_message",
                "Next step: attach the iOS build to App Store Connect.",
            ),
            (
                "agent_message",
                "Blocked: cargo publish is waiting on a crates.io token.",
            ),
        ],
    );
    write_memory_session_file(
        &source,
        "second.jsonl",
        "memory-session-2",
        "/Users/me/projects/palabruno",
        "2026-04-13T02:00:00Z",
        &[(
            "agent_message",
            "Decision: Teacher stays web-only and mobile billing is Premium-only.",
        )],
    );
    index_sources(&db, &[&source]);

    let memories = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["memories", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        memories.status.success(),
        "memories failed: {}",
        String::from_utf8_lossy(&memories.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&memories.stdout).unwrap();
    assert_eq!(json["object"], "list");
    let results = json["results"].as_array().unwrap();
    assert!(
        results.len() >= 3,
        "{}",
        serde_json::to_string_pretty(&json).unwrap()
    );

    let decision = results
        .iter()
        .find(|item| item["kind"] == "decision")
        .unwrap();
    assert_eq!(decision["object"], "memory");
    assert!(decision["id"]
        .as_str()
        .unwrap()
        .starts_with("mem_decision_"));
    assert_eq!(decision["evidence_count"], 2);
    assert_eq!(
        decision["summary"],
        "Teacher stays web-only and mobile billing is Premium-only."
    );
    assert_eq!(
        decision["resource_uri"],
        format!("agent-recall://memory/{}", decision["id"].as_str().unwrap())
    );
}

#[test]
fn memory_resources_and_read_resource_return_mcp_ready_json() {
    let temp = temp_dir("memory-resources");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_memory_session_file(
        &source,
        "resource.jsonl",
        "resource-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        &[(
            "agent_message",
            "Decision: Keep the watcher LaunchAgent generic.",
        )],
    );
    index_sources(&db, &[&source]);

    let resources = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["resources", "--kind", "memory", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        resources.status.success(),
        "resources failed: {}",
        String::from_utf8_lossy(&resources.stderr)
    );
    let resources_json: serde_json::Value = serde_json::from_slice(&resources.stdout).unwrap();
    assert_eq!(resources_json["object"], "resource_list");
    let uri = resources_json["resources"][0]["uri"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(uri.starts_with("agent-recall://memory/"), "{uri}");

    let read = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["read-resource", &uri, "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "read-resource failed: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    let read_json: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(read_json["object"], "memory");
    assert_eq!(read_json["resource_uri"], uri);
    assert_eq!(read_json["evidence"][0]["session_id"], "resource-session");
}

#[test]
fn delta_uses_cursors_for_incremental_sessions_and_memories() {
    let temp = temp_dir("delta");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_memory_session_file(
        &source,
        "first.jsonl",
        "delta-session-1",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        &[("agent_message", "Decision: Keep MCP resources JSON-only.")],
    );
    index_sources(&db, &[&source]);

    let first = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["delta", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "delta failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let cursor = first_json["next_cursor"].as_str().unwrap().to_owned();
    assert!(first_json["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["object"] == "memory"));
    assert!(first_json["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["object"] == "session"));

    write_memory_session_file(
        &source,
        "second.jsonl",
        "delta-session-2",
        "/Users/me/projects/agent-recall",
        "2026-04-13T02:00:00Z",
        &[(
            "agent_message",
            "Next step: wire delta cursors into the CLI.",
        )],
    );
    index_sources(&db, &[&source]);

    let second = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["delta", "--cursor", &cursor, "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "delta with cursor failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let items = second_json["items"].as_array().unwrap();
    assert!(
        !items.is_empty(),
        "{}",
        serde_json::to_string_pretty(&second_json).unwrap()
    );
    assert!(items.iter().all(|item| {
        item["session_id"] == "delta-session-2"
            || item["summary"] == "wire delta cursors into the CLI."
    }));
}

#[test]
fn related_finds_sessions_and_memories_via_shared_memory_objects() {
    let temp = temp_dir("related");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_memory_session_file(
        &source,
        "first.jsonl",
        "related-session-1",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        &[
            (
                "agent_message",
                "Decision: Keep the watcher LaunchAgent generic.",
            ),
            (
                "agent_message",
                "Task: add a resources command for MCP clients.",
            ),
        ],
    );
    write_memory_session_file(
        &source,
        "second.jsonl",
        "related-session-2",
        "/Users/me/projects/agent-recall",
        "2026-04-13T02:00:00Z",
        &[(
            "agent_message",
            "Decision: Keep the watcher LaunchAgent generic.",
        )],
    );
    index_sources(&db, &[&source]);

    let related = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["related", "related-session-1", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        related.status.success(),
        "related failed: {}",
        String::from_utf8_lossy(&related.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&related.stdout).unwrap();
    assert_eq!(json["object"], "related_context");
    assert_eq!(json["reference"]["object"], "session");
    assert!(json["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|session| session["session_id"] == "related-session-2"));
    assert!(json["memories"]
        .as_array()
        .unwrap()
        .iter()
        .any(|memory| memory["kind"] == "decision"));
}

#[test]
fn search_trace_and_eval_fixture_work_for_agent_memory_retrieval() {
    let temp = temp_dir("trace-eval");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_memory_session_file(
        &source,
        "trace.jsonl",
        "trace-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        &[(
            "agent_message",
            "Decision: Keep resources exposed as agent-recall URIs.",
        )],
    );
    index_sources(&db, &[&source]);

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "agent-recall uris", "--trace", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let search_json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    assert_eq!(search_json["match_strategy"], "all_terms");
    assert_eq!(
        search_json["trace"]["fts_query"],
        "\"agent\" AND \"recall\" AND \"uris\""
    );
    assert_eq!(search_json["trace"]["query_terms"][0], "agent");
    assert!(
        search_json["results"][0]["trace"]["session_hit_count"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert!(search_json["results"][0]["trace"]["fts_score"].is_number());
    assert!(search_json["results"][0]["trace"]["source_priority"].is_number());
    assert_eq!(
        search_json["results"][0]["trace"]["duplicate_session_id"],
        "trace-session"
    );

    let fixture = temp.join("eval.json");
    fs::write(
        &fixture,
        r#"{
  "cases": [
    {
      "name": "search recall",
      "command": "search",
      "query": "agent-recall uris",
      "expected": { "session_id": "trace-session" }
    },
    {
      "name": "memory recall",
      "command": "memories",
      "query": "agent-recall uris",
      "expected": {
        "top_memory_kind": "decision",
        "top_memory_summary_contains": "agent-recall URIs"
      }
    },
    {
      "name": "delta change feed",
      "command": "delta",
      "expected": {
        "contains_session_id": "trace-session",
        "contains_memory_kind": "decision",
        "next_cursor_present": true
      }
    }
  ]
}"#,
    )
    .unwrap();

    let eval = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["eval", fixture.to_str().unwrap(), "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        eval.status.success(),
        "eval failed: {}",
        String::from_utf8_lossy(&eval.stderr)
    );
    let eval_json: serde_json::Value = serde_json::from_slice(&eval.stdout).unwrap();
    assert_eq!(eval_json["object"], "eval_result");
    assert_eq!(eval_json["passed"], 3);
    assert_eq!(eval_json["failed"], 0);
}

#[test]
fn delta_cursor_uses_monotonic_change_feed_ids() {
    let temp = temp_dir("delta-change-feed");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_memory_session_file(
        &source,
        "first.jsonl",
        "change-feed-1",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        &[(
            "agent_message",
            "Decision: Keep the delta feed append-only.",
        )],
    );
    index_sources(&db, &[&source]);

    let first = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["delta", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(first.status.success());
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let first_cursor = first_json["next_cursor"].as_str().unwrap().to_owned();
    assert!(first_cursor.starts_with("chg_"), "{first_cursor}");
    assert!(first_json["items"].as_array().unwrap()[0]["change_id"].is_number());
    assert_eq!(
        first_json["items"].as_array().unwrap()[0]["change_kind"],
        "session"
    );

    write_memory_session_file(
        &source,
        "second.jsonl",
        "change-feed-2",
        "/Users/me/projects/agent-recall",
        "2026-04-13T02:00:00Z",
        &[(
            "agent_message",
            "Task: add stronger trace fields for search.",
        )],
    );
    index_sources(&db, &[&source]);

    let second = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["delta", "--cursor", &first_cursor, "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(second.status.success());
    let second_json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let items = second_json["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items
        .iter()
        .all(|item| item["change_id"].as_u64().unwrap() > 0));
    assert!(items
        .iter()
        .any(|item| item["session_id"] == "change-feed-2"));
    assert!(items.iter().any(|item| item["kind"] == "task"));
}

#[test]
fn status_json_reports_pending_files_without_creating_missing_database() {
    let temp = temp_dir("status-pending");
    let source = temp.join("sessions");
    let db = temp.join("missing").join("index.sqlite");
    let state = temp.join("state.json");
    write_sample_session(&source);

    let status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["db_exists"], false);
    assert_eq!(json["freshness"], "watcher-not-running");
    assert_eq!(json["pending_files"], 1);
    assert_eq!(json["last_error"], serde_json::Value::Null);
    assert_eq!(json["launch_agent"]["supported"], cfg!(target_os = "macos"));
    assert!(!db.exists(), "status should not create the database");
}

#[test]
fn status_json_reports_fresh_and_live_write_verdicts() {
    let temp = temp_dir("status-freshness");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "watch",
            "--once",
            "--quiet-for",
            "0",
            "--interval",
            "0",
            "--db",
        ])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(watch.status.success());

    let fresh = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(fresh.status.success());
    let json: serde_json::Value = serde_json::from_slice(&fresh.stdout).unwrap();
    assert_eq!(json["freshness"], "fresh");
    assert_eq!(json["freshness_message"], "index is current");
    assert_eq!(json["launch_agent"]["supported"], cfg!(target_os = "macos"));

    write_session_file(
        &source,
        "live.jsonl",
        "live-write",
        "/Users/me/project",
        "2026-04-13T02:00:00Z",
        "A live write is still settling.",
    );
    let live = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--quiet-for", "86400", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(live.status.success());
    let json: serde_json::Value = serde_json::from_slice(&live.stdout).unwrap();
    assert_eq!(json["freshness"], "pending-live-writes");
    assert_eq!(json["waiting_files"], 1);
}

#[test]
fn status_json_describes_mixed_stable_and_live_backlog() {
    let temp = temp_dir("status-mixed-backlog");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);

    let initial_watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["watch", "--once", "--quiet-for", "0", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(initial_watch.status.success());

    write_session_file(
        &source,
        "stable.jsonl",
        "stable-session",
        "/Users/me/project",
        "2026-04-13T03:00:00Z",
        "The stable pending session should index next.",
    );
    thread::sleep(Duration::from_millis(1100));
    write_session_file(
        &source,
        "live.jsonl",
        "live-session",
        "/Users/me/project",
        "2026-04-13T04:00:00Z",
        "The live write is still settling.",
    );

    let status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--quiet-for", "1", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(status.status.success());
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["freshness"], "stale");
    assert_eq!(json["stable_pending_files"], 1);
    assert_eq!(json["waiting_files"], 1);
    let message = json["freshness_message"].as_str().unwrap();
    assert!(
        message.contains("1 stable files are ready to index"),
        "{message}"
    );
    assert!(
        message.contains("1 files are still within the quiet window"),
        "{message}"
    );
}

#[test]
fn watch_once_indexes_stable_files_even_when_live_writes_are_still_settling() {
    let temp = temp_dir("watch-stable-while-live");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);
    index_sources(&db, &[&source]);

    write_session_file(
        &source,
        "stable.jsonl",
        "stable-backlog",
        "/Users/me/project",
        "2026-04-13T03:00:00Z",
        "The stable pending session should index now.",
    );
    thread::sleep(Duration::from_millis(1100));
    write_session_file(
        &source,
        "live.jsonl",
        "live-write",
        "/Users/me/project",
        "2026-04-13T04:00:00Z",
        "The live write is still settling.",
    );

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["watch", "--once", "--quiet-for", "1", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&watch.stderr)
    );
    let stdout = String::from_utf8_lossy(&watch.stdout);
    assert!(stdout.contains("watch indexed 1 session files"), "{stdout}");
    assert!(
        stdout.contains("1 pending files remain (1 still within quiet window)"),
        "{stdout}"
    );

    let stable_search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "stable pending session", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(stable_search.status.success());
    assert!(
        String::from_utf8_lossy(&stable_search.stdout).contains("stable-backlog"),
        "{}",
        String::from_utf8_lossy(&stable_search.stdout)
    );

    let live_search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "live write is still settling", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(live_search.status.success());
    assert!(
        String::from_utf8_lossy(&live_search.stdout).contains("no matches"),
        "{}",
        String::from_utf8_lossy(&live_search.stdout)
    );

    let status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--quiet-for", "1", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(status.status.success());
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["freshness"], "pending-live-writes");
    assert_eq!(json["pending_files"], 1);
    assert_eq!(json["stable_pending_files"], 0);
    assert_eq!(json["waiting_files"], 1);
}

#[test]
fn status_json_reports_using_stale_index_after_lock_failure() {
    let temp = temp_dir("status-stale-lock");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);
    index_sources(&db, &[&source]);
    fs::write(
        &state,
        r#"{"last_run_at":"2026-04-13T01:00:00.000Z","last_indexed_at":"2026-04-13T01:00:00.000Z","last_error":"database is locked; using stale index","last_indexed_sessions":1,"last_indexed_events":2,"last_files_seen":1,"last_files_total":1,"pending_files":1}"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["freshness"], "using-stale-index");
    assert!(json["freshness_message"]
        .as_str()
        .unwrap()
        .contains("database is locked"));
    assert!(json["freshness_message"]
        .as_str()
        .unwrap()
        .contains("stale index"));
}

#[test]
fn status_json_reports_refreshing_when_refresh_lock_is_active() {
    let temp = temp_dir("status-refreshing");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_session_file(
        &source,
        "old.jsonl",
        "old-session",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
        "The old indexed session remains available.",
    );
    index_sources(&db, &[&source]);
    write_session_file(
        &source,
        "new.jsonl",
        "new-session",
        "/Users/me/project",
        "2026-04-13T02:00:00Z",
        "The new pending session should wait for the active refresh.",
    );
    let lock_path = PathBuf::from(format!("{}.refresh.lock", db.display()));
    fs::write(
        &lock_path,
        format!("pid={}\ncreated_at=test\n", std::process::id()),
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("AGENT_RECALL_REFRESH_LOCK_STALE_MS", "0")
        .args(["status", "--json", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["freshness"], "refreshing");
    assert!(json["freshness_message"]
        .as_str()
        .unwrap()
        .contains("current index remains usable"));
    assert_eq!(json["pending_files"], 1);
    assert_eq!(json["refresh_lock"]["exists"], true);
    assert_eq!(json["refresh_lock"]["active"], true);
    assert_eq!(json["refresh_lock"]["stale"], false);
    assert_eq!(
        json["refresh_lock"]["pid"],
        serde_json::json!(std::process::id())
    );
    assert_eq!(json["refresh_lock"]["owner_alive"], true);
}

#[test]
fn status_json_does_not_report_refreshing_for_stale_refresh_lock() {
    let temp = temp_dir("status-stale-refresh-lock");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::write(format!("{}.refresh.lock", db.display()), "active refresh\n").unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("AGENT_RECALL_REFRESH_LOCK_STALE_MS", "0")
        .args(["status", "--json", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["freshness"], "stale");
    assert!(json["freshness_message"]
        .as_str()
        .unwrap()
        .contains("refresh lock appears stale"));
    assert_eq!(json["refresh_lock"]["exists"], true);
    assert_eq!(json["refresh_lock"]["active"], false);
    assert_eq!(json["refresh_lock"]["stale"], true);
}

#[test]
fn watch_once_uses_stale_index_when_refresh_database_is_locked() {
    let temp = temp_dir("watch-stale-lock");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_session_file(
        &source,
        "old.jsonl",
        "old-session",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
        "The old indexed session is still searchable.",
    );
    index_sources(&db, &[&source]);
    write_session_file(
        &source,
        "new.jsonl",
        "new-session",
        "/Users/me/project",
        "2026-04-13T02:00:00Z",
        "The new pending session is not indexed yet.",
    );

    let lock = Connection::open(&db).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("AGENT_RECALL_SQLITE_BUSY_TIMEOUT_MS", "1")
        .env("AGENT_RECALL_WATCH_LOCK_RETRY_MS", "1")
        .env("AGENT_RECALL_WATCH_LOCK_RETRIES", "2")
        .args(["watch", "--once", "--quiet-for", "0", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&watch.stderr)
    );
    let stdout = String::from_utf8_lossy(&watch.stdout);
    assert!(stdout.contains("using stale index"), "{stdout}");
    assert!(stdout.contains("database is locked"), "{stdout}");

    let status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(status.status.success());
    let json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["freshness"], "using-stale-index");
    assert_eq!(json["pending_files"], 1);

    drop(lock);

    let old_search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "old indexed", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(old_search.status.success());
    let stdout = String::from_utf8_lossy(&old_search.stdout);
    assert!(stdout.contains("old-session"), "{stdout}");

    let new_search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "new pending", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(new_search.status.success());
    let stdout = String::from_utf8_lossy(&new_search.stdout);
    assert!(stdout.contains("no matches"), "{stdout}");
}

#[test]
fn watch_once_uses_current_index_when_refresh_lock_is_busy() {
    let temp = temp_dir("watch-refresh-lock-busy");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_session_file(
        &source,
        "old.jsonl",
        "old-session",
        "/Users/me/project",
        "2026-04-13T01:00:00Z",
        "The old indexed session remains available.",
    );
    index_sources(&db, &[&source]);
    write_session_file(
        &source,
        "new.jsonl",
        "new-session",
        "/Users/me/project",
        "2026-04-13T02:00:00Z",
        "The new pending session should wait for the active refresh.",
    );
    fs::write(format!("{}.refresh.lock", db.display()), "active refresh").unwrap();

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("AGENT_RECALL_REFRESH_LOCK_WAIT_MS", "1")
        .env("AGENT_RECALL_REFRESH_LOCK_STALE_MS", "600000")
        .args(["watch", "--once", "--quiet-for", "0", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&watch.stderr)
    );
    let stdout = String::from_utf8_lossy(&watch.stdout);
    assert!(
        stdout.contains("another refresh is already active"),
        "{stdout}"
    );

    let new_search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "new pending", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(new_search.status.success());
    assert!(
        String::from_utf8_lossy(&new_search.stdout).contains("no matches"),
        "{}",
        String::from_utf8_lossy(&new_search.stdout)
    );
}

#[test]
fn watch_once_waits_for_refresh_lock_then_indexes() {
    let temp = temp_dir("watch-refresh-lock-waits");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);
    let lock_path = PathBuf::from(format!("{}.refresh.lock", db.display()));
    fs::write(&lock_path, "active refresh").unwrap();
    let lock_to_remove = lock_path.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        let _ = fs::remove_file(lock_to_remove);
    });

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("AGENT_RECALL_REFRESH_LOCK_WAIT_MS", "2000")
        .env("AGENT_RECALL_REFRESH_LOCK_STALE_MS", "600000")
        .args(["watch", "--once", "--quiet-for", "0", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&watch.stderr)
    );
    let stdout = String::from_utf8_lossy(&watch.stdout);
    assert!(stdout.contains("watch indexed"), "{stdout}");
    assert!(!lock_path.exists());
}

#[test]
fn watch_once_indexes_stable_sessions_and_writes_status() {
    let temp = temp_dir("watch-once");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "watch",
            "--once",
            "--quiet-for",
            "0",
            "--interval",
            "0",
            "--db",
        ])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed: {}",
        String::from_utf8_lossy(&watch.stderr)
    );

    let stdout = String::from_utf8_lossy(&watch.stdout);
    assert!(stdout.contains("watch indexed"), "{stdout}");

    let stats = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["stats", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(stats.status.success());
    assert!(String::from_utf8_lossy(&stats.stdout).starts_with("1 sessions, 2 events"));

    let json: serde_json::Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    assert_eq!(json["pending_files"], 0);
    assert_eq!(json["last_error"], serde_json::Value::Null);
    assert_eq!(json["last_indexed_sessions"], 1);
    assert!(json["last_run_at"].as_str().is_some());
}

#[test]
fn status_and_doctor_support_repo_and_since_filters() {
    let temp = temp_dir("status-doctor-filters");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch.json");
    let old_dir = source.join("2026/04/12");
    fs::create_dir_all(&old_dir).unwrap();
    fs::write(old_dir.join("broken-old.jsonl"), "{not-json\n").unwrap();
    write_session_file(
        &source,
        "matching.jsonl",
        "matching-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T01:00:00Z",
        "Filtered status should stay focused on this session.",
    );

    let watch = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "watch",
            "--once",
            "--quiet-for",
            "0",
            "--repo",
            "agent-recall",
            "--since",
            "2026-04-13",
            "--db",
        ])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        watch.status.success(),
        "watch failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&watch.stdout),
        String::from_utf8_lossy(&watch.stderr)
    );

    let unfiltered_status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["status", "--json", "--quiet-for", "0", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(unfiltered_status.status.success());
    let json: serde_json::Value = serde_json::from_slice(&unfiltered_status.stdout).unwrap();
    assert_eq!(json["freshness"], "stale");
    assert_eq!(json["pending_files"], 1);

    let filtered_status = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "status",
            "--json",
            "--quiet-for",
            "0",
            "--repo",
            "agent-recall",
            "--since",
            "2026-04-13",
            "--db",
        ])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(filtered_status.status.success());
    let json: serde_json::Value = serde_json::from_slice(&filtered_status.stdout).unwrap();
    assert_eq!(json["freshness"], "fresh");
    assert_eq!(json["pending_files"], 0);
    assert_eq!(json["filters"]["repo"], "agent-recall");
    assert_eq!(json["filters"]["since"], "2026-04-13");

    let filtered_doctor = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "doctor",
            "--json",
            "--quiet-for",
            "0",
            "--repo",
            "agent-recall",
            "--since",
            "2026-04-13",
            "--db",
        ])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(filtered_doctor.status.success());
    let json: serde_json::Value = serde_json::from_slice(&filtered_doctor.stdout).unwrap();
    assert_eq!(json["freshness"]["state"], "fresh");
    assert_eq!(
        json["freshness"]["status"]["filters"]["repo"],
        "agent-recall"
    );
    assert_eq!(
        json["freshness"]["status"]["filters"]["since"],
        "2026-04-13"
    );
}

#[test]
fn watch_installs_launch_agent_plist_when_requested() {
    let temp = temp_dir("watch-launch-agent");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    let agent = temp.join("com.example.agent-recall.watch.plist");
    fs::create_dir_all(&source).unwrap();

    let install = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "watch",
            "--install-launch-agent",
            "--agent-label",
            "com.example.agent-recall.watch",
            "--agent-path",
        ])
        .arg(&agent)
        .args(["--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .args(["--interval", "60", "--quiet-for", "5"])
        .output()
        .unwrap();
    if !cfg!(target_os = "macos") {
        assert!(!install.status.success());
        assert!(
            String::from_utf8_lossy(&install.stderr)
                .contains("watch --install-launch-agent is only supported on macOS"),
            "{}",
            String::from_utf8_lossy(&install.stderr)
        );
        return;
    }
    assert!(
        install.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let plist = fs::read_to_string(&agent).unwrap();
    assert!(plist.contains("<key>Label</key>"), "{plist}");
    assert!(plist.contains("com.example.agent-recall.watch"), "{plist}");
    assert!(plist.contains("<string>watch</string>"), "{plist}");
    assert!(plist.contains("<string>--interval</string>"), "{plist}");
    assert!(plist.contains("<string>60</string>"), "{plist}");
    assert!(plist.contains(source.to_str().unwrap()), "{plist}");
}

#[test]
fn watch_can_install_and_start_launch_agent_with_configurable_launchctl() {
    let temp = temp_dir("watch-launch-agent-start");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    let agent = temp.join("com.example.agent-recall.watch.plist");
    let launchctl_log = temp.join("launchctl.log");
    let fake_launchctl = temp.join("launchctl");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        &fake_launchctl,
        format!(
            "#!/bin/sh\nprintf '%s ' \"$@\" >> {}\nprintf '\\n' >> {}\nexit 0\n",
            launchctl_log.display(),
            launchctl_log.display()
        ),
    )
    .unwrap();
    let chmod = Command::new("chmod")
        .args(["+x"])
        .arg(&fake_launchctl)
        .output()
        .unwrap();
    assert!(chmod.status.success());

    let install = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("AGENT_RECALL_LAUNCHCTL", &fake_launchctl)
        .env("AGENT_RECALL_UID", "501")
        .args([
            "watch",
            "--install-launch-agent",
            "--start-launch-agent",
            "--agent-label",
            "com.example.agent-recall.watch",
            "--agent-path",
        ])
        .arg(&agent)
        .args(["--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .args(["--interval", "60", "--quiet-for", "5"])
        .output()
        .unwrap();
    if !cfg!(target_os = "macos") {
        assert!(!install.status.success());
        assert!(
            String::from_utf8_lossy(&install.stderr)
                .contains("watch --install-launch-agent is only supported on macOS"),
            "{}",
            String::from_utf8_lossy(&install.stderr)
        );
        return;
    }
    assert!(
        install.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let stdout = String::from_utf8_lossy(&install.stdout);
    assert!(stdout.contains("installed launch agent"), "{stdout}");
    assert!(stdout.contains("started launch agent"), "{stdout}");
    let log = fs::read_to_string(launchctl_log).unwrap();
    assert!(log.contains("bootstrap gui/501"), "{log}");
    assert!(log.contains(agent.to_str().unwrap()), "{log}");
    assert!(
        log.contains("print gui/501/com.example.agent-recall.watch"),
        "{log}"
    );
}

#[test]
fn default_watch_install_retires_legacy_launch_agent() {
    if !cfg!(target_os = "macos") {
        return;
    }

    let temp = temp_dir("watch-launch-agent-retire-legacy");
    let home = temp.join("home");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    let agent = temp.join("dev.agent-recall.watch.plist");
    let launchctl_log = temp.join("launchctl.log");
    let fake_launchctl = temp.join("launchctl");
    let legacy_agent = home
        .join("Library")
        .join("LaunchAgents")
        .join("com.hanif.codex-recall.watch.plist");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(legacy_agent.parent().unwrap()).unwrap();
    fs::write(&legacy_agent, "<plist/>").unwrap();
    fs::write(
        &fake_launchctl,
        format!(
            "#!/bin/sh\nprintf '%s ' \"$@\" >> {}\nprintf '\\n' >> {}\nexit 0\n",
            launchctl_log.display(),
            launchctl_log.display()
        ),
    )
    .unwrap();
    let chmod = Command::new("chmod")
        .args(["+x"])
        .arg(&fake_launchctl)
        .output()
        .unwrap();
    assert!(chmod.status.success());

    let install = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .env("AGENT_RECALL_LAUNCHCTL", &fake_launchctl)
        .env("AGENT_RECALL_UID", "501")
        .env("HOME", &home)
        .args([
            "watch",
            "--install-launch-agent",
            "--start-launch-agent",
            "--agent-path",
        ])
        .arg(&agent)
        .args(["--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let stdout = String::from_utf8_lossy(&install.stdout);
    assert!(
        stdout.contains("retired legacy launch agent: com.hanif.codex-recall.watch"),
        "{stdout}"
    );
    assert!(!legacy_agent.exists());
    let log = fs::read_to_string(launchctl_log).unwrap();
    assert!(
        log.contains("print gui/501/com.hanif.codex-recall.watch"),
        "{log}"
    );
    assert!(
        log.contains("bootout gui/501/com.hanif.codex-recall.watch"),
        "{log}"
    );
    assert!(log.contains("bootstrap gui/501"), "{log}");
    assert!(
        log.contains("print gui/501/dev.agent-recall.watch"),
        "{log}"
    );
}

#[test]
fn doctor_json_includes_freshness_verdict() {
    let temp = temp_dir("doctor-freshness");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let state = temp.join("watch-state.json");
    write_sample_session(&source);

    let doctor = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["doctor", "--json", "--db"])
        .arg(&db)
        .args(["--state"])
        .arg(&state)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(doctor.status.success());
    let json: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert_eq!(json["freshness"]["state"], "watcher-not-running");
    assert!(json["freshness"]["message"]
        .as_str()
        .unwrap()
        .contains("watcher has no state"));
}

#[test]
fn bundle_outputs_agent_ready_context() {
    let temp = temp_dir("bundle");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_session_file(
        &source,
        "revenuecat.jsonl",
        "revenuecat",
        "/Users/me/projects/payments",
        "2026-04-13T01:00:00Z",
        "The RevenueCat webhook secret was missing from production.",
    );
    write_session_file(
        &source,
        "stripe.jsonl",
        "stripe",
        "/Users/me/projects/payments",
        "2026-04-13T02:00:00Z",
        "Stripe retries were caused by the missing webhook secret.",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let bundle = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["bundle", "webhook secret", "--db"])
        .arg(&db)
        .args(["--limit", "2", "--repo", "payments"])
        .output()
        .unwrap();
    assert!(
        bundle.status.success(),
        "bundle failed: {}",
        String::from_utf8_lossy(&bundle.stderr)
    );

    let stdout = String::from_utf8_lossy(&bundle.stdout);
    assert!(stdout.contains("# agent-recall bundle"), "{stdout}");
    assert!(stdout.contains("Query: webhook secret"), "{stdout}");
    assert!(stdout.contains("## Top Sessions"), "{stdout}");
    assert!(stdout.contains("## Receipts"), "{stdout}");
    assert!(stdout.contains("revenuecat"), "{stdout}");
    assert!(stdout.contains("stripe"), "{stdout}");
    assert!(stdout.contains("agent-recall show"), "{stdout}");
}

#[test]
fn recent_lists_latest_sessions_with_filters() {
    let temp = temp_dir("recent");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_session(
        &source,
        "old-project",
        "/Users/me/projects/project",
        "2026-04-01T01:00:00Z",
    );
    write_session(
        &source,
        "new-project",
        "/Users/me/projects/project",
        "2026-04-13T02:00:00Z",
    );
    write_session(
        &source,
        "new-other",
        "/Users/me/projects/other",
        "2026-04-13T03:00:00Z",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let recent = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "recent",
            "--repo",
            "project",
            "--since",
            "2026-04-10",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        recent.status.success(),
        "recent failed: {}",
        String::from_utf8_lossy(&recent.stderr)
    );

    let stdout = String::from_utf8_lossy(&recent.stdout);
    assert!(stdout.contains("new-project"), "{stdout}");
    assert!(stdout.contains("agent-recall show"), "{stdout}");
    assert!(!stdout.contains("old-project"), "{stdout}");
    assert!(!stdout.contains("new-other"), "{stdout}");
}

#[test]
fn recent_filters_by_from_until_exclusion_and_dedupes_duplicates() {
    let temp = temp_dir("recent-range-dedupe");
    let active = temp.join("sessions");
    let archived = temp.join("archived_sessions");
    let db = temp.join("index.sqlite");
    write_session(
        &active,
        "too-old",
        "/Users/me/projects/project",
        "2026-04-01T01:00:00Z",
    );
    write_session_file(
        &active,
        "active.jsonl",
        "session-dup",
        "/Users/me/projects/project",
        "2026-04-13T01:00:00Z",
        "Active duplicate.",
    );
    write_session_file(
        &archived,
        "archived.jsonl",
        "session-dup",
        "/Users/me/projects/project",
        "2026-04-13T01:00:00Z",
        "Archived duplicate.",
    );
    write_session(
        &active,
        "excluded",
        "/Users/me/projects/project",
        "2026-04-13T02:00:00Z",
    );
    write_session(
        &active,
        "too-new",
        "/Users/me/projects/project",
        "2026-04-15T01:00:00Z",
    );
    index_sources(&db, &[&active, &archived]);

    let recent = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "recent",
            "--from",
            "2026-04-10",
            "--until",
            "2026-04-14",
            "--exclude-session",
            "excluded",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        recent.status.success(),
        "recent failed: {}",
        String::from_utf8_lossy(&recent.stderr)
    );

    let stdout = String::from_utf8_lossy(&recent.stdout);
    assert!(stdout.contains("session-dup"), "{stdout}");
    assert!(stdout.contains("/sessions/"), "{stdout}");
    assert!(!stdout.contains("/archived_sessions/"), "{stdout}");
    assert!(!stdout.contains("too-old"), "{stdout}");
    assert!(!stdout.contains("excluded"), "{stdout}");
    assert!(!stdout.contains("too-new"), "{stdout}");

    let duplicate_recent = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["recent", "--include-duplicates", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(duplicate_recent.status.success());
    let stdout = String::from_utf8_lossy(&duplicate_recent.stdout);
    assert!(stdout.contains("/sessions/"), "{stdout}");
    assert!(stdout.contains("/archived_sessions/"), "{stdout}");
}

#[test]
fn recent_json_supports_day_and_kind_filter() {
    let temp = temp_dir("recent-json-day-kind");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_mixed_session_file(
        &source,
        "with-command.jsonl",
        "with-command",
        "/Users/me/projects/project",
        "2026-04-13T01:00:00Z",
    );
    write_session(
        &source,
        "other-day",
        "/Users/me/projects/project",
        "2026-04-14T01:00:00Z",
    );
    index_sources(&db, &[&source]);

    let recent = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "recent",
            "--day",
            "2026-04-13",
            "--kind",
            "command",
            "--json",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        recent.status.success(),
        "recent failed: {}",
        String::from_utf8_lossy(&recent.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&recent.stdout).unwrap();
    assert_eq!(json["count"], 1);
    assert_eq!(json["sessions"][0]["session_id"], "with-command");
}

#[test]
fn recent_accepts_all_repos_for_search_parity() {
    let temp = temp_dir("recent-all-repos");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let workspace = temp.join("project");
    fs::create_dir_all(workspace.join(".git")).unwrap();
    write_session(
        &source,
        "project-session",
        workspace.to_str().unwrap(),
        "2026-04-13T01:00:00Z",
    );
    write_session(
        &source,
        "other-session",
        "/Users/me/projects/other",
        "2026-04-13T02:00:00Z",
    );
    index_sources(&db, &[&source]);

    let recent = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .current_dir(&workspace)
        .args(["recent", "--all-repos", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        recent.status.success(),
        "recent failed: {}",
        String::from_utf8_lossy(&recent.stderr)
    );

    let stdout = String::from_utf8_lossy(&recent.stdout);
    assert!(stdout.contains("project-session"), "{stdout}");
    assert!(stdout.contains("other-session"), "{stdout}");
}

#[test]
fn show_json_supports_kind_filter() {
    let temp = temp_dir("show-json-kind");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_mixed_session_file(
        &source,
        "mixed.jsonl",
        "mixed-session",
        "/Users/me/projects/project",
        "2026-04-13T01:00:00Z",
    );
    index_sources(&db, &[&source]);

    let show = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["show", "mixed-session", "--kind", "user", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "show failed: {}",
        String::from_utf8_lossy(&show.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(json["session_id"], "mixed-session");
    assert_eq!(json["events"].as_array().unwrap().len(), 1);
    assert_eq!(json["events"][0]["kind"], "user_message");
}

#[test]
fn day_json_outputs_session_inventory() {
    let temp = temp_dir("day-json");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    write_session(
        &source,
        "project-one",
        "/Users/me/projects/project",
        "2026-04-13T01:00:00Z",
    );
    write_session(
        &source,
        "other-one",
        "/Users/me/projects/other",
        "2026-04-13T02:00:00Z",
    );
    write_session(
        &source,
        "next-day",
        "/Users/me/projects/project",
        "2026-04-14T01:00:00Z",
    );
    index_sources(&db, &[&source]);

    let day = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["day", "2026-04-13", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        day.status.success(),
        "day failed: {}",
        String::from_utf8_lossy(&day.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&day.stdout).unwrap();
    assert_eq!(json["day"], "2026-04-13");
    assert_eq!(json["from"], "2026-04-13");
    assert_eq!(json["until"], "2026-04-14");
    assert_eq!(json["count"], 2);
    assert_eq!(json["repo_counts"]["project"], 1);
    assert_eq!(json["repo_counts"]["other"], 1);
}

#[test]
fn pin_stores_durable_session_anchor_and_pins_filters_it() {
    let temp = temp_dir("pins");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let pins = temp.join("pins.json");
    write_session_file(
        &source,
        "watcher.jsonl",
        "watcher-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T02:00:00Z",
        "LaunchAgent watcher freshness decision.",
    );
    write_session_file(
        &source,
        "other.jsonl",
        "other-session",
        "/Users/me/projects/other",
        "2026-04-13T03:00:00Z",
        "Different project decision.",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "watcher freshness", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(search.status.success());
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    let session_key = json["results"][0]["session_key"].as_str().unwrap();

    let pin = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pin", session_key, "--label", "watcher design", "--db"])
        .arg(&db)
        .args(["--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(
        pin.status.success(),
        "pin failed: {}",
        String::from_utf8_lossy(&pin.stderr)
    );
    assert!(String::from_utf8_lossy(&pin.stdout).contains("pinned watcher-session"));

    let pins_output = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pins", "--repo", "agent-recall", "--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(
        pins_output.status.success(),
        "pins failed: {}",
        String::from_utf8_lossy(&pins_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&pins_output.stdout);
    assert!(stdout.contains("watcher design"), "{stdout}");
    assert!(stdout.contains("watcher-session"), "{stdout}");
    assert!(stdout.contains("agent-recall show"), "{stdout}");
    assert!(!stdout.contains("other-session"), "{stdout}");

    let pins_json = fs::read_to_string(&pins).unwrap();
    assert!(pins_json.contains("watcher design"), "{pins_json}");
    assert!(pins_json.contains(session_key), "{pins_json}");
}

#[test]
fn pins_filter_by_command_cwd_repo_membership() {
    let temp = temp_dir("pins-command-repo");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let pins = temp.join("pins.json");
    let session_dir = source.join("2026/04/13");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("mixed-repo.jsonl"),
        r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"mixed-repo","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/notes-vault","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"exec_command_end","command":["/bin/zsh","-lc","cargo test"],"cwd":"/Users/me/projects/agent-recall","exit_code":0,"aggregated_output":"watcher freshness tested"}}
"#,
    )
    .unwrap();

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "watcher freshness", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(search.status.success());
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    let session_key = json["results"][0]["session_key"].as_str().unwrap();

    let pin = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pin", session_key, "--label", "mixed repo", "--db"])
        .arg(&db)
        .args(["--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(pin.status.success());

    let pins_output = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pins", "--repo", "agent-recall", "--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(pins_output.status.success());
    let stdout = String::from_utf8_lossy(&pins_output.stdout);
    assert!(stdout.contains("mixed repo"), "{stdout}");
    assert!(stdout.contains("agent-recall"), "{stdout}");
}

#[test]
fn pins_json_outputs_machine_readable_pin_records() {
    let temp = temp_dir("pins-json");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let pins = temp.join("pins.json");
    write_session_file(
        &source,
        "watcher.jsonl",
        "watcher-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T02:00:00Z",
        "LaunchAgent watcher freshness decision.",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "watcher freshness", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(search.status.success());
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    let session_key = json["results"][0]["session_key"].as_str().unwrap();

    let pin = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pin", session_key, "--label", "watcher design", "--db"])
        .arg(&db)
        .args(["--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(pin.status.success());

    let pins_output = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pins", "--repo", "agent-recall", "--json", "--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(
        pins_output.status.success(),
        "pins failed: {}",
        String::from_utf8_lossy(&pins_output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&pins_output.stdout).unwrap();
    assert_eq!(json["count"], 1);
    assert_eq!(json["pins"][0]["label"], "watcher design");
    assert_eq!(json["pins"][0]["session_id"], "watcher-session");
    assert_eq!(
        json["pins"][0]["show_command"],
        format!("agent-recall show '{session_key}' --limit 120")
    );
}

#[test]
fn unpin_removes_existing_pin() {
    let temp = temp_dir("unpin");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let pins = temp.join("pins.json");
    write_session_file(
        &source,
        "watcher.jsonl",
        "watcher-session",
        "/Users/me/projects/agent-recall",
        "2026-04-13T02:00:00Z",
        "LaunchAgent watcher freshness decision.",
    );

    let index = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(&db)
        .args(["--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(index.status.success());

    let search = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["search", "watcher freshness", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(search.status.success());
    let json: serde_json::Value = serde_json::from_slice(&search.stdout).unwrap();
    let session_key = json["results"][0]["session_key"].as_str().unwrap();

    let pin = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pin", session_key, "--label", "watcher design", "--db"])
        .arg(&db)
        .args(["--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(pin.status.success());

    let unpin = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["unpin", session_key, "--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(
        unpin.status.success(),
        "unpin failed: {}",
        String::from_utf8_lossy(&unpin.stderr)
    );
    assert!(String::from_utf8_lossy(&unpin.stdout).contains("unpinned watcher-session"));

    let pins_output = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["pins", "--json", "--pins"])
        .arg(&pins)
        .output()
        .unwrap();
    assert!(pins_output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&pins_output.stdout).unwrap();
    assert_eq!(json["count"], 0);
    assert_eq!(json["pins"].as_array().unwrap().len(), 0);
}
