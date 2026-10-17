mod cancel;
mod deliver;
mod event;
mod list;
mod runcmd;
mod schedule;
mod timeparse;

use anyhow::{Context, Result};
use clap::{ArgAction, Parser, Subcommand};
use event::FileEvent;
#[allow(unused)]
use jlogger_tracing::{jinfo, JloggerBuilder, LevelFilter};

/// CUI timer notifier — schedule a text message to a local user, delivered via write(1).
#[derive(Parser, Debug)]
#[command(name = "hark", version, about)]
struct Cli {
    /// Absolute date/time to send, e.g. "2026-06-22 15:30" or "2026-06-22 15:30:00".
    #[arg(long, value_name = "DATETIME", group = "trigger")]
    at: Option<String>,

    /// Relative delay from now, e.g. "3 minutes later", "90s", "2 hours". "0s" = now.
    #[arg(long = "in", value_name = "DURATION", group = "trigger")]
    relative: Option<String>,

    /// Notify when this process id exits.
    #[arg(long = "on-exit", value_name = "PID", group = "trigger")]
    on_exit: Option<i32>,

    /// Notify on a filesystem event for this path (see --event).
    #[arg(long = "on-file", value_name = "PATH", group = "trigger")]
    on_file: Option<String>,

    /// Which file event to wait for (with --on-file). Defaults to "modified".
    #[arg(long, value_enum, requires = "on_file")]
    event: Option<FileEvent>,

    /// The message text to deliver.
    #[arg(short, long)]
    message: Option<String>,

    /// Recipient username. Defaults to the current user.
    #[arg(short = 't', long = "to", value_name = "USER")]
    to: Option<String>,

    /// Increase verbosity
    #[arg(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a command and notify when it exits, including its success/failure.
    Run {
        /// Optional message; defaults to the command line. The outcome is appended.
        #[arg(short, long)]
        message: Option<String>,
        /// Recipient username. Defaults to the current user.
        #[arg(short = 't', long = "to", value_name = "USER")]
        to: Option<String>,
        /// The command and its arguments, e.g. `hark run -- bitbake image`.
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            required = true,
            value_name = "CMD"
        )]
        cmd: Vec<String>,
    },

    /// List notifications currently registered with systemd.
    List,

    /// Cancel (remove) one or more registered notifications.
    Cancel {
        /// Cancel all registered notifications.
        #[arg(long)]
        all: bool,
        /// Unit name(s) to cancel, as shown in the UNIT column of `hark list`.
        #[arg(value_name = "UNIT")]
        units: Vec<String>,
    },

    /// Internal: deliver a message right now (invoked by the systemd timer).
    #[command(hide = true)]
    Deliver {
        #[arg(long)]
        to: String,
        #[arg(short, long)]
        message: String,
    },

    /// Internal: block until a file event, then deliver (background watcher).
    #[command(hide = true)]
    WatchFile {
        #[arg(long)]
        path: String,
        #[arg(long, value_enum)]
        event: FileEvent,
        #[arg(long)]
        to: String,
        #[arg(short, long)]
        message: String,
    },

    /// Internal: block until a process exits, then deliver (background watcher).
    #[command(hide = true)]
    WatchPid {
        #[arg(long)]
        pid: i32,
        #[arg(long)]
        to: String,
        #[arg(short, long)]
        message: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.verbose {
        0 => {
            JloggerBuilder::new().max_level(LevelFilter::INFO).build();
        }
        1 => {
            JloggerBuilder::new().max_level(LevelFilter::DEBUG).build();
        }
        _ => {
            JloggerBuilder::new().max_level(LevelFilter::TRACE).build();
        }
    }

    // Subcommand roles.
    match &cli.command {
        Some(Command::Run { message, to, cmd }) => {
            let recipient = to.clone().unwrap_or_else(whoami::username);
            return runcmd::run(cmd, &recipient, message.as_deref());
        }
        Some(Command::List) => return list::list(),
        Some(Command::Cancel { all, units }) => return cancel::cancel(units, *all),
        Some(Command::Deliver { to, message }) => return deliver::deliver(to, message),
        Some(Command::WatchFile {
            path,
            event,
            to,
            message,
        }) => return event::watch_file(path, *event, to, message),
        Some(Command::WatchPid { pid, to, message }) => return event::watch_pid(*pid, to, message),
        None => {}
    }

    // Schedule role: user-facing. A message is required for all triggers here.
    let message = cli
        .message
        .as_deref()
        .context("a message is required (use -m/--message)")?;

    let recipient = cli.to.clone().unwrap_or_else(whoami::username);

    // Dispatch by trigger kind (the clap "trigger" group is mutually exclusive).
    if let Some(pid) = cli.on_exit {
        return event::register_pid_watch(pid, &recipient, message, cli.verbose);
    }
    if let Some(path) = &cli.on_file {
        let ev = cli.event.unwrap_or(FileEvent::Modified);
        return event::register_file_watch(path, ev, &recipient, message, cli.verbose);
    }

    // Otherwise it's a time-based trigger (default: now).
    let target = match (&cli.at, &cli.relative) {
        (Some(at), None) => timeparse::parse_absolute(at)
            .with_context(|| format!("could not parse date/time: {at:?}"))?,
        (None, Some(rel)) => timeparse::parse_relative(rel)
            .with_context(|| format!("could not parse relative time: {rel:?}"))?,
        (None, None) => chrono::Local::now(),
        (Some(_), Some(_)) => unreachable!("clap enforces trigger mutual exclusion"),
    };

    schedule::schedule(target, &recipient, message, cli.verbose)
}
