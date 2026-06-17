use crate::config::{default_db_path, default_pins_path};
use crate::output::{now_timestamp, shell_quote};
use crate::store::{SessionMatch, Store};
use anyhow::{bail, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Args)]
pub struct PinArgs {
    #[arg(help = "Session id or session key to pin")]
    pub session_ref: String,
    #[arg(long, help = "Human-readable pin label")]
    pub label: String,
    #[arg(long, help = "SQLite index path")]
    pub db: Option<PathBuf>,
    #[arg(long, help = "Pins JSON path")]
    pub pins: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct PinsArgs {
    #[arg(long, help = "Pins JSON path")]
    pub pins: Option<PathBuf>,
    #[arg(long, default_value_t = 50, help = "Maximum pins to print")]
    pub limit: usize,
    #[arg(long, help = "Restrict pins to a repo name")]
    pub repo: Option<String>,
    #[arg(long, help = "Restrict pins to a cwd substring")]
    pub cwd: Option<String>,
    #[arg(long, help = "Emit machine-readable JSON")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct UnpinArgs {
    #[arg(help = "Session id or session key to unpin")]
    pub session_ref: String,
    #[arg(long, help = "Pins JSON path")]
    pub pins: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PinsFile {
    version: u32,
    pins: Vec<PinRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PinRecord {
    session_key: String,
    session_id: String,
    label: String,
    repo: String,
    #[serde(default)]
    repos: Vec<String>,
    cwd: String,
    source_file_path: PathBuf,
    created_at: String,
    updated_at: String,
}

pub fn run_pin(args: PinArgs) -> Result<()> {
    let db_path = args.db.unwrap_or(default_db_path()?);
    let pins_path = args.pins.unwrap_or(default_pins_path()?);
    let store = Store::open_readonly(&db_path)?;
    let matches = store.resolve_session_reference(&args.session_ref)?;
    let session = resolve_single_session(&args.session_ref, &matches)?;
    let repos = store.session_repos(&session.session_key)?;
    let mut pins_file = read_pins_file(&pins_path)?;
    let now = now_timestamp();

    if let Some(existing) = pins_file
        .pins
        .iter_mut()
        .find(|pin| pin.session_key == session.session_key)
    {
        existing.session_id = session.session_id.clone();
        existing.label = args.label;
        existing.repo = session.repo.clone();
        existing.repos = repos;
        existing.cwd = session.cwd.clone();
        existing.source_file_path = session.source_file_path.clone();
        existing.updated_at = now;
    } else {
        pins_file.pins.push(PinRecord {
            session_key: session.session_key.clone(),
            session_id: session.session_id.clone(),
            label: args.label,
            repo: session.repo.clone(),
            repos,
            cwd: session.cwd.clone(),
            source_file_path: session.source_file_path.clone(),
            created_at: now.clone(),
            updated_at: now,
        });
    }

    write_pins_file(&pins_path, &pins_file)?;
    println!("pinned {}  {}", session.session_id, session.session_key);
    Ok(())
}

pub fn run_pins(args: PinsArgs) -> Result<()> {
    let pins_path = args.pins.unwrap_or(default_pins_path()?);
    let mut pins = read_pins_file(&pins_path)?.pins;
    pins.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| left.session_key.cmp(&right.session_key))
    });
    pins.retain(|pin| pin_matches_filters(pin, args.repo.as_deref(), args.cwd.as_deref()));
    pins.truncate(args.limit.clamp(1, 100));

    if args.json {
        print_pins_json(&pins)?;
        return Ok(());
    }

    if pins.is_empty() {
        println!("no pins");
        return Ok(());
    }

    print_pins(&pins);
    Ok(())
}

pub fn run_unpin(args: UnpinArgs) -> Result<()> {
    let pins_path = args.pins.unwrap_or(default_pins_path()?);
    let mut pins_file = read_pins_file(&pins_path)?;
    let Some(index) = pins_file
        .pins
        .iter()
        .position(|pin| pin.session_key == args.session_ref || pin.session_id == args.session_ref)
    else {
        bail!("no pin matches `{}`", args.session_ref);
    };
    let removed = pins_file.pins.remove(index);
    write_pins_file(&pins_path, &pins_file)?;
    println!("unpinned {}  {}", removed.session_id, removed.session_key);
    Ok(())
}

fn resolve_single_session<'a>(
    session_ref: &str,
    matches: &'a [SessionMatch],
) -> Result<&'a SessionMatch> {
    if matches.is_empty() {
        bail!("no indexed session matches `{session_ref}`");
    }
    if matches.len() > 1 {
        let choices = matches
            .iter()
            .map(|session| {
                format!(
                    "  {}  {}  {}",
                    session.session_key,
                    session.cwd,
                    session.source_file_path.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!("multiple indexed sessions match `{session_ref}`; use one session_key:\n{choices}");
    }
    Ok(&matches[0])
}

fn read_pins_file(path: &Path) -> Result<PinsFile> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PinsFile {
                version: 1,
                pins: Vec::new(),
            });
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn write_pins_file(path: &Path, pins_file: &PinsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(pins_file)?;
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn pin_matches_filters(pin: &PinRecord, repo: Option<&str>, cwd: Option<&str>) -> bool {
    if let Some(repo) = repo {
        if !pin.repo.eq_ignore_ascii_case(repo)
            && !pin
                .repos
                .iter()
                .any(|membership| membership.eq_ignore_ascii_case(repo))
        {
            return false;
        }
    }
    if let Some(cwd) = cwd {
        if !pin.cwd.contains(cwd) {
            return false;
        }
    }
    true
}

fn print_pins(pins: &[PinRecord]) {
    for (index, pin) in pins.iter().enumerate() {
        println!(
            "{}. {}  {}  {}",
            index + 1,
            pin.label,
            pin.session_id,
            pin.repo
        );
        if !pin.repos.is_empty() {
            println!("   repos: {}", pin.repos.join(", "));
        }
        println!("   session_key: {}", pin.session_key);
        println!("   pinned_at: {}", pin.created_at);
        println!("   updated_at: {}", pin.updated_at);
        println!("   cwd: {}", pin.cwd);
        println!("   source: {}", pin.source_file_path.display());
        println!(
            "   show: agent-recall show {} --limit 120",
            shell_quote(&pin.session_key)
        );
    }
}

fn print_pins_json(pins: &[PinRecord]) -> Result<()> {
    let values = pins
        .iter()
        .map(|pin| {
            json!({
                "session_key": pin.session_key,
                "session_id": pin.session_id,
                "label": pin.label,
                "repo": pin.repo,
                "repos": pin.repos,
                "cwd": pin.cwd,
                "source_file_path": pin.source_file_path,
                "created_at": pin.created_at,
                "updated_at": pin.updated_at,
                "show_command": format!(
                    "agent-recall show {} --limit 120",
                    shell_quote(&pin.session_key)
                ),
            })
        })
        .collect::<Vec<_>>();
    let value = json!({
        "count": values.len(),
        "pins": values,
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
