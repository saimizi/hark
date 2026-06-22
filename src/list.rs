//! `list` role: show the notifications currently registered with systemd.
//!
//! `hark` keeps no state of its own — registered notifications live as systemd
//! `--user` units: `hark-*` timers (time triggers) and `hark-watch-*` services
//! (event watchers). This module queries systemd and presents them as a table.
//! Details are recovered from the unit's structured Description (written at
//! registration) and the free-form message from its ExecStart command line.

use anyhow::{Context, Result};
use chrono::DateTime;
use std::process::Command;

/// One registered notification, as derived from its systemd unit.
struct Job {
    typ: String,     // "time" | "file" | "proc" | "?"
    trigger: String, // when it fires / what it watches
    to: String,      // recipient
    state: String,   // scheduled / running / failed / ...
    message: String, // the message text
    unit: String,    // the unit to stop in order to cancel
}

/// Print the table of registered notifications.
pub fn list() -> Result<()> {
    let raw = systemctl(&["list-units", "--all", "--plain", "--no-legend", "hark-*"])?;

    let mut jobs = Vec::new();
    for line in raw.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let (unit, active, sub) = (cols[0], cols[2], cols[3]);

        // Time triggers are the .timer unit; its paired .service holds details.
        // Event watchers are hark-watch-*.service units. The bare .service that
        // pairs a timer is skipped (covered via the timer).
        if let Some(stem) = unit.strip_suffix(".timer") {
            let service = format!("{stem}.service");
            jobs.push(build_job(unit, &service, "scheduled")?);
        } else if unit.starts_with("hark-watch-") && unit.ends_with(".service") {
            let state = if active == "failed" { "failed" } else { sub };
            jobs.push(build_job(unit, unit, state)?);
        }
    }

    if jobs.is_empty() {
        println!("No registered notifications.");
        return Ok(());
    }
    render(&jobs);
    Ok(())
}

/// Query `service` for its details and assemble a [`Job`].
fn build_job(unit: &str, service: &str, state: &str) -> Result<Job> {
    let show = systemctl(&["show", service, "-p", "Description", "-p", "ExecStart"])?;
    let description = property(&show, "Description").unwrap_or_default();
    let exec = property(&show, "ExecStart").unwrap_or_default();

    let d = Desc::parse(description);
    let (typ, trigger) = match d.kind.as_str() {
        "time" => ("time", d.at.as_deref().map(format_time).unwrap_or_default()),
        "file" => (
            "file",
            format!(
                "{} {}",
                d.event.as_deref().unwrap_or("?"),
                d.path.as_deref().unwrap_or("?")
            ),
        ),
        "pid" => ("proc", format!("pid {}", d.pid.as_deref().unwrap_or("?"))),
        _ => ("?", String::new()),
    };

    Ok(Job {
        typ: typ.to_string(),
        trigger,
        to: d.to,
        state: state.to_string(),
        message: message_from_exec(exec).unwrap_or_default(),
        unit: unit.to_string(),
    })
}

/// Run `systemctl --user <args>` and return its stdout.
fn systemctl(args: &[&str]) -> Result<String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("failed to run systemctl --user")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Value of `KEY=` from `systemctl show` key/value output.
fn property<'a>(show: &'a str, key: &str) -> Option<&'a str> {
    show.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// Fields recovered from a `hark-job …` Description.
#[derive(Default)]
struct Desc {
    kind: String,
    to: String,
    at: Option<String>,
    event: Option<String>,
    path: Option<String>,
    pid: Option<String>,
}

