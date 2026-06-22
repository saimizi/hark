# Software Design Specification — `hark`

**Document:** SDS
**System:** `hark` — CUI Timer Notifier
**Version:** 1.0
**Date:** 2026-06-22
**Implements:** SRS v1.0 (`doc/srs.md`)

---

## 1. Introduction

### 1.1 Purpose

This document describes the design of `hark`: its architecture, module
decomposition, data, key algorithms, and the rationale behind major decisions.
It refines the requirements in `doc/srs.md` into an implementable design.

### 1.2 Design goals

- Satisfy the SRS with the smallest reliable moving-part count.
- No bespoke daemon, persistence, or privilege escalation for the common case.
- Punctual, one-shot, self-cleaning schedules.
- Graceful, targeted delivery that degrades sensibly on imperfect hosts.

---

## 2. Architectural Overview

### 2.1 The two-role model

`hark` is a single executable that operates in one of two roles, selected
by its arguments:

- **Schedule role** — the user-facing path. Parses inputs, resolves the
  delivery time, and registers a one-shot timer that will later re-invoke this
  same executable in the deliver role.
- **Deliver role** — invoked by the timer (hidden `deliver` subcommand). Sends
  the message to the recipient's terminal, then exits.

This keeps a single binary while cleanly separating "decide and register" from
"act now."

### 2.2 Delegated scheduling

Rather than running a process that sleeps until delivery time (which would
violate FR-17 and require a daemon, NFR-4), scheduling is delegated to the
per-user **systemd** instance through a **transient timer** created with
`systemd-run --user`. systemd owns the wait, survives the scheduling process's
exit, fires punctually, and garbage-collects the unit afterwards.

### 2.3 Component diagram

```
                         schedule role                       deliver role
  user ──► hark ─┬─► timeparse ──► (absolute time)        (timer fires)
                 │                                                  │
                 └─► schedule ──► systemd-run --user ──► hark deliver
                                         (transient timer)          │
                                                                    ▼
                                                                 deliver
                                                              ├─ write(1)
                                                              └─ direct TTY
                                                                    │
                                                                    ▼
                                                          recipient terminal(s)
```

### 2.4 Runtime sequence (relative example)

```
$ hark --in "3 minutes later" -m "tea" -t alice

main            parse CLI, recipient = alice
timeparse       "3 minutes later" -> now + 3min  -> target T
schedule        delay = T - now = 180s
                exec: systemd-run --user --on-active=180s
                      --timer-property=RemainAfterElapse=no
                      --timer-property=AccuracySec=1s
                      --unit=hark-<ts>-<pid>
                      <self> deliver --to alice -m "tea"
main            print confirmation, exit 0
   ... 180s later, systemd starts the transient service ...
deliver         try write(1) -> on failure, direct-TTY fallback
                write banner + "tea" to alice's terminal(s)
                exit 0  (timer unit is GC'd)
```

---

## 3. Module Design

Source layout (`src/`):

| Module | Responsibility |
|--------|----------------|
| `main.rs` | CLI definition, role dispatch, input validation |
| `timeparse.rs` | Resolve absolute/relative "when" to a local timestamp |
| `schedule.rs` | Register the one-shot systemd transient timer |
| `deliver.rs` | Deliver the message to the recipient's terminal |

### 3.1 `main.rs` — CLI and dispatch

- Defines the argument model with `clap` (derive API).
  - Top-level scheduling options: `--at`, `--in` (mutually exclusive via
    `conflicts_with`), `-m/--message`, `-t/--to`.
  - Hidden `deliver` subcommand with `--to` and `-m/--message` (required there).
- Dispatch:
  - If the `deliver` subcommand is present → call `deliver::deliver`.
  - Otherwise (schedule role):
    1. Require a message (FR-2).
    2. Resolve recipient: `--to` or current user via `whoami::username()`
       (FR-4).
    3. Resolve the delivery time by matching on `(--at, --in)`:
       - exactly one present → call the corresponding `timeparse` function;
       - neither → error (FR-5);
       - both → unreachable (clap enforces FR-6).
    4. Call `schedule::schedule`.
- Errors propagate as `anyhow::Result`, yielding a diagnostic and non-zero exit
  (NFR-5).

**Rationale (single binary, hidden subcommand).** A hidden `deliver`
subcommand lets the scheduler re-invoke the same executable without exposing an
internal command in help, and avoids shipping/locating a second binary.

### 3.2 `timeparse.rs` — time resolution

Public API:

- `parse_absolute(&str) -> Result<DateTime<Local>>`
- `parse_relative(&str) -> Result<DateTime<Local>>`

