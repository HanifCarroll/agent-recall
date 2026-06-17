pub mod cli;
mod commands;
pub mod config;
pub mod indexer;
pub mod log_importer;
pub mod memory;
mod output;
pub mod parser;
pub mod redact;
mod refresh_lock;
pub mod store;

use anyhow::Result;

pub fn run() -> Result<()> {
    cli::run(std::env::args().skip(1))
}
