# agent-recall

[![CI](https://github.com/HanifCarroll/agent-recall/actions/workflows/ci.yml/badge.svg)](https://github.com/HanifCarroll/agent-recall/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/agent-recall.svg)](https://crates.io/crates/agent-recall)

Local search and recall for Codex, OMP, Pi, and Claude session JSONL archives, with an optional forensic importer for archived Codex SQLite telemetry logs.

`agent-recall` builds a disposable SQLite FTS5 index over transcript archives so you can search, inspect, and reuse prior session context without treating raw JSONL logs as a database. Results carry explicit source labels such as `codex`, `omp`, `pi`, `claude`, and optional `codex-log`.

Raw JSONL files remain the source of truth. Archived Codex SQLite logs are a lower-priority fallback for conversations whose JSONL transcript was cleaned up or lost.

## Install

```bash
cargo install agent-recall
```

Or install directly from GitHub:

```bash
cargo install --git https://github.com/HanifCarroll/agent-recall
```

Build from source:

```bash
cargo install --path .
```

## Quick Start

Index a recent slice of your local agent archives, then query them. Use an
unbounded `agent-recall index` only when you really want the full history.

```bash
agent-recall index --since 30d
agent-recall search "payment webhook"
agent-recall memories "launch agent"
agent-recall delta --json
agent-recall recent --since 7d
agent-recall doctor --json
```

If old Codex JSONL files were cleaned up, optionally import archived Codex
SQLite telemetry logs as a lower-priority forensic source:

```bash
agent-recall index-logs
```

If your transcripts live outside the default roots, point the tool at them explicitly:

```bash
CODEX_HOME=/path/to/codex-home agent-recall index --since 30d
OMP_HOME=/path/to/omp-home agent-recall index --since 30d
PI_HOME=/path/to/pi-home agent-recall index --since 30d
CLAUDE_HOME=/path/to/claude-home agent-recall index --since 30d
agent-recall index --since 30d --source /path/to/exported/sessions
```

`index-logs` is intentionally separate from `index`; it is not used by the
watcher and does not run by default.

## Example Output

Search returns grouped receipts with exact source lines:

```text
$ agent-recall search "signing secret" --db /tmp/agent-recall-demo/index.sqlite
1. [codex] demo-session:84a7836c808a80c6  demo-session  /Users/me/projects/acme-api
   - [codex] assistant_message  /tmp/agent-recall-demo/sessions/2026/04/13/demo.jsonl:3
     The production signing secret was stale after the provider rotation.
```

Recent is useful when you know the repo or time window but not the query:

```text
$ agent-recall recent --repo acme-api --since 30d --db /tmp/agent-recall-demo/index.sqlite
1. [codex] demo-session:84a7836c808a80c6  demo-session  acme-api
   when: 2026-04-13T01:00:00Z
   cwd: /Users/me/projects/acme-api
   source: codex /tmp/agent-recall-demo/sessions/2026/04/13/demo.jsonl
   show: agent-recall show 'demo-session:84a7836c808a80c6' --limit 120
```

Doctor gives a fast health check for the index:

```json
{
  "ok": true,
  "checks": {
    "fts_integrity": "ok",
    "quick_check": "ok"
  },
  "stats": {
    "duplicate_source_files": 0,
    "events": 3,
    "sessions": 1,
    "source_files": 1
  },
  "freshness": "fresh"
}
```

Memories give agents durable objects with receipts instead of raw transcript blobs:

```json
{
  "object": "list",
  "type": "memory",
  "count": 1,
  "match_strategy": "all_terms",
  "results": [
    {
      "object": "memory",
      "id": "mem_decision_1d5e8b7c5bb0e851",
      "kind": "decision",
      "summary": "Keep the watcher LaunchAgent generic.",
      "evidence_count": 2,
      "resource_uri": "agent-recall://memory/mem_decision_1d5e8b7c5bb0e851"
    }
  ]
}
```

## Support Scope

- Works anywhere you have supported session JSONL archives on disk.
- Defaults to Codex, OMP, Pi, and Claude transcript roots under `~`.
- Can optionally import archived Codex `logs_*.sqlite` telemetry databases through `index-logs`.
- Honors `CODEX_HOME`, `OMP_HOME`, `PI_HOME`, and `CLAUDE_HOME` when data lives somewhere else.
- Stores index and pin data under XDG-style data/state paths when available, otherwise falls back to `~/.local/share` and `~/.local/state`.
- `watch --install-launch-agent` is macOS-only because it writes and manages a LaunchAgent plist.

## Privacy and Safety

- Transcript files stay local. `agent-recall` reads JSONL archives from disk and builds a local SQLite index.
- The SQLite index is disposable. You can delete it and rebuild from the raw transcript files.
- Pins are stored locally as JSON outside the SQLite index so they survive rebuilds.
- Secret redaction is best-effort. It catches common token patterns before indexing, but it is not a hard security boundary.
- If your transcripts contain data that should never be indexed, keep those files out of the configured source roots.

## Default Paths

Source roots:

- `$CODEX_HOME/sessions`
- `$CODEX_HOME/archived_sessions`
- `$OMP_HOME/agent/sessions`
- `$PI_HOME/agent/sessions`
- `$CLAUDE_HOME/projects`
- or, when those env vars are unset:
  - `~/.codex/sessions`
  - `~/.codex/archived_sessions`
  - `~/.omp/agent/sessions`
  - `~/.pi/agent/sessions`
  - `~/.claude/projects`


Optional archived Codex log sources for `index-logs`:

- `$CODEX_HOME/archived_logs/**/logs_*.sqlite`
- or, when `CODEX_HOME` is unset:
  - `~/.codex/archived_logs/**/logs_*.sqlite`

Index and state files:

- `$AGENT_RECALL_DB` overrides the SQLite path
- `$AGENT_RECALL_STATE` overrides the watch state path
- `$AGENT_RECALL_PINS` overrides the pins path
- otherwise:
  - `$XDG_DATA_HOME/agent-recall/index.sqlite`
  - `$XDG_DATA_HOME/agent-recall/pins.json`
  - `$XDG_STATE_HOME/agent-recall/watch.json`
- with fallback to:
  - `~/.local/share/agent-recall/index.sqlite`
  - `~/.local/share/agent-recall/pins.json`
  - `~/.local/state/agent-recall/watch.json`

## Commands

```bash
agent-recall index
agent-recall index --since 30d
agent-recall index --repo acme-api --since 7d
agent-recall index-logs
agent-recall index-logs --source ~/.codex/archived_logs/keep-codex-fast-20260502-183512/logs_2.sqlite
agent-recall rebuild
agent-recall watch
agent-recall watch --once
agent-recall watch --once --repo acme-api --since 7d --quiet-for 0
agent-recall watch --install-launch-agent --start-launch-agent
agent-recall status
agent-recall status --repo acme-api --since 7d --json
agent-recall status --json
agent-recall search "payment webhook"
agent-recall search "payment webhook" --repo acme-api --since 2026-04-01
agent-recall search "payment webhook" --from 2026-04-01 --until 2026-04-14
agent-recall search "payment webhook" --day 2026-04-13 --kind assistant --json
agent-recall search "payment webhook" --since 7d
agent-recall search "payment webhook" --cwd projects/acme-api
agent-recall search "payment webhook" --exclude-session <session-id-or-session-key>
agent-recall search "payment webhook" --exclude-current
agent-recall search "payment webhook" --trace --json
agent-recall search "payment webhook" --json
agent-recall recent --repo acme-api --since 7d
agent-recall recent --day 2026-04-13 --json
agent-recall day 2026-04-13 --json
agent-recall bundle "payment webhook" --repo acme-api --since 14d
agent-recall show <session-id-or-session-key> --json
agent-recall memories "launch agent" --kind decision --json
agent-recall memory-show <memory-id> --json
agent-recall delta --cursor <opaque-cursor> --json
agent-recall related <session-id-or-session-key> --json
agent-recall related <memory-id> --json
agent-recall eval evals/recall.json --json
agent-recall resources --kind memory --json
agent-recall read-resource agent-recall://memory/<memory-id>
agent-recall pin <session-key> --label "watcher design"
agent-recall pins --repo agent-recall
agent-recall pins --repo agent-recall --json
agent-recall unpin <session-key>
agent-recall doctor --json
agent-recall doctor --repo acme-api --since 7d --json
agent-recall stats
```

Useful flags:

```bash
agent-recall index --db /tmp/index.sqlite --source ~/.codex/sessions/2026/04
agent-recall index --since 30d
agent-recall index --repo agent-recall --since 2026-04-13
agent-recall index-logs --force
agent-recall index-logs --include-duplicates
agent-recall watch --interval 30 --quiet-for 5
agent-recall watch --once --repo agent-recall --since 2026-04-13 --quiet-for 0
agent-recall watch --install-launch-agent
agent-recall watch --install-launch-agent --start-launch-agent
agent-recall status --repo agent-recall --since 2026-04-13 --json
agent-recall doctor --repo agent-recall --since 2026-04-13 --json
agent-recall search "source-map" --limit 5
agent-recall search "source-map" --all-repos
agent-recall search "source-map" --include-duplicates
agent-recall search "source-map" --kind command
agent-recall recent --limit 10
agent-recall recent --all-repos
agent-recall recent --json
agent-recall memories --limit 10 --trace --json
agent-recall resources --limit 10 --json
agent-recall show <session-key> --limit 20
agent-recall pin <session-key> --label "canonical decision" --pins /tmp/pins.json
agent-recall unpin <session-key> --pins /tmp/pins.json
```

## Behavior

- Streams JSONL files and indexes high-signal user, assistant, shell command, and tool events.
- Extracts deterministic memory objects during indexing for `decision`, `task`, `fact`, `open_question`, and `blocker` cues.
- Consolidates repeated memory statements across sessions into stable `mem_<kind>_<hash>` ids with evidence receipts.
- Redacts common secret shapes before writing searchable text to SQLite.
- Skips Codex instruction preambles such as `AGENTS.md` and environment context blocks.
- Deduplicates exact duplicate transcript events.
- Keeps exact source provenance as `source_kind + path:line`.
- Stores a stable `session_key` derived from `session_id + source_file_path`.
- Deduplicates active/archive copies by `source_kind + session_id` in `search`, `recent`, and `bundle` by default, preferring active `sessions` files over `archived_sessions` files. Use `--include-duplicates` to inspect every indexed source copy.
- Uses SQLite FTS5 with safe query normalization, so punctuation-heavy queries like `source-map` work.
- Falls back to matching any query term when no single event contains every term.
- Supports search filters by repo slug, cwd substring, session start date, event kind, and explicit excluded sessions. Repo matching uses both the session cwd and command cwd values seen inside the session.
- Accepts absolute `--since` dates plus relative values like `7d`, `30d`, `today`, and `yesterday`.
- Accepts `--from` as an explicit lower bound and `--until` as an exclusive upper bound. Use `--from 2026-04-13 --until 2026-04-14` for the local calendar day of April 13.
- Accepts `--day YYYY-MM-DD` as shorthand for `--from YYYY-MM-DD --until <next-day>`.
- Rejects `--since` and `--from` together because both are lower bounds.
- Rejects `--day` when combined with `--since`, `--from`, or `--until`.
- Accepts repeatable `--kind user`, `--kind assistant`, `--kind command`, and `--kind tool` filters.
- Accepts `--exclude-current` when `CODEX_SESSION_ID` or `CODEX_THREAD_ID` is set.
- Interprets `today` and `yesterday` using the local day boundary, then compares against UTC transcript timestamps.
- Boosts results from the current git repo by default. Use `--repo` to filter to a repo, or `--all-repos` to disable the current-repo boost.
- Accepts `recent --all-repos` for command-shape parity with `search` and `bundle`; `recent` already spans all repos unless `--repo` is set.
- Tracks file size and mtime so repeat indexing skips unchanged sessions.
- Reports indexing progress to stderr with discovered file totals, bytes processed, elapsed time, ETA, current file, and skipped-file reason counts.
- `index` and `rebuild` accept `--repo` and `--since`, so first-time imports can skip old multi-GB archives.
- Batches multiple session files per SQLite transaction and keeps compatible older Codex index rows current, avoiding unnecessary full reindexing after metadata-only parser upgrades.
- Watches session roots with a polling freshness loop, waits for files to be quiet before indexing, and records watcher state in the configured state path.
- When settled backlog and live writes coexist, `watch` indexes the stable files first and leaves only the still-changing files pending.
- Supports bounded one-shot watcher catch-ups with `watch --once --repo <repo> --since <date-or-window>` so agents can refresh the relevant recent slice without walking old archive directories.
- `status` and `doctor` accept the same `--repo` and `--since` filters so freshness checks can match a bounded watch scope.
- Reports a blunt freshness verdict: `fresh`, `refreshing`, `stale`, `pending-live-writes`, `using-stale-index`, or `watcher-not-running`.
- Reports freshness status with pending file counts, stable/waiting file counts, last indexed time, last watcher error, refresh-lock state, and LaunchAgent installed/running state.
- Can write a macOS LaunchAgent plist for the watcher with `watch --install-launch-agent`.
- Can bootstrap and verify that LaunchAgent immediately with `watch --install-launch-agent --start-launch-agent`.
- Groups text search output by session, with the best receipts under each session.
- Exposes `search --trace --json` so agents can inspect match strategy, repo boost, per-session hit counts, and FTS scores.
- Exposes `search --trace --json` so agents can inspect the normalized query terms, concrete FTS query, fetch window, repo boost, duplicate identity, per-session hit counts, source priority, and FTS scores.
- Lists recent sessions without a query when you know the timeframe or repo but not the exact words to search.
- Prints machine-readable `recent --json`, `show --json`, and `day --json` output for automation.
- Prints machine-readable `memories`, `memory-show`, `delta`, `related`, `eval`, `resources`, and `read-resource` output for automation.
- Accepts fixture-driven `eval` cases for `search`, `memories`, and `delta`, so agent retrieval regressions can be checked in CI.
- Prints a day inventory with `day YYYY-MM-DD --json`, including session records plus repo and cwd counts.
- Formats search results into an agent-ready context bundle with top sessions, receipts, and follow-up `show` commands.
- Returns incremental session and memory feeds through `delta`, with append-only `chg_<id>` cursors for deterministic “what changed since I last looked?” polling.
- Expands related context from a session or memory reference using shared memory evidence instead of a second manual search.
- Lists and reads MCP-style `agent-recall://session/...` and `agent-recall://memory/...` resources so an external MCP server can wrap the CLI without redesigning its data model.
- Stores durable labeled pins outside the disposable SQLite index.
- Ranks sessions by current-repo match, hit count, event kind, FTS rank, and recency.
- Reports source-file counts and duplicate source-file counts in `stats`.
- Keeps `--json` output compact by returning `text_preview` instead of full transcript blobs.
- Separates progress and diagnostics onto stderr so `--json` output stays pipe-safe.
- Opens read-only commands without running schema migrations, so `search`, `recent`, `bundle`, `show`, `doctor`, and `stats` do not create missing databases or take writer locks.
- Uses SQLite WAL mode, a 30-second busy timeout, normal synchronous writes, and lock-aware watcher retry/backoff so read commands and refreshes can overlap without confusing stale index data for missing results.
- Serializes refresh writers with an app-level `index.sqlite.refresh.lock` file, so `watch`, `index`, and `rebuild` do not compete for the same SQLite writer lock. A foreground one-shot refresh waits briefly for the active refresh; if it cannot acquire the refresh lock, it keeps the current index and reports that another refresh is already active.
- `index-logs` imports archived Codex SQLite telemetry as `codex-log` sessions, skipping thread ids that still have matching Codex JSONL session files unless `--include-duplicates` is set.
- `index-logs` reports stderr progress by log database, scan phase, row counts, duplicate threads, parsed sessions, indexed sessions/events, elapsed time, ETA, and current file.
- `codex-log` results are ranked below normal transcript results and should be treated as forensic fallback receipts because the source rows are API/telemetry logs, not clean transcripts.

## Maintenance

Use `doctor` when the index feels stale or suspicious:

```bash
agent-recall doctor
agent-recall doctor --json
agent-recall doctor --repo agent-recall --since 7d --json
```

`doctor` is read-only when the database is missing. It reports the missing index instead of creating an empty one.

Use `rebuild` when the disposable SQLite index should be recreated from the raw JSONL source files:

```bash
agent-recall rebuild
```

Use `watch` when the index should stay fresh while agents write new transcripts:

```bash
agent-recall watch
agent-recall status
agent-recall status --repo agent-recall --since 7d --json
```

On macOS, `watch --install-launch-agent` writes a plist to `~/Library/LaunchAgents/dev.agent-recall.watch.plist` by default and prints the `launchctl bootstrap` command to start it.

Use `bundle` when an agent needs compact prior-session context:

```bash
agent-recall bundle "launch agent watcher" --since 14d --limit 5
agent-recall bundle "launch agent watcher" --from 2026-04-13 --until 2026-04-14 --limit 5
agent-recall bundle "launch agent watcher" --day 2026-04-13 --kind assistant --limit 5
```

Use `recent` when you do not know the right query yet:

```bash
agent-recall recent --repo agent-recall --since 7d --limit 10
agent-recall recent --repo agent-recall --from 2026-04-13 --until 2026-04-14 --limit 10
agent-recall recent --repo agent-recall --day 2026-04-13 --json
agent-recall day 2026-04-13 --json
```

Use `pin` after finding a high-value session that should be easy to return to:

```bash
agent-recall pin <session-key> --label "watcher freshness design"
agent-recall pins --repo agent-recall
agent-recall pins --repo agent-recall --json
agent-recall unpin <session-key>
```

## Agent Workflow

When an agent needs prior-session context:

1. Run `agent-recall status --json`.
2. If `freshness` is `fresh` or `pending-live-writes`, continue. `pending-live-writes` means the remaining backlog is only very recent files that are still settling.
3. If `freshness` is `refreshing`, keep using the current index unless the answer depends on very recent transcript files. The background watcher or another refresh already owns the refresh lock.
4. If a one-shot refresh reports `another refresh is already active`, keep using the current index unless the answer depends on very recent transcript files.
5. If `freshness` is `using-stale-index`, the last refresh could not take the SQLite writer lock after retry/backoff. Existing search results are usable, but a no-match result may mean the newest transcript files were not indexed yet. Retry `agent-recall watch --once --repo <repo> --since 7d --quiet-for 0` after the active refresh finishes when the current turn depends on recent sessions.
6. If `freshness` is `stale` and the LaunchAgent is running without `last_error`, prefer searching the current index or running a bounded catch-up such as `agent-recall watch --once --repo <repo> --since 7d --quiet-for 0`. Avoid unbounded foreground refreshes while the background watcher is already catching up.
7. If `freshness` is `stale` and the LaunchAgent is not running, run a bounded catch-up for the repo/timeframe you need. Use unbounded `agent-recall watch --once --quiet-for 0` or `agent-recall index` only when a full refresh is actually required.
8. If `freshness` is `watcher-not-running`, start the background watcher with `agent-recall watch --install-launch-agent --start-launch-agent`, then run `agent-recall watch --once --repo <repo> --since 7d --quiet-for 0` for an immediate targeted catch-up.
9. Use `agent-recall recent --repo <repo> --since 7d --limit 10` when you do not know the right search terms yet.
10. For calendar-day review, prefer `agent-recall day YYYY-MM-DD --json` or `--day YYYY-MM-DD` on `recent`, `search`, and `bundle`.
11. Use `agent-recall bundle "<query>" --repo <repo> --day YYYY-MM-DD --limit 5` for compact context.
12. Use `agent-recall search "<query>" --json --day YYYY-MM-DD --exclude-current` when programmatic filtering is needed during an automation.
13. Use `--kind user`, `--kind assistant`, or `--kind command` to narrow noisy searches.
14. Add `--exclude-session <session-id-or-session-key>` when the current automation or session id is known and `--exclude-current` is unavailable.
15. Keep the default deduped view unless the question is specifically about active/archive divergence. Use `--include-duplicates` only for that inspection.
16. Use `agent-recall show <session_key> --json` only for sessions that look relevant from `bundle`, `search`, `day`, or `recent`.
17. Use `agent-recall pin <session_key> --label "<why this matters>"` for canonical decisions or sessions that are likely to be reused.
18. Use `agent-recall pins --json` when scripts or agents need stable pin data.
19. Use `agent-recall unpin <session_key>` when a memory anchor is stale or mistaken.
20. Treat transcript evidence as historical. Verify against the current repo before acting.

## Verification Notes

In development, a full rebuild across a four-digit session-file archive completed in tens of minutes, and repeat indexing runs were much faster because unchanged files were skipped.

## Release Process

- CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` on every push to `main` and on pull requests.
- Release notes live in [CHANGELOG.md](CHANGELOG.md).

## Project Status

This is maintained as a personal tool that happens to be public. Bug reports are useful. I am not actively reviewing outside pull requests.