Internal: `parse_duration(&str) -> Result<chrono::Duration>`.

**Absolute parsing (FR-7).** Trims input and tries, in order,
`"%Y-%m-%d %H:%M:%S"` then `"%Y-%m-%d %H:%M"` via
`NaiveDateTime::parse_from_str`. The naive time is mapped to `Local`. For DST
edge cases where the local time is ambiguous or nonexistent, the earliest valid
interpretation is chosen rather than failing (robustness). Unparseable input
returns an error (FR-11).

**Relative parsing (FR-8..FR-10).**

1. Lowercase and trim; strip a trailing `later` or `from now` (FR-9).
2. Split into a leading numeric part (digits and optional sign) and a trailing
   unit, tolerating an optional space (`90s`, `3 min`).
3. Reject empty/zero/negative quantities (FR-11).
4. Map the unit to a `Duration`:
   - `s/sec/secs/second/seconds` → seconds
   - `(empty)/m/min/mins/minute/minutes` → minutes (empty ⇒ default minutes,
     FR-10)
   - `h/hr/hrs/hour/hours` → hours
   - `d/day/days` → days
   - anything else → error.
5. `parse_relative` returns `Local::now() + duration`.

**Design choice (hand-rolled parser).** The accepted grammar is tiny and
domain-specific (including natural trailing words). A small explicit parser is
clearer and dependency-light versus a general duration crate, and is directly
unit-tested.

### 3.3 `schedule.rs` — timer registration

Public API: `schedule(target: DateTime<Local>, recipient: &str, message: &str)`.

Algorithm:

1. Compute `delay = target - now` in whole seconds; reject `<= 0` (FR-12).
2. Determine the executable path via `std::env::current_exe()` so the timer
   re-invokes exactly this binary.
3. Build a unique unit name `hark-<YYYYMMDDHHMMSS>-<pid>` (FR-14 — uniqueness
   so concurrent schedules coexist).
4. Spawn `systemd-run` with:
   - `--user` — per-user instance, no root (NFR-3).
   - `--on-active=<delay>s` — fire once after the delay. Both absolute and
     relative inputs are normalized to a delay, avoiding calendar/timezone
     edge-cases in the timer itself.
   - `--timer-property=RemainAfterElapse=no` — GC the unit after it elapses
     (FR-15, one-shot self-cleanup).
   - `--timer-property=AccuracySec=1s` — override systemd's default 1-minute
     accuracy for punctual delivery (NFR-1).
   - `--unit=<name>`, `--description=...`.
   - The executable path followed by `deliver --to <recipient> -m <message>`.
5. On non-zero `systemd-run` status → error. On success → print a confirmation
   with the resolved time, a human-readable delay, and the unit name (FR-16).
   The process returns immediately (FR-17).

Helper `humanize(secs)` renders the delay as `1d 2h 3m 4s` (omitting zero
parts) for the confirmation line.

**Design choice (normalize to `--on-active`).** Converting absolute times to a
relative delay means a single, well-understood timer property handles both
input modes and sidesteps `OnCalendar` parsing differences.

### 3.4 `deliver.rs` — message delivery

Public API: `deliver(recipient: &str, message: &str)`.

Strategy (FR-18..FR-22): attempt the standard utility first, then a targeted
fallback.

1. **Primary — `deliver_via_write`.** Spawn `write <recipient>`, pipe the
   message followed by a newline to its stdin, capture stderr, and `wait`.
   Non-zero exit ⇒ `Err` carrying the captured stderr.
2. **Fallback — `deliver_via_ttys`** (only if the primary failed):
   - `recipient_ttys` runs `who(1)`, keeps lines whose first column equals the
     recipient and whose terminal column begins with `tty` or `pts/` (excludes
     X displays such as `:0`).
   - For each terminal, open `/dev/<tty>` for writing. On success, write a
     CRLF-wrapped banner and the message (interior newlines converted to CRLF).
     Count successful writes. Terminals that cannot be opened (messaging
     disabled, not owned, not a real device) are skipped with a note.
   - Returns the count delivered.
3. If the fallback delivered to zero terminals → error (FR-21). Otherwise →
   success, noting how many terminals received it.

`banner()` produces `Message from <sender> (hark) at HH:MM ...`,
distinguishing the message from ordinary output (FR-22).

**Design choice (write-then-direct fallback).** `write(1)` is the canonical,
correct mechanism and is preferred (FR-19). However, it fails on hosts where
the `write` binary has lost its set-gid `tty` bit. The direct-TTY fallback
reproduces what `write` does internally but stays **targeted** to the specific
recipient — unlike `wall`, it does not broadcast to unrelated users (FR-20).
For the default self-notify case the user owns their own terminals, so the
fallback succeeds without elevated privileges.

