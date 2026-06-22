//! `run` role: execute a command in the foreground, then notify with its exit
//! status. The notifying process is the command's parent, so the real exit
//! code is available — ideal for "tell me when the build finishes or fails".

use anyhow::{Context, Result};
use std::os::unix::process::ExitStatusExt;
use std::process::Command;

/// Run `cmd` (program + args), deliver a notification describing the outcome to
/// `recipient`, and exit with the command's own exit code.
///
/// `prefix` is an optional user-supplied message; when omitted the command line
/// itself is used.
pub fn run(cmd: &[String], recipient: &str, prefix: Option<&str>) -> Result<()> {
    let (program, args) = cmd
        .split_first()
        .context("no command given (use: hark run -- <cmd> [args...])")?;

    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to start command: {program}"))?;

    let base = match prefix {
        Some(p) => p.to_string(),
        None => format!("`{}`", cmd.join(" ")),
    };
    let outcome = if status.success() {
        "finished: SUCCESS".to_string()
    } else if let Some(code) = status.code() {
        format!("FAILED (exit {code})")
    } else if let Some(sig) = status.signal() {
        format!("terminated by signal {sig}")
    } else {
        "ended abnormally".to_string()
    };

    let message = format!("{base} {outcome}");
    // Best-effort: a failed notification must not mask the command's exit code.
    if let Err(e) = crate::deliver::deliver(recipient, &message) {
        eprintln!("notification failed: {e:#}");
    }

    // Propagate the command's exit status to our caller.
    std::process::exit(status.code().unwrap_or(1));
}
