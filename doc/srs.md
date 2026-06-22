# Software Requirements Specification — `hark`

**Document:** SRS
**System:** `hark` — CUI Timer Notifier
**Version:** 1.0
**Date:** 2026-06-22
**Status:** Baselined

---

## 1. Introduction

### 1.1 Purpose

This document specifies the requirements for `hark`, a console (CUI)
utility that delivers a text message to a local Linux user at a scheduled time.
It is intended for developers, reviewers, and maintainers of the tool.

### 1.2 Scope

`hark` lets a user schedule a one-shot text notification, to be delivered
either at an absolute date/time or after a relative delay. The message is
delivered to a logged-in local user's terminal. The tool requires no graphical
environment and no long-running daemon of its own; each invocation is
independent and may be issued many times.

Out of scope: recurring/repeating reminders, GUI/desktop notifications,
delivery to remote hosts, message persistence across reboots, and delivery to
users who are not logged in at delivery time.

### 1.3 Definitions, Acronyms, Abbreviations

| Term | Meaning |
|------|---------|
| CUI | Character/Console User Interface (text-only, no GUI) |
| User | A local Linux user account, identified by username |
| Sender | The user who invokes `hark` to schedule a message |
| Recipient | The user who is to receive the message |
| Absolute time | A complete calendar date and clock time |
| Relative time | A duration measured from the moment of scheduling |
| TTY | A terminal device (`/dev/tty*`, `/dev/pts/*`) |
| `write(1)` | Standard Unix utility that sends a message to a user's terminal |
| Transient timer | A systemd timer created at runtime that is not persisted to disk |

### 1.4 References

- `requirement.md` — original feature brief (source of REQ-1..REQ-6).
- `write(1)`, `wall(1)`, `who(1)` — util-linux manual pages.
- `systemd-run(1)`, `systemd.timer(5)` — systemd manual pages.

---

## 2. Overall Description

### 2.1 Product Perspective

`hark` is a standalone command-line executable. It relies on the host's
service manager (systemd, per-user instance) to hold the schedule and trigger
delivery, and on standard terminal-messaging utilities to deliver the message.
It introduces no daemon, database, or network service of its own.

### 2.2 Product Functions

- Accept a message, a delivery time, and an optional recipient.
- Schedule the message for one-shot delivery at the requested time.
- At the requested time, deliver the message to the recipient's terminal.

### 2.3 User Characteristics

Users are local Linux account holders comfortable with a shell. No special
privileges are required to notify oneself.

### 2.4 Operating Environment

- Linux with a running per-user systemd instance (`systemd --user`).
- Terminal-messaging utilities available: `write`, `wall`, `who`.
- A text terminal for the recipient to be logged into at delivery time.

### 2.5 Constraints

- No GUI dependency (REQ — CUI only).
- Each program run is a single, independent, one-shot action.

### 2.6 Assumptions and Dependencies

- The recipient is logged in on a writable terminal at delivery time.
- The recipient's per-user systemd instance is running at delivery time
  (lingering may be required if the recipient could log out before delivery).
- Cross-user delivery depends on a correctly installed, set-gid `tty`
  `write(1)` and the recipient having messaging enabled (`mesg y`).

---

## 3. Functional Requirements

Requirements are written as **shall** statements and are individually testable.

### 3.1 Scheduling input

- **FR-1 (Message).** The system shall accept a message text to be delivered.
- **FR-2 (Message required).** The system shall reject a scheduling request
  that does not provide a message, with a non-zero exit code and a diagnostic.
- **FR-3 (Recipient).** The system shall accept an optional recipient username.
- **FR-4 (Default recipient).** When no recipient is given, the system shall
  use the invoking (sender) user as the recipient.
- **FR-5 (When required).** The system shall require exactly one delivery-time
  specification: either an absolute time or a relative delay.
- **FR-6 (Mutual exclusion).** The system shall reject a request that supplies
  both an absolute time and a relative delay.

### 3.2 Time specification

- **FR-7 (Absolute time).** The system shall accept an absolute local date and
  time of the form `YYYY-MM-DD HH:MM` and `YYYY-MM-DD HH:MM:SS`.
- **FR-8 (Relative time).** The system shall accept a relative delay expressed
  as an integer quantity and a unit, supporting seconds, minutes, hours, and
  days, with common abbreviations (e.g. `90s`, `3 min`, `2 hours`, `1 day`).
- **FR-9 (Relative phrasing).** The system shall tolerate trailing natural
  phrasing on a relative delay, specifically `later` and `from now`.
- **FR-10 (Default unit).** When a relative delay omits its unit, the system
  shall interpret the quantity as minutes.
- **FR-11 (Invalid time).** The system shall reject an unparseable or
  non-positive time specification with a non-zero exit code and a diagnostic.
- **FR-12 (Past time).** The system shall reject a delivery time that is not in
  the future relative to the moment of scheduling.

### 3.3 Scheduling behavior

- **FR-13 (One-shot).** The system shall schedule the message to be delivered
  exactly once at the requested time.
- **FR-14 (Independent invocations).** The system shall allow multiple
  independent schedules to coexist, created by repeated invocations.
- **FR-15 (Self-cleanup).** After a scheduled delivery has elapsed, the system
  shall not leave residual scheduling artifacts for a successful delivery.
- **FR-16 (Confirmation).** Upon successful scheduling, the system shall report
  the resolved delivery time and an identifier for the scheduled job.
- **FR-17 (No blocking).** The scheduling invocation shall return promptly and
  shall not remain running until delivery time.

### 3.4 Delivery behavior

- **FR-18 (Terminal delivery).** At delivery time, the system shall deliver the
  message to the recipient's terminal(s) as text.
