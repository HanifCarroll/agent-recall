use crate::commands::doctor::{run_doctor, run_stats, DoctorArgs, StatsArgs};
use crate::commands::index::{run_index, run_rebuild, IndexArgs, RebuildArgs};
use crate::commands::logs::{run_index_logs, IndexLogsArgs};
use crate::commands::memory::{
    run_delta, run_eval, run_memories, run_memory_show, run_read_resource, run_related,
    run_resources, DeltaArgs, EvalArgs, MemoriesArgs, MemoryShowArgs, ReadResourceArgs,
    RelatedArgs, ResourcesArgs,
};
use crate::commands::pins::{run_pin, run_pins, run_unpin, PinArgs, PinsArgs, UnpinArgs};
use crate::commands::recent::{run_day, run_recent, DayArgs, RecentArgs};
use crate::commands::search::{run_bundle, run_search, run_show, BundleArgs, SearchArgs, ShowArgs};
use crate::commands::watch::{run_status, run_watch, StatusArgs, WatchArgs};
use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Local search and recall for agent sessions, including archived Codex log archives"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Incrementally index agent session archives.
    Index(IndexArgs),
    /// Optionally import archived Codex SQLite telemetry logs.
    IndexLogs(IndexLogsArgs),
    /// Delete and rebuild the index from session archives.
    Rebuild(RebuildArgs),
    /// Continuously index stable pending session files.
    Watch(WatchArgs),
    /// Show watch/index freshness and pending-file status.
    Status(StatusArgs),
    /// Search indexed sessions and print grouped receipts.
    Search(SearchArgs),
    /// Show latest indexed sessions without a query.
    Recent(RecentArgs),
    /// Show an indexed session inventory for one local calendar day.
    Day(DayArgs),
    /// Print a compact Markdown bundle for a search query.
    Bundle(BundleArgs),
    /// Show indexed events for one session.
    Show(ShowArgs),
    /// List or query extracted memory objects.
    Memories(MemoriesArgs),
    /// Show one extracted memory object with evidence receipts.
    MemoryShow(MemoryShowArgs),
    /// List changed sessions and memories since an optional cursor.
    Delta(DeltaArgs),
    /// Expand related sessions and memories from a session or memory reference.
    Related(RelatedArgs),
    /// Run retrieval eval cases from a JSON fixture.
    Eval(EvalArgs),
    /// List MCP-style resources for sessions and memories.
    Resources(ResourcesArgs),
    /// Read a single MCP-style session or memory resource.
    ReadResource(ReadResourceArgs),
    /// Pin a high-value session anchor.
    Pin(PinArgs),
    /// List pinned session anchors.
    Pins(PinsArgs),
    /// Remove a pinned session anchor.
    Unpin(UnpinArgs),
    /// Check database health, FTS integrity, and source paths.
    Doctor(DoctorArgs),
    /// Print database counts.
    Stats(StatsArgs),
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<()> {
    let cli = Cli::parse_from(std::iter::once("agent-recall".to_owned()).chain(args));
    match cli.command {
        Some(Command::Index(args)) => run_index(args),
        Some(Command::IndexLogs(args)) => run_index_logs(args),
        Some(Command::Rebuild(args)) => run_rebuild(args),
        Some(Command::Watch(args)) => run_watch(args),
        Some(Command::Status(args)) => run_status(args),
        Some(Command::Search(args)) => run_search(args),
        Some(Command::Recent(args)) => run_recent(args),
        Some(Command::Day(args)) => run_day(args),
        Some(Command::Bundle(args)) => run_bundle(args),
        Some(Command::Show(args)) => run_show(args),
        Some(Command::Memories(args)) => run_memories(args),
        Some(Command::MemoryShow(args)) => run_memory_show(args),
        Some(Command::Delta(args)) => run_delta(args),
        Some(Command::Related(args)) => run_related(args),
        Some(Command::Eval(args)) => run_eval(args),
        Some(Command::Resources(args)) => run_resources(args),
        Some(Command::ReadResource(args)) => run_read_resource(args),
        Some(Command::Pin(args)) => run_pin(args),
        Some(Command::Pins(args)) => run_pins(args),
        Some(Command::Unpin(args)) => run_unpin(args),
        Some(Command::Doctor(args)) => run_doctor(args),
        Some(Command::Stats(args)) => run_stats(args),
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}
