//! Scheduling role: register a one-shot systemd `--user` transient timer that
//! will re-invoke this binary in the `deliver` role when the time arrives.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local};
use std::process::Command;

/// Schedule delivery of `message` to `recipient` at `target`.
pub fn schedule(
    target: DateTime<Local>,
    recipient: &str,
    message: &str,
    verbose: u8,
) -> Result<()> {
    let now = Local::now();
    let delay = target - now;
    let secs = delay.num_seconds();

    if secs < 0 {
        bail!(
            "target time {} is in the past",
            target.format("%Y-%m-%d %H:%M:%S")
        );
    }

    // Zero delay (e.g. `--in 0s`) means "send now": deliver inline rather than
    // registering a timer, so the caller gets the real delivery result and no
    // transient unit is created.
    if secs == 0 {
        crate::deliver::deliver(recipient, message)?;
        println!("Delivered now to '{recipient}'");
        return Ok(());
    }

    let exe = std::env::current_exe().context("could not determine own executable path")?;

    // Unique unit name; timestamp + pid is plenty to avoid collisions.
    let unit = format!("hark-{}-{}", now.format("%Y%m%d%H%M%S"), std::process::id());

    let status = Command::new("systemd-run")
        .arg("--user")
        .arg(format!("--on-active={secs}s"))
        // Garbage-collect the timer unit once it has elapsed: true one-shot.
        .arg("--timer-property=RemainAfterElapse=no")
        // Default accuracy is 1min, which can fire many seconds late;
        // hark wants to be punctual.
        .arg("--timer-property=AccuracySec=1s")
        .arg(format!("--unit={unit}"))
        // Structured description so `hark list` can recover the details.
        .arg(format!(
            "--description=hark-job kind=time to={recipient} at={}",
            target.to_rfc3339()
        ))
        .arg(&exe)
        .args(std::iter::repeat("-v").take(verbose.into()))
        .arg("deliver")
        .arg("--to")
        .arg(recipient)
        .arg("--message")
        .arg(message)
        .status()
        .context("failed to launch systemd-run (is systemd --user available?)")?;

    if !status.success() {
        bail!("systemd-run exited with {status}");
    }

    println!(
        "Scheduled notify to '{recipient}' at {} (in {}), unit {unit}",
        target.format("%Y-%m-%d %H:%M:%S"),
        humanize(secs),
    );
    Ok(())
}

/// Render a second count as a short human string for the confirmation line.
fn humanize(secs: i64) -> String {
    let (d, h, m, s) = (
        secs / 86400,
        (secs % 86400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(format!("{d}d"));
    }
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if m > 0 {
        parts.push(format!("{m}m"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(format!("{s}s"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_basic() {
        assert_eq!(humanize(45), "45s");
        assert_eq!(humanize(90), "1m 30s");
        assert_eq!(humanize(3600), "1h");
        assert_eq!(humanize(90061), "1d 1h 1m 1s");
    }
}