- **FR-19 (Primary mechanism).** The system shall attempt delivery using the
  standard terminal-messaging utility `write(1)`.
- **FR-20 (Fallback).** If `write(1)` cannot deliver, the system shall attempt
  to deliver directly to the recipient's own terminals, remaining targeted to
  the specific recipient (no broadcast to unrelated users).
- **FR-21 (Delivery failure).** If the message cannot be delivered to any
  terminal, the system shall report a delivery failure via a non-zero exit code
  and a diagnostic recorded where the scheduler captures service output.
- **FR-22 (Identification).** Delivered messages shall include a short banner
  identifying the sender and the time, distinguishing the text from ordinary
  terminal output.

### 3.5 Delivery when the recipient is absent

The product is a TUI (terminal) notifier: a notification only has meaning while
the recipient is present at a terminal. Delivery is therefore **best-effort**,
and a notification that cannot be shown on a terminal is intentionally dropped.

- **FR-23 (Best-effort, no retry).** Delivery shall be attempted exactly once,
  at the scheduled time. The system shall not retry, queue, or store an
  undelivered message for later delivery.
- **FR-24 (Drop when no terminal).** If, at delivery time, the recipient is not
  logged in on any reachable terminal, the system shall drop the notification.
  This is acceptable, expected behavior, not an error in the system's design.
  The attempt shall still be reported as failed per FR-21 so the outcome is
  observable in the scheduler's service log.
- **FR-25 (Drop when session ended).** If the recipient's per-user service
  manager is no longer running at delivery time (e.g. the recipient logged out
  and lingering is not enabled), the scheduled job may cease to exist before it
  fires; in that case the notification shall simply not be delivered, with no
  recovery on subsequent login.
- **FR-26 (No offline persistence).** The system shall not provide an offline
  inbox, store-and-forward, or replay-on-login mechanism. Guaranteed delivery to
  absent users is explicitly out of scope (see §1.2).

> **Design note.** These semantics follow directly from the TUI nature of the
> product: if the user is not using a terminal, there is no notification to
> receive. Reliability mechanisms (lingering, an offline inbox, sender failure
> notification) are deliberately excluded; should guaranteed delivery become a
> requirement, FR-23–FR-26 would need revision.

---

## 4. Non-Functional Requirements

- **NFR-1 (Punctuality).** Delivery shall occur within a small bounded window
  of the requested time (target: within ~1 second under normal load).
- **NFR-2 (No GUI).** The system shall operate with no graphical environment.
- **NFR-3 (No elevated privileges for self-notify).** Notifying oneself shall
  require no root privileges and no system-wide configuration.
- **NFR-4 (No persistent daemon).** The tool shall not introduce a daemon of
  its own; scheduling shall rely on the platform service manager.
- **NFR-5 (Diagnostics).** Error conditions shall produce human-readable
  messages and distinct non-zero exit codes from success.
- **NFR-6 (Portability of inputs).** Time inputs shall be interpreted in the
  host's local timezone.

---

## 5. External Interface Requirements

### 5.1 Command-line interface

```
hark --in <DURATION>  -m <MESSAGE> [-t <USER>]
hark --at <DATETIME>  -m <MESSAGE> [-t <USER>]
```

| Option | Required | Description |
|--------|----------|-------------|
| `--in <DURATION>` | one of `--in`/`--at` | Relative delay from now |
| `--at <DATETIME>` | one of `--in`/`--at` | Absolute local date/time |
| `-m, --message <TEXT>` | yes | Message text |
| `-t, --to <USER>` | no | Recipient username (default: sender) |

An internal, hidden `deliver` subcommand performs delivery and is not part of
the public interface; it is invoked only by the scheduler.

### 5.2 System interfaces

- Scheduler: per-user systemd via `systemd-run`/`systemctl --user`.
- Delivery: `write(1)`; fallback writes to terminal device files; `who(1)` is
  used to enumerate the recipient's active terminals.

### 5.3 Outputs

- On scheduling: a confirmation line on standard output.
- On error: a diagnostic on standard error and a non-zero exit code.
- On delivery: text written to the recipient's terminal(s); delivery
  diagnostics captured by the scheduler's service log.

---

## 6. Acceptance Criteria (traceability summary)

| ID | Acceptance check |
|----|------------------|
| FR-1..FR-4 | Schedule with/without message and recipient; verify defaults and rejection. |
| FR-5..FR-12 | Provide absolute, relative, both, neither, malformed, and past times; verify parse results and rejections. |
| FR-13..FR-17 | Verify single delivery, coexistence of multiple jobs, cleanup after success, confirmation output, and prompt return. |
| FR-18..FR-22 | Verify terminal delivery, fallback path, failure reporting, and banner presence. |
| FR-23..FR-26 | Schedule for a recipient who is not on any terminal; verify the message is dropped (no retry/queue/inbox) and the failure is recorded in the service log. |
| NFR-1 | Measure delivery latency against requested time. |
| NFR-2..NFR-6 | Verify no-GUI operation, self-notify without root, no daemon, diagnostics, and local-time interpretation. |

---

## 7. Requirements Mapping to Original Brief

| Original brief (`requirement.md`) | SRS requirements |
|-----------------------------------|------------------|
| CUI without GUI support | NFR-2 |
| Sent to specific user as text | FR-3, FR-18 |
| User = logged-in local user by name | FR-3, Assumptions §2.6 |
| Specify when / message / recipient | FR-1, FR-3, FR-5 |
| Recipient omitted → self | FR-4 |
| When = absolute date and time | FR-7 |
| When = relative ("3 minutes later") | FR-8, FR-9 |
