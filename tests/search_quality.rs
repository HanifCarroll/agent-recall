use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent-recall-quality-test-{}-{}",
        std::process::id(),
        name
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_session(root: &Path, file_name: &str, contents: &str) {
    let session_dir = root.join("2026/04/13");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(session_dir.join(file_name), contents).unwrap();
}

fn run_index(source: &Path, db: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args(["index", "--db"])
        .arg(db)
        .args(["--source"])
        .arg(source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn quality_fixture_prioritizes_relevant_repo_membership_and_fallbacks() {
    let temp = temp_dir("ranking");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");
    let current_repo = temp.join("agent-recall");
    fs::create_dir_all(current_repo.join(".git")).unwrap();

    write_session(
        &source,
        "right-command-cwd.jsonl",
        &format!(
            r#"{{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{{"id":"right-command-cwd","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/notes-vault","cli_version":"0.1.0"}}}}
{{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{{"type":"user_message","message":"Dogfood recall ranking against the current repository."}}}}
{{"timestamp":"2026-04-13T01:00:02Z","type":"event_msg","payload":{{"type":"exec_command_end","command":["/bin/zsh","-lc","rg dogfood"],"cwd":"{}","exit_code":0,"aggregated_output":"dogfood ranking receipt"}}}}
"#,
            current_repo.display()
        ),
    );
    write_session(
        &source,
        "newer-other.jsonl",
        r#"{"timestamp":"2026-04-13T02:00:00Z","type":"session_meta","payload":{"id":"newer-other","timestamp":"2026-04-13T02:00:00Z","cwd":"/Users/me/projects/other","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T02:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Dogfood recall ranking against a different repository."}}
"#,
    );
    write_session(
        &source,
        "split-terms.jsonl",
        r#"{"timestamp":"2026-04-13T03:00:00Z","type":"session_meta","payload":{"id":"split-terms","timestamp":"2026-04-13T03:00:00Z","cwd":"/Users/me/projects/agent-recall","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T03:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Need alpha coverage for recall quality."}}
{"timestamp":"2026-04-13T03:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"Beta coverage lives in a separate event."}}
"#,
    );
    run_index(&source, &db);

    let ranked = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .current_dir(&current_repo)
        .args(["search", "dogfood ranking", "--json", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        ranked.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&ranked.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&ranked.stdout).unwrap();
    assert_eq!(json["results"][0]["session_id"], "right-command-cwd");

    let repo_filtered = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "search",
            "dogfood ranking",
            "--repo",
            "agent-recall",
            "--json",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(repo_filtered.status.success());
    let json: serde_json::Value = serde_json::from_slice(&repo_filtered.stdout).unwrap();
    assert_eq!(json["results"][0]["session_id"], "right-command-cwd");

    let fallback = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
        .args([
            "search",
            "alpha beta",
            "--repo",
            "agent-recall",
            "--json",
            "--db",
        ])
        .arg(&db)
        .output()
        .unwrap();
    assert!(fallback.status.success());
    let json: serde_json::Value = serde_json::from_slice(&fallback.stdout).unwrap();
    assert!(json["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|result| result["session_id"] == "split-terms"));
}

#[test]
fn golden_quality_fixture_preserves_agent_workflow_queries() {
    let temp = temp_dir("golden-ranking");
    let source = temp.join("sessions");
    let db = temp.join("index.sqlite");

    write_session(
        &source,
        "clap-refactor.jsonl",
        r#"{"timestamp":"2026-04-13T01:00:00Z","type":"session_meta","payload":{"id":"clap-refactor","timestamp":"2026-04-13T01:00:00Z","cwd":"/Users/me/projects/agent-recall","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T01:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Refactor the CLI parser to Clap typed commands."}}
{"timestamp":"2026-04-13T01:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"Split commands into modules and keep cargo clippy clean."}}
"#,
    );
    write_session(
        &source,
        "watcher-freshness.jsonl",
        r#"{"timestamp":"2026-04-13T02:00:00Z","type":"session_meta","payload":{"id":"watcher-freshness","timestamp":"2026-04-13T02:00:00Z","cwd":"/Users/me/projects/agent-recall","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T02:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Wire LaunchAgent watcher freshness status."}}
{"timestamp":"2026-04-13T02:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"Status should say stale, fresh, watcher-not-running, or pending-live-writes."}}
"#,
    );
    write_session(
        &source,
        "redaction-hardening.jsonl",
        r#"{"timestamp":"2026-04-13T03:00:00Z","type":"session_meta","payload":{"id":"redaction-hardening","timestamp":"2026-04-13T03:00:00Z","cwd":"/Users/me/projects/agent-recall","cli_version":"0.1.0"}}
{"timestamp":"2026-04-13T03:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"Harden secret redaction fixtures for bearer tokens, private keys, and webhook secrets."}}
{"timestamp":"2026-04-13T03:00:02Z","type":"event_msg","payload":{"type":"agent_message","message":"The redaction corpus must not leak API keys into the SQLite FTS index."}}
"#,
    );
    run_index(&source, &db);

    for (query, expected_id) in [
        (
            "clap typed commands command modules cargo clippy",
            "clap-refactor",
        ),
        (
            "launchagent watcher freshness stale pending",
            "watcher-freshness",
        ),
        (
            "secret redaction private key bearer token",
            "redaction-hardening",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_agent-recall"))
            .args(["search", query, "--repo", "agent-recall", "--json", "--db"])
            .arg(&db)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "search failed for {query}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            json["results"][0]["session_id"],
            expected_id,
            "query `{query}` returned {}",
            serde_json::to_string_pretty(&json["results"]).unwrap()
        );
    }
}
