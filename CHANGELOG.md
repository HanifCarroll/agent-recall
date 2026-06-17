# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Added OMP, Pi, and Claude JSONL transcript ingestion with explicit `source_kind`/`source_label` fields in search, recent, show, bundle, and related-session JSON output.
- Added `tool` event indexing for non-shell tool calls/results, alongside existing user, assistant, and shell command events.
- Added `--repo` and `--since` filters to `index` and `rebuild` so large multi-source imports can be bounded without using the watcher path.
- Added `index-logs` for optional lower-priority `codex-log` imports from archived Codex `logs_*.sqlite` telemetry databases, skipping threads that still have matching Codex JSONL transcripts by default.
- Added row-level stderr progress for `index-logs`, including scan/index phases, ETA, duplicate-thread counts, and indexed session/event totals.

### Changed

- Renamed the project from `codex-recall` to `agent-recall` to match its multi-agent (Codex, OMP, Pi, Claude) transcript support. Existing installs need a one-time migration:
  - The crate and binary are now `agent-recall` (`cargo install agent-recall`); remove the old `codex-recall` binary.
  - Environment overrides moved from `CODEX_RECALL_DB`/`CODEX_RECALL_STATE`/`CODEX_RECALL_PINS` to `AGENT_RECALL_DB`/`AGENT_RECALL_STATE`/`AGENT_RECALL_PINS`.
  - Default data and state files moved from `codex-recall/` to `agent-recall/` under the XDG data/state directories; copy `index.sqlite`, `pins.json`, and `watch.json` over or run `agent-recall rebuild`.
  - MCP-style resource URIs changed from `codex-recall://` to `agent-recall://`.
  - The macOS LaunchAgent label default is now `dev.agent-recall.watch`; `watch --install-launch-agent` retires the old `dev.codex-recall.watch` and `com.hanif.codex-recall.watch` agents automatically.

### Fixed

- Added `watch --once --repo <repo> --since <date-or-window>` for bounded one-shot catch-ups, including date-directory pruning so agents can avoid walking large old archives when only recent repo context matters.
- Added watcher retry/backoff for transient SQLite lock failures and a `using-stale-index` freshness state so agents can tell a refresh failure from an empty result set.
- Fixed `watch` so a few live writes no longer block indexing an older stable backlog, clarified mixed backlog freshness messages, and added matching `--repo`/`--since` filters to `status` and `doctor`.
- Fixed metadata-only parser upgrades so compatible existing Codex index rows stay current instead of forcing a multi-GB reindex.
- Improved bulk indexing throughput by batching multiple session files per SQLite transaction and tuning SQLite cache/WAL pragmas.
- Reduced OMP/Pi/Claude parse cost by deserializing only indexed message fields and skipping bulky hidden thinking/signature fields.
- Made transcript parsing tolerant of malformed JSONL records so one bad log line does not abort a full indexing pass.
- Pruned ISO-prefixed OMP/Pi session filenames for `--since` filters before parsing, avoiding malformed or old files outside the requested date window.
- Accepted `recent --all-repos` as a compatibility alias so agent-generated commands stop failing, and clarified in the docs that `recent` is already cross-repo unless `--repo` is set.

## [0.1.3] - 2026-04-15

### Added

- Added deterministic memory extraction during indexing, with stable memory ids plus evidence receipts for decisions, tasks, facts, blockers, and open questions.
- Added `memories`, `memory-show`, `delta`, `related`, `eval`, `resources`, and `read-resource` commands for agent-facing memory retrieval and MCP-style resource access.
- Added append-only `chg_<id>` delta cursors so incremental polling is deterministic and independent of timestamp ordering.
- Expanded `search --trace --json` with normalized query terms, concrete FTS queries, source priority, duplicate identity, and fetch-window details.
- Expanded the fixture-driven eval harness so `search`, `memories`, and `delta` retrieval regressions can be asserted in CI.

## [0.1.2] - 2026-04-15

### Added

- Published `codex-recall` to crates.io and documented the registry install path in the README.
- Added a crates.io badge so the canonical package version is visible from the repo homepage.

## [0.1.1] - 2026-04-15

### Fixed

- Made the LaunchAgent CLI tests platform-aware so GitHub Actions passes on Linux runners while still exercising the full install path on macOS.
- Bumped `actions/checkout` to a Node 24 compatible major in the CI workflow to avoid the GitHub-hosted runner deprecation warning.

## [0.1.0] - 2026-04-15

### Added

- Initial public release of `codex-recall`.
- Full-text search, recent-session listing, day views, bundles, pins, and freshness diagnostics for Codex transcript archives.
- macOS watcher support with LaunchAgent install and bootstrap helpers.
- GitHub Actions CI for formatting, clippy, and tests.
- Public README improvements with support scope, quick-start examples, and privacy guidance.

### Changed

- Switched default local data paths to honor `XDG_DATA_HOME` and `XDG_STATE_HOME` when available.
- Switched transcript source discovery to honor `CODEX_HOME` when set.
- Replaced the personal LaunchAgent label default with the generic `dev.codex-recall.watch`.
- Scrubbed personal repo and vault names from public-facing docs and test fixtures.
