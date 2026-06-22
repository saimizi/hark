//! Event-based triggers: deliver a notification when a filesystem event occurs
//! or when a given process exits, instead of at a fixed time.
//!
//! The user-facing `register_*` functions launch a background systemd `--user`
//! transient *service* (via `systemd-run`, no `--wait`) that runs this binary
//! in a hidden watch role and returns immediately. The `watch_*` functions are
//! that watch role: they block until the event, then deliver and exit.

use anyhow::{bail, Context, Result};
use chrono::Local;
use clap::ValueEnum;
use inotify::{Inotify, WatchMask};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Which filesystem event to wait for on a watched path.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum FileEvent {
    /// The path is created (or moved into place).
    Created,
    /// The path's contents are modified.
    Modified,
    /// The path is removed (or moved away).
    Removed,
}

impl FileEvent {
    /// The token passed to the hidden `watch-file --event` subcommand.
    fn as_arg(self) -> &'static str {
        match self {
            FileEvent::Created => "created",
            FileEvent::Modified => "modified",
            FileEvent::Removed => "removed",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            FileEvent::Created => "is created",
            FileEvent::Modified => "is modified",
            FileEvent::Removed => "is removed",
        }
    }
}

/// Build a unique transient unit name for a watcher.
fn watch_unit() -> String {
    format!(
        "hark-watch-{}-{}",
        Local::now().format("%Y%m%d%H%M%S"),
        std::process::id()
    )
}

/// Start `systemd-run --user <unit> -- <self> <args...>` as a background service.
fn spawn_watcher(unit: &str, description: &str, args: &[&str]) -> Result<()> {
    let exe = std::env::current_exe().context("could not determine own executable path")?;
    let status = Command::new("systemd-run")
        .arg("--user")
        .arg(format!("--unit={unit}"))
        .arg(format!("--description={description}"))
        .arg(&exe)
        .args(args)
        .status()
        .context("failed to launch systemd-run (is systemd --user available?)")?;
    if !status.success() {
        bail!("systemd-run exited with {status}");
    }
    Ok(())
}

/// Register a background watcher that notifies when `path` sees `event`.
pub fn register_file_watch(
    path: &str,
    event: FileEvent,
    recipient: &str,
    message: &str,
) -> Result<()> {
    let unit = watch_unit();
    // Structured description so `hark list` can recover the details. `path` is
    // placed last because it may contain spaces.
    let description = format!(
        "hark-job kind=file event={} to={recipient} path={path}",
        event.as_arg()
    );
    spawn_watcher(
        &unit,
        &description,
        &[
            "watch-file",
            "--path",
            path,
            "--event",
            event.as_arg(),
            "--to",
            recipient,
            "--message",
            message,
        ],
    )?;
    println!(
        "Watching '{path}' until it {}; will notify '{recipient}'. unit {unit}",
        event.describe()
    );
    Ok(())
}

/// Register a background watcher that notifies when process `pid` exits.
pub fn register_pid_watch(pid: i32, recipient: &str, message: &str) -> Result<()> {
    if !proc_exists(pid) {
        bail!("no such process: {pid}");
    }
    let unit = watch_unit();
    let pid_s = pid.to_string();
    spawn_watcher(
        &unit,
        &format!("hark-job kind=pid pid={pid} to={recipient}"),
        &[
            "watch-pid",
            "--pid",
            &pid_s,
            "--to",
            recipient,
            "--message",
            message,
        ],
    )?;
    println!("Watching PID {pid}; will notify '{recipient}' when it exits. unit {unit}");
    Ok(())
}

/// Watch role: block until `event` happens to `path`, then deliver `message`.
pub fn watch_file(path: &str, event: FileEvent, recipient: &str, message: &str) -> Result<()> {
    let p = Path::new(path);
    let file_name = p
        .file_name()
        .with_context(|| format!("path has no file name: {path}"))?
        .to_os_string();
    // inotify watches a directory; we filter events by the file's basename.
    let parent = match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };

    // "created" but it already exists: the condition is already satisfied.
    if event == FileEvent::Created && p.exists() {
        return deliver_and_log(recipient, message);
    }

    let mask = match event {
        FileEvent::Created => WatchMask::CREATE | WatchMask::MOVED_TO,
        FileEvent::Modified => WatchMask::CLOSE_WRITE | WatchMask::MODIFY,
        FileEvent::Removed => WatchMask::DELETE | WatchMask::MOVED_FROM | WatchMask::DELETE_SELF,
    };

    let mut inotify = Inotify::init().context("failed to initialize inotify")?;
    inotify
        .watches()
        .add(parent, mask)
        .with_context(|| format!("failed to watch directory {}", parent.display()))?;

    let mut buffer = [0u8; 4096];
    loop {
        let events = inotify
            .read_events_blocking(&mut buffer)
            .context("failed to read inotify events")?;
        for ev in events {
            let matches_name = ev
                .name
                .map(|n| n == file_name.as_os_str())
                // DELETE_SELF has no name; it refers to the watched path itself.
                .unwrap_or(event == FileEvent::Removed);
            if matches_name {
                return deliver_and_log(recipient, message);
            }
        }
    }
}

/// Watch role: block until process `pid` exits, then deliver `message`.
pub fn watch_pid(pid: i32, recipient: &str, message: &str) -> Result<()> {
    if !proc_exists(pid) {
        bail!("no such process: {pid}");
    }
    // No exit status is available for a non-child process; poll until it goes.
    while proc_exists(pid) {
        std::thread::sleep(Duration::from_millis(1000));
    }
    deliver_and_log(recipient, message)
}

/// Whether `/proc/<pid>` currently exists.
fn proc_exists(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn deliver_and_log(recipient: &str, message: &str) -> Result<()> {
    crate::deliver::deliver(recipient, message)?;
    eprintln!("delivered to '{recipient}'");
    Ok(())
}
