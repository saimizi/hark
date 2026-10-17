//! Delivery role: actually send the text to the recipient, now. Runs inside
//! the transient systemd timer when it fires, so output goes to the journal.

use anyhow::{bail, Context, Result};
use chrono::Local;
#[allow(unused)]
use jlogger_tracing::{jdebug, jerror, jinfo, jwarn};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::process::{Command, Stdio};

/// Deliver `message` to `recipient`.
///
/// Self-notifications write directly to all owned terminal devices. Other
/// recipients use write(1) first, with device discovery as a fallback.
/// Output/errors land in the systemd journal for transient units.
pub fn deliver(recipient: &str, message: &str) -> Result<()> {
    // Self-notifications go to every owned terminal, including sessions that
    // terminal emulators do not register in utmp (and therefore `who`).
    let uid = recipient_uid(recipient)?;
    if uid == unsafe { libc::getuid() } && deliver_via_ttys(uid, message)? > 0 {
        return Ok(());
    }
    match deliver_via_write(recipient, message) {
        Ok(()) => Ok(()),
        Err(write_err) => {
            jerror!("write(1) failed ({write_err:#}); trying direct TTY delivery");
            let delivered = deliver_via_ttys(uid, message).context("direct TTY delivery failed")?;
            if delivered == 0 {
                bail!(
                    "could not reach '{recipient}': not logged in on a writable terminal \
                     (messaging disabled, or no permission to other users' TTYs)"
                );
            }
            jdebug!("delivered to {delivered} terminal(s) of '{recipient}'");
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
fn deliver_via_ttys(uid: u32, message: &str) -> Result<usize> {
    let banner = banner();
    let mut delivered = 0;

    for path in recipient_ttys(uid)? {
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(&path)
        {
            Ok(mut dev) => {
                // Recheck the opened device in case it changed during discovery.
                let metadata = dev.metadata()?;
                if metadata.uid() != uid || !metadata.file_type().is_char_device() {
                    continue;
                }
                // CRLF so the text renders cleanly on a raw terminal line.
                let payload = format!("\r\n{banner}\r\n{}\r\n", message.replace('\n', "\r\n"));
                if let Err(e) = dev.write_all(payload.as_bytes()) {
                    jerror!("could not write to {path}: {e}");
                } else {
                    delivered += 1;
                    jdebug!("delivered to {path}");
                }
            }
            // Messaging disabled / not ours / not a real device: skip quietly.
            Err(e) => jwarn!("skipping {path}: {e}"),
        }
    }
    Ok(delivered)
}

/// Resolve through NSS, including users supplied by directory services.
fn recipient_uid(recipient: &str) -> Result<u32> {
    let output = Command::new("getent")
        .args(["passwd", recipient])
        .output()
        .context("failed to run getent passwd")?;
    if !output.status.success() {
        bail!("could not resolve recipient: {recipient}");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split(':')
        .nth(2)
        .context("missing UID in passwd record")?
        .parse()
        .context("invalid UID in passwd record")
}

/// Discover owned PTYs and numbered Linux consoles without login records.
fn recipient_ttys(uid: u32) -> Result<Vec<String>> {
    let mut ttys = Vec::new();
    for (directory, prefix) in [("/dev/pts", ""), ("/dev", "tty")] {
        for entry in
            std::fs::read_dir(directory).with_context(|| format!("could not scan {directory}"))?
        {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(number) = name.strip_prefix(prefix) else {
                continue;
            };
            if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let path = entry.path();
            // Do not follow aliases or symlinks to unrelated devices.
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.uid() == uid && metadata.file_type().is_char_device() {
                ttys.push(path.to_string_lossy().into_owned());
            }
        }
    }
    ttys.sort();
    Ok(ttys)
}

/// A short banner identifying the sender, mirroring write(1)'s preamble.
fn banner() -> String {
    let sender = whoami::username();
    let now = Local::now().format("%H:%M");
    format!("Message from {sender} (hark) at {now} ...")
}
