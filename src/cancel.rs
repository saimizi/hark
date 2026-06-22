//! `cancel` role: stop (remove) registered notification units.
//!
//! Registered notifications are systemd `--user` transient units; cancelling
//! one is just stopping its unit. Targets are identified by the unit name shown
//! in the `UNIT` column of `hark list`.

use anyhow::{bail, Context, Result};
use std::process::Command;

/// Cancel the named `units`, or every hark notification when `all` is set.
pub fn cancel(units: &[String], all: bool) -> Result<()> {
    if all && !units.is_empty() {
        bail!("give either UNIT name(s) or --all, not both");
    }

    let targets: Vec<String> = if all {
        hark_units()?
    } else {
        if units.is_empty() {
            bail!("specify a UNIT to cancel (see `hark list`), or pass --all");
        }
        // Safety: only ever act on our own units.
        for u in units {
            if !u.starts_with("hark-") {
                bail!("refusing to cancel non-hark unit: {u}");
            }
        }
        units.to_vec()
    };

    if targets.is_empty() {
        println!("No registered notifications.");
        return Ok(());
    }

    let mut cancelled = 0;
    for unit in &targets {
        match stop(unit) {
            Ok(()) => {
                println!("Cancelled {unit}");
                cancelled += 1;
            }
            Err(e) => eprintln!("could not cancel {unit}: {e:#}"),
        }
    }

    // Tidy any failed transient leftovers so `hark list` stays clean.
    let _ = systemctl(&["reset-failed", "hark-*"]);

    if cancelled == 0 {
        bail!("nothing was cancelled");
    }
    Ok(())
}

/// Stop a single unit via `systemctl --user stop`.
fn stop(unit: &str) -> Result<()> {
    let out = Command::new("systemctl")
        .arg("--user")
        .arg("stop")
        .arg(unit)
        .output()
        .context("failed to run systemctl --user stop")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// All currently-registered hark units: time timers and watcher services.
fn hark_units() -> Result<Vec<String>> {
    let raw = systemctl(&["list-units", "--all", "--plain", "--no-legend", "hark-*"])?;
    Ok(parse_unit_names(&raw))
}

/// Extract cancelable unit names from `systemctl list-units` output: time
/// timers and watcher services (the timer-paired inactive service is omitted,
/// since stopping the timer cancels the job).
fn parse_unit_names(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|u| {
            u.ends_with(".timer") || (u.starts_with("hark-watch-") && u.ends_with(".service"))
        })
        .map(String::from)
        .collect()
}

fn systemctl(args: &[&str]) -> Result<String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("failed to run systemctl --user")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unit_names_keeps_timers_and_watchers() {
        let raw = "\
hark-20260622185037-1.timer            loaded active   waiting  hark-job ...
hark-20260622185037-1.service          loaded inactive dead     hark-job ...
hark-watch-20260622192602-2.service    loaded active   running  hark-job ...
hark-watch-20260622192238-3.service    loaded failed   failed   hark-job ...";
        let names = parse_unit_names(raw);
        assert_eq!(
            names,
            vec![
                "hark-20260622185037-1.timer".to_string(),
                "hark-watch-20260622192602-2.service".to_string(),
                "hark-watch-20260622192238-3.service".to_string(),
            ]
        );
        // The timer-paired inactive service is intentionally omitted.
        assert!(!names.iter().any(|n| n == "hark-20260622185037-1.service"));
    }

    #[test]
    fn parse_unit_names_empty() {
        assert!(parse_unit_names("").is_empty());
    }
}