impl Desc {
    fn parse(d: &str) -> Self {
        let mut out = Desc::default();
        // `path=` may contain spaces, so it is always last: take the remainder.
        let head = match d.split_once(" path=") {
            Some((h, p)) => {
                out.path = Some(p.to_string());
                h
            }
            None => d,
        };
        for tok in head.split_whitespace() {
            if let Some((k, v)) = tok.split_once('=') {
                match k {
                    "kind" => out.kind = v.to_string(),
                    "to" => out.to = v.to_string(),
                    "at" => out.at = Some(v.to_string()),
                    "event" => out.event = Some(v.to_string()),
                    "pid" => out.pid = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        out
    }
}

/// Recover the message from an ExecStart record: it is always the final
/// argument (`… --message <message>`), so everything after `--message ` in the
/// argv list is the message.
fn message_from_exec(exec: &str) -> Option<String> {
    let argv = exec.split_once("argv[]=")?.1;
    // The argv list ends at the next record field.
    let argv = argv.split(" ; ").next().unwrap_or(argv);
    argv.split_once(" --message ")
        .map(|(_, m)| m.trim().to_string())
}

/// Render an RFC 3339 instant as a friendly local timestamp, or pass it through.
fn format_time(rfc3339: &str) -> String {
    match DateTime::parse_from_rfc3339(rfc3339) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        Err(_) => rfc3339.to_string(),
    }
}

/// Print the jobs as a column-aligned table.
fn render(jobs: &[Job]) {
    let headers = ["TYPE", "TRIGGER", "TO", "STATE", "MESSAGE", "UNIT"];
    let mut w = headers.map(str::len);
    for j in jobs {
        for (i, f) in [&j.typ, &j.trigger, &j.to, &j.state, &j.message, &j.unit]
            .iter()
            .enumerate()
        {
            w[i] = w[i].max(f.len());
        }
    }
    let row = |c: [&str; 6]| {
        println!(
            "{:<tw$}  {:<rw$}  {:<ow$}  {:<sw$}  {:<mw$}  {}",
            c[0],
            c[1],
            c[2],
            c[3],
            c[4],
            c[5],
            tw = w[0],
            rw = w[1],
            ow = w[2],
            sw = w[3],
            mw = w[4],
        );
    };
    row(headers);
    for j in jobs {
        row([&j.typ, &j.trigger, &j.to, &j.state, &j.message, &j.unit]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_extracts_value() {
        let show = "Description=hark-job kind=time to=joukan at=2026-06-22T19:06:00+09:00\nExecStart={ ... }";
        assert_eq!(
            property(show, "Description").unwrap(),
            "hark-job kind=time to=joukan at=2026-06-22T19:06:00+09:00"
        );
    }

    #[test]
    fn desc_parses_time() {
        let d = Desc::parse("hark-job kind=time to=joukan at=2026-06-22T19:06:00+09:00");
        assert_eq!(d.kind, "time");
        assert_eq!(d.to, "joukan");
        assert_eq!(d.at.as_deref(), Some("2026-06-22T19:06:00+09:00"));
    }

    #[test]
    fn desc_parses_file_with_spaces_in_path() {
        let d = Desc::parse("hark-job kind=file event=created to=joukan path=/tmp/my dir/img.wic");
        assert_eq!(d.kind, "file");
        assert_eq!(d.event.as_deref(), Some("created"));
        assert_eq!(d.path.as_deref(), Some("/tmp/my dir/img.wic"));
    }

    #[test]
    fn desc_parses_pid() {
        let d = Desc::parse("hark-job kind=pid pid=193673 to=joukan");
        assert_eq!(d.kind, "pid");
        assert_eq!(d.pid.as_deref(), Some("193673"));
        assert_eq!(d.to, "joukan");
    }

    #[test]
    fn message_from_exec_recovers_multiword_message() {
        let exec = "{ path=/x/hark ; argv[]=/x/hark deliver --to joukan --message tea time, please ; ignore_errors=no ; start_time=[n/a] }";
        assert_eq!(message_from_exec(exec).as_deref(), Some("tea time, please"));
    }

    #[test]
    fn message_from_exec_none_without_message() {
        let exec = "{ path=/x/hark ; argv[]=/x/hark list ; ignore_errors=no }";
        assert_eq!(message_from_exec(exec), None);
    }
}