---

## 4. Data Design

`hark` holds no persistent application state. The only "state" is the set
of pending transient timer units, owned and stored by systemd.

Transient values during a run:

| Item | Type | Where |
|------|------|-------|
| Resolved delivery time | `chrono::DateTime<Local>` | schedule role |
| Delay (seconds) | `i64` | `schedule` |
| Unit name | `String` (`hark-<ts>-<pid>`) | `schedule` |
| Recipient terminals | `Vec<String>` | `deliver` fallback |

---

## 5. Error Handling

- All fallible functions return `anyhow::Result`; `main` returns
  `Result<()>`, so any error prints a chained diagnostic to stderr and exits
  non-zero (NFR-5).
- Delivery diagnostics in the deliver role are written to stdout/stderr, which
  systemd captures into the service's journal for post-hoc inspection.
- Categories: input/parse errors (schedule role, before any side effect);
  scheduler errors (`systemd-run` failure); delivery errors (no reachable
  terminal).

---

## 6. Design Decisions and Rationale (summary)

| Decision | Rationale | Alternatives rejected |
|----------|-----------|-----------------------|
| systemd `--user` transient timer | One-shot, punctual, self-cleaning, no daemon, no root | `cron` (recurring, manual cleanup, 1-min granularity); `at`/`atd` (not installed); custom sleeping daemon (NFR-4) |
| Single binary, hidden `deliver` subcommand | One artifact; clean re-invocation by the timer | Separate delivery binary (deployment/path complexity) |
| Normalize all times to `--on-active=<secs>` | One timer mechanism for both input modes; avoids calendar parsing quirks | `--on-calendar` for absolute (extra edge cases) |
| `RemainAfterElapse=no` + `AccuracySec=1s` | Self-cleanup (FR-15) and punctuality (NFR-1) | systemd defaults (lingering units, ~1-min lateness) |
| `write(1)` then direct-TTY fallback | Canonical first; robust + targeted when `write` is broken | `wall` fallback (broadcasts to all users — not targeted) |
| Hand-rolled duration parser | Tiny domain grammar incl. natural phrasing; few deps; testable | General-purpose duration crate |

---

## 7. Testing Strategy

- **Unit tests** (`#[cfg(test)]`):
  - `timeparse`: absolute with/without seconds, garbage rejection; relative
    variants (`3 minutes later`, `3 min`, `3m`, `90s`, `2 hours`, `1 day from
    now`, bare number), and rejection of malformed/zero/unknown-unit input.
  - `schedule`: `humanize` formatting across day/hour/minute/second mixes.
- **Integration / manual** (host-dependent, require a logged-in terminal and
  systemd `--user`):
  - Schedule a short self-notify; verify the timer fires within ~1s (NFR-1),
    the message reaches the terminal(s), the service result is success, and the
    unit is garbage-collected (FR-13, FR-15, FR-18).
  - Verify confirmation output and immediate return (FR-16, FR-17).
  - Verify rejection paths: missing message, no time, past time, malformed time
    (FR-2, FR-5, FR-11, FR-12).

---

## 8. Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI parsing and validation |
| `chrono` | Local date/time and duration arithmetic |
| `anyhow` | Error propagation with context |
| `whoami` | Current username and host for default recipient and banner |

External commands: `systemd-run`, `systemctl` (scheduling); `write`, `who`
(delivery).

---

## 9. Traceability (SRS → design)

| SRS | Design element |
|-----|----------------|
| FR-1..FR-6 | `main.rs` CLI model + validation |
| FR-7 | `timeparse::parse_absolute` |
| FR-8..FR-10 | `timeparse::parse_duration` |
| FR-11, FR-12 | `timeparse` errors; `schedule` past-time check |
| FR-13, FR-15 | `--on-active`, `RemainAfterElapse=no` |
| FR-14 | unique unit name per invocation |
| FR-16, FR-17 | confirmation print; immediate return |
| FR-18, FR-19 | `deliver_via_write` |
| FR-20 | `deliver_via_ttys` (targeted fallback) |
| FR-21 | zero-delivery error |
| FR-22 | `banner()` |
| NFR-1 | `AccuracySec=1s` |
| NFR-2 | no GUI calls anywhere |
| NFR-3 | `systemd-run --user`, no root |
| NFR-4 | no daemon; systemd owns the wait |
| NFR-5 | `anyhow` diagnostics + exit codes |
| NFR-6 | `chrono::Local` throughout |
