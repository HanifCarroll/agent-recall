# codex-recall

[![CI](https://github.com/HanifCarroll/codex-recall/actions/workflows/ci.yml/badge.svg)](https://github.com/HanifCarroll/codex-recall/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codex-recall.svg)](https://crates.io/crates/codex-recall)

Local search and recall for Codex session JSONL archives.

`codex-recall` builds a disposable SQLite FTS5 index over transcript archives so you can search, inspect, and reuse prior session context without treating raw JSONL logs as a database.

Raw JSONL files remain the source of truth.

## Install

```bash
cargo install codex-recall
```

Or install directly from GitHub:

```bash
cargo install --git https://github.com/HanifCarroll/codex-recall
```

Build from source:

```bash
cargo install --path .
```

## Quick Start

Index your local Codex archives, then query them:

```bash
codex-recall index
codex-recall search "payment webhook"
codex-recall memories "launch agent"
codex-recall delta --json
codex-recall recent --since 7d
codex-recall doctor --json
```

If your transcripts live outside `~/.codex`, point the tool at them explicitly:

```bash
CODEX_HOME=/path/to/codex-home codex-recall index
codex-recall index --source /path/to/exported/sessions
```

## Example Output

Search returns grouped receipts with exact source lines:

```text
$ codex-recall search "signing secret" --db /tmp/codex-recall-demo/index.sqlite
1. demo-session:84a7836c808a80c6  demo-session  /Users/me/projects/acme-api
   - assistant_message  /tmp/codex-recall-demo/sessions/2026/04/13/demo.jsonl:3
     The production signing secret was stale after the provider rotation.
```

Recent is useful when you know the repo or time window but not the query:

```text
$ codex-recall recent --repo acme-api --since 30d --db /tmp/codex-recall-demo/index.sqlite
1. demo-session:84a7836c808a80c6  demo-session  acme-api
   when: 2026-04-13T01:00:00Z
   cwd: /Users/me/projects/acme-api
   source: /tmp/codex-recall-demo/sessions/2026/04/13/demo.jsonl
   show: codex-recall show 'demo-session:84a7836c808a80c6' --limit 120
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
      "resource_uri": "codex-recall://memory/mem_decision_1d5e8b7c5bb0e851"
    }
  ]
}
```

## Support Scope

- Works anywhere you have Codex-style session JSONL archives on disk.
- Defaults to `~/.codex/sessions` and `~/.codex/archived_sessions`.
- Honors `CODEX_HOME` when Codex data lives somewhere else.
- Stores index and pin data under XDG-style data/state paths when available, otherwise falls back to `~/.local/share` and `~/.local/state`.
- `watch --install-launch-agent` is macOS-only because it writes and manages a LaunchAgent plist.

## Privacy and Safety

- Transcript files stay local. `codex-recall` reads JSONL archives from disk and builds a local SQLite index.
- The SQLite index is disposable. You can delete it and rebuild from the raw transcript files.
- Pins are stored locally as JSON outside the SQLite index so they survive rebuilds.
- Secret redaction is best-effort. It catches common token patterns before indexing, but it is not a hard security boundary.
- If your transcripts contain data that should never be indexed, keep those files out of the configured source roots.

## Default Paths

Source roots:

- `$CODEX_HOME/sessions`
- `$CODEX_HOME/archived_sessions`
- or, when `CODEX_HOME` is unset:
  - `~/.codex/sessions`
  - `~/.codex/archived_sessions`

Index and state files:

- `$CODEX_RECALL_DB` overrides the SQLite path
- `$CODEX_RECALL_STATE` overrides the watch state path
- `$CODEX_RECALL_PINS` overrides the pins path
- otherwise:
  - `$XDG_DATA_HOME/codex-recall/index.sqlite`
  - `$XDG_DATA_HOME/codex-recall/pins.json`
  - `$XDG_STATE_HOME/codex-recall/watch.json`
- with fallback to:
  - `~/.local/share/codex-recall/index.sqlite`
  - `~/.local/share/codex-recall/pins.json`
  - `~/.local/state/codex-recall/watch.json`

## Commands

```bash
codex-recall index
codex-recall rebuild
codex-recall watch
codex-recall watch --once
codex-recall watch --once --repo acme-api --since 7d --quiet-for 0
codex-recall watch --install-launch-agent --start-launch-agent
codex-recall status
codex-recall status --repo acme-api --since 7d --json
codex-recall status --json
codex-recall search "payment webhook"
codex-recall search "payment webhook" --repo acme-api --since 2026-04-01
codex-recall search "payment webhook" --from 2026-04-01 --until 2026-04-14
codex-recall search "payment webhook" --day 2026-04-13 --kind assistant --json
codex-recall search "payment webhook" --since 7d
codex-recall search "payment webhook" --cwd projects/acme-api
codex-recall search "payment webhook" --exclude-session <session-id-or-session-key>
codex-recall search "payment webhook" --exclude-current
codex-recall search "payment webhook" --trace --json
codex-recall search "payment webhook" --json
codex-recall recent --repo acme-api --since 7d
codex-recall recent --day 2026-04-13 --json
codex-recall day 2026-04-13 --json
codex-recall bundle "payment webhook" --repo acme-api --since 14d
codex-recall show <session-id-or-session-key> --json
codex-recall memories "launch agent" --kind decision --json
codex-recall memory-show <memory-id> --json
codex-recall delta --cursor <opaque-cursor> --json
codex-recall related <session-id-or-session-key> --json
codex-recall related <memory-id> --json
codex-recall eval evals/recall.json --json
codex-recall resources --kind memory --json
codex-recall read-resource codex-recall://memory/<memory-id>
codex-recall pin <session-key> --label "watcher design"
codex-recall pins --repo codex-recall
codex-recall pins --repo codex-recall --json
codex-recall unpin <session-key>
codex-recall doctor --json
codex-recall doctor --repo acme-api --since 7d --json
codex-recall stats
```

Useful flags:

```bash
codex-recall index --db /tmp/index.sqlite --source ~/.codex/sessions/2026/04
codex-recall watch --interval 30 --quiet-for 5
codex-recall watch --once --repo codex-recall --since 2026-04-13 --quiet-for 0
codex-recall watch --install-launch-agent
codex-recall watch --install-launch-agent --start-launch-agent
codex-recall status --repo codex-recall --since 2026-04-13 --json
codex-recall doctor --repo codex-recall --since 2026-04-13 --json
codex-recall search "source-map" --limit 5
codex-recall search "source-map" --all-repos
codex-recall search "source-map" --include-duplicates
codex-recall search "source-map" --kind command
codex-recall recent --limit 10
codex-recall recent --all-repos
codex-recall recent --json
codex-recall memories --limit 10 --trace --json
codex-recall resources --limit 10 --json
codex-recall show <session-key> --limit 20
codex-recall pin <session-key> --label "canonical decision" --pins /tmp/pins.json
codex-recall unpin <session-key> --pins /tmp/pins.json
```

## Behavior

- Streams JSONL files and indexes high-signal user, assistant, and command events.
- Extracts deterministic memory objects during indexing for `decision`, `task`, `fact`, `open_question`, and `blocker` cues.
- Consolidates repeated memory statements across sessions into stable `mem_<kind>_<hash>` ids with evidence receipts.
- Redacts common secret shapes before writing searchable text to SQLite.
- Skips Codex instruction preambles such as `AGENTS.md` and environment context blocks.
- Deduplicates exact duplicate transcript events.
- Keeps exact source provenance as `path:line`.
- Stores a stable `session_key` derived from `session_id + source_file_path`.
- Deduplicates active/archive copies by `session_id` in `search`, `recent`, and `bundle` by default, preferring active `sessions` files over `archived_sessions` files. Use `--include-duplicates` to inspect every indexed source copy.
- Uses SQLite FTS5 with safe query normalization, so punctuation-heavy queries like `source-map` work.
- Falls back to matching any query term when no single event contains every term.
- Supports search filters by repo slug, cwd substring, session start date, event kind, and explicit excluded sessions. Repo matching uses both the session cwd and command cwd values seen inside the session.
- Accepts absolute `--since` dates plus relative values like `7d`, `30d`, `today`, and `yesterday`.
- Accepts `--from` as an explicit lower bound and `--until` as an exclusive upper bound. Use `--from 2026-04-13 --until 2026-04-14` for the local calendar day of April 13.
- Accepts `--day YYYY-MM-DD` as shorthand for `--from YYYY-MM-DD --until <next-day>`.
- Rejects `--since` and `--from` together because both are lower bounds.
- Rejects `--day` when combined with `--since`, `--from`, or `--until`.
- Accepts repeatable `--kind user`, `--kind assistant`, and `--kind command` filters.
- Accepts `--exclude-current` when `CODEX_SESSION_ID` or `CODEX_THREAD_ID` is set.
- Interprets `today` and `yesterday` using the local day boundary, then compares against UTC transcript timestamps.
- Boosts results from the current git repo by default. Use `--repo` to filter to a repo, or `--all-repos` to disable the current-repo boost.
- Accepts `recent --all-repos` for command-shape parity with `search` and `bundle`; `recent` already spans all repos unless `--repo` is set.
- Tracks file size and mtime so repeat indexing skips unchanged sessions.
- Reports indexing progress to stderr with discovered file totals, bytes processed, elapsed time, ETA, current file, and skipped-file reason counts.
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
- Lists and reads MCP-style `codex-recall://session/...` and `codex-recall://memory/...` resources so an external MCP server can wrap the CLI without redesigning its data model.
- Stores durable labeled pins outside the disposable SQLite index.
- Ranks sessions by current-repo match, hit count, event kind, FTS rank, and recency.
- Reports source-file counts and duplicate source-file counts in `stats`.
- Keeps `--json` output compact by returning `text_preview` instead of full transcript blobs.
- Separates progress and diagnostics onto stderr so `--json` output stays pipe-safe.
- Opens read-only commands without running schema migrations, so `search`, `recent`, `bundle`, `show`, `doctor`, and `stats` do not create missing databases or take writer locks.
- Uses SQLite WAL mode, a 30-second busy timeout, normal synchronous writes, and lock-aware watcher retry/backoff so read commands and refreshes can overlap without confusing stale index data for missing results.
- Serializes refresh writers with an app-level `index.sqlite.refresh.lock` file, so `watch`, `index`, and `rebuild` do not compete for the same SQLite writer lock. A foreground one-shot refresh waits briefly for the active refresh; if it cannot acquire the refresh lock, it keeps the current index and reports that another refresh is already active.

## Maintenance

Use `doctor` when the index feels stale or suspicious:

```bash
codex-recall doctor
codex-recall doctor --json
codex-recall doctor --repo codex-recall --since 7d --json
```

`doctor` is read-only when the database is missing. It reports the missing index instead of creating an empty one.

Use `rebuild` when the disposable SQLite index should be recreated from the raw JSONL source files:

```bash
codex-recall rebuild
```

Use `watch` when the index should stay fresh while Codex writes new transcripts:

```bash
codex-recall watch
codex-recall status
codex-recall status --repo codex-recall --since 7d --json
```

On macOS, `watch --install-launch-agent` writes a plist to `~/Library/LaunchAgents/dev.codex-recall.watch.plist` by default and prints the `launchctl bootstrap` command to start it.

Use `bundle` when an agent needs compact prior-session context:

```bash
codex-recall bundle "launch agent watcher" --since 14d --limit 5
codex-recall bundle "launch agent watcher" --from 2026-04-13 --until 2026-04-14 --limit 5
codex-recall bundle "launch agent watcher" --day 2026-04-13 --kind assistant --limit 5
```

Use `recent` when you do not know the right query yet:

```bash
codex-recall recent --repo codex-recall --since 7d --limit 10
codex-recall recent --repo codex-recall --from 2026-04-13 --until 2026-04-14 --limit 10
codex-recall recent --repo codex-recall --day 2026-04-13 --json
codex-recall day 2026-04-13 --json
```

Use `pin` after finding a high-value session that should be easy to return to:

```bash
codex-recall pin <session-key> --label "watcher freshness design"
codex-recall pins --repo codex-recall
codex-recall pins --repo codex-recall --json
codex-recall unpin <session-key>
```

## Agent Workflow

When an agent needs prior-session context:

1. Run `codex-recall status --json`.
2. If `freshness` is `fresh` or `pending-live-writes`, continue. `pending-live-writes` means the remaining backlog is only very recent files that are still settling.
3. If `freshness` is `refreshing`, keep using the current index unless the answer depends on very recent transcript files. The background watcher or another refresh already owns the refresh lock.
4. If a one-shot refresh reports `another refresh is already active`, keep using the current index unless the answer depends on very recent transcript files.
5. If `freshness` is `using-stale-index`, the last refresh could not take the SQLite writer lock after retry/backoff. Existing search results are usable, but a no-match result may mean the newest transcript files were not indexed yet. Retry `codex-recall watch --once --repo <repo> --since 7d --quiet-for 0` after the active refresh finishes when the current turn depends on recent sessions.
6. If `freshness` is `stale` and the LaunchAgent is running without `last_error`, prefer searching the current index or running a bounded catch-up such as `codex-recall watch --once --repo <repo> --since 7d --quiet-for 0`. Avoid unbounded foreground refreshes while the background watcher is already catching up.
7. If `freshness` is `stale` and the LaunchAgent is not running, run a bounded catch-up for the repo/timeframe you need. Use unbounded `codex-recall watch --once --quiet-for 0` or `codex-recall index` only when a full refresh is actually required.
8. If `freshness` is `watcher-not-running`, start the background watcher with `codex-recall watch --install-launch-agent --start-launch-agent`, then run `codex-recall watch --once --repo <repo> --since 7d --quiet-for 0` for an immediate targeted catch-up.
9. Use `codex-recall recent --repo <repo> --since 7d --limit 10` when you do not know the right search terms yet.
10. For calendar-day review, prefer `codex-recall day YYYY-MM-DD --json` or `--day YYYY-MM-DD` on `recent`, `search`, and `bundle`.
11. Use `codex-recall bundle "<query>" --repo <repo> --day YYYY-MM-DD --limit 5` for compact context.
12. Use `codex-recall search "<query>" --json --day YYYY-MM-DD --exclude-current` when programmatic filtering is needed during an automation.
13. Use `--kind user`, `--kind assistant`, or `--kind command` to narrow noisy searches.
14. Add `--exclude-session <session-id-or-session-key>` when the current automation or session id is known and `--exclude-current` is unavailable.
15. Keep the default deduped view unless the question is specifically about active/archive divergence. Use `--include-duplicates` only for that inspection.
16. Use `codex-recall show <session_key> --json` only for sessions that look relevant from `bundle`, `search`, `day`, or `recent`.
17. Use `codex-recall pin <session_key> --label "<why this matters>"` for canonical decisions or sessions that are likely to be reused.
18. Use `codex-recall pins --json` when scripts or agents need stable pin data.
19. Use `codex-recall unpin <session_key>` when a memory anchor is stale or mistaken.
20. Treat transcript evidence as historical. Verify against the current repo before acting.

## Verification Notes

In development, a full rebuild across a four-digit session-file archive completed in tens of minutes, and repeat indexing runs were much faster because unchanged files were skipped.

## Release Process

- CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` on every push to `main` and on pull requests.
- Release notes live in [CHANGELOG.md](CHANGELOG.md).

## Project Status

This is maintained as a personal tool that happens to be public. Bug reports are useful. I am not actively reviewing outside pull requests.
