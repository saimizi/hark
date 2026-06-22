//! Delivery role: actually send the text to the recipient, now. Runs inside
//! the transient systemd timer when it fires, so output goes to the journal.

use anyhow::{bail, Context, Result};
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};

/// Deliver `message` to `recipient`.
///
/// Primary path is write(1), which targets a logged-in user's TTY. On systems
/// where write(1) is unusable (e.g. missing its set-gid `tty` bit) we fall back
/// to writing directly to the recipient's own terminals — the same mechanism
/// write(1) uses, but limited to TTYs this process can open. Output/errors land
/// in the systemd journal for the transient unit.
pub fn deliver(recipient: &str, message: &str) -> Result<()> {
    match deliver_via_write(recipient, message) {
        Ok(()) => Ok(()),
        Err(write_err) => {
            eprintln!("write(1) failed ({write_err:#}); trying direct TTY delivery");
            let delivered =
                deliver_via_ttys(recipient, message).context("direct TTY delivery failed")?;
            if delivered == 0 {
                bail!(
                    "could not reach '{recipient}': not logged in on a writable terminal \
                     (messaging disabled, or no permission to other users' TTYs)"
                );
            }
            eprintln!("delivered to {delivered} terminal(s) of '{recipient}'");
            Ok(())
        }
    }
}

/// Pipe the message into `write <recipient>`.
fn deliver_via_write(recipient: &str, message: &str) -> Result<()> {
    let mut child = Command::new("write")
        .arg(recipient)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to launch write(1)")?;

    {
        let mut stdin = child
            .stdin
            .take()
            .context("could not open stdin of write(1)")?;
        // write(1) relays each line; a trailing newline flushes the final line.
        stdin
            .write_all(message.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .context("failed to pipe message to write(1)")?;
        // stdin dropped here -> EOF, so write(1) finishes.
    }

    let output = child
        .wait_with_output()
        .context("failed to wait on write(1)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("write(1) exited with {}: {}", output.status, stderr.trim());
    }
    Ok(())
}

/// Write the message directly to each of the recipient's terminals that this
/// process can open, returning how many received it.
fn deliver_via_ttys(recipient: &str, message: &str) -> Result<usize> {
    let banner = banner();
    let mut delivered = 0;

    for tty in recipient_ttys(recipient)? {
        let path = format!("/dev/{tty}");
        match OpenOptions::new().write(true).open(&path) {
            Ok(mut dev) => {
                // CRLF so the text renders cleanly on a raw terminal line.
                let payload = format!("\r\n{banner}\r\n{}\r\n", message.replace('\n', "\r\n"));
                if let Err(e) = dev.write_all(payload.as_bytes()) {
                    eprintln!("could not write to {path}: {e}");
                } else {
                    delivered += 1;
                }
            }
            // Messaging disabled / not ours / not a real device: skip quietly.
            Err(e) => eprintln!("skipping {path}: {e}"),
        }
    }
    Ok(delivered)
}

/// The terminals `recipient` is currently logged in on, per who(1).
fn recipient_ttys(recipient: &str) -> Result<Vec<String>> {
    let output = Command::new("who")
        .output()
        .context("failed to run who(1)")?;
    if !output.status.success() {
        bail!("who(1) exited with {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let ttys = text
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let user = cols.next()?;
            let tty = cols.next()?;
            // Only real character-device terminals (tty*/pts/*), not X displays.
            if user == recipient && (tty.starts_with("tty") || tty.starts_with("pts/")) {
                Some(tty.to_string())
            } else {
                None
            }
        })
        .collect();
    Ok(ttys)
}

/// A short banner identifying the sender, mirroring write(1)'s preamble.
fn banner() -> String {
    let sender = whoami::username();
    let now = Local::now().format("%H:%M");
    format!("Message from {sender} (hark) at {now} ...")
}
