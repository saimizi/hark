mod deliver;
mod schedule;
mod timeparse;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

/// CUI timer notifier — schedule a text message to a local user, delivered via write(1).
#[derive(Parser, Debug)]
#[command(name = "hark", version, about)]
struct Cli {
    /// Absolute date/time to send, e.g. "2026-06-22 15:30" or "2026-06-22 15:30:00".
    #[arg(long, value_name = "DATETIME", conflicts_with = "relative")]
    at: Option<String>,

    /// Relative delay from now, e.g. "3 minutes later", "90s", "2 hours".
    #[arg(long = "in", value_name = "DURATION", conflicts_with = "at")]
    relative: Option<String>,

    /// The message text to deliver.
    #[arg(short, long)]
    message: Option<String>,

    /// Recipient username. Defaults to the current user.
    #[arg(short = 't', long = "to", value_name = "USER")]
    to: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Internal: deliver a message right now (invoked by the systemd timer).
    #[command(hide = true)]
    Deliver {
        /// Recipient username.
        #[arg(long)]
        to: String,
        /// The message text.
        #[arg(short, long)]
        message: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Deliver role: invoked by the timer when it fires.
    if let Some(Command::Deliver { to, message }) = &cli.command {
        return deliver::deliver(to, message);
    }

    // Schedule role: user-facing.
    let message = cli
        .message
        .as_deref()
        .context("a message is required (use -m/--message)")?;

    let recipient = match cli.to {
        Some(ref u) => u.clone(),
        None => whoami::username(),
    };

    let target = match (&cli.at, &cli.relative) {
        (Some(at), None) => timeparse::parse_absolute(at)
            .with_context(|| format!("could not parse date/time: {at:?}"))?,
        (None, Some(rel)) => timeparse::parse_relative(rel)
            .with_context(|| format!("could not parse relative time: {rel:?}"))?,
        (None, None) => bail!("specify when to send with --at <DATETIME> or --in <DURATION>"),
        (Some(_), Some(_)) => unreachable!("--at and --in are mutually exclusive"),
    };

    schedule::schedule(target, &recipient, message)
}
