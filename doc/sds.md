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

### 2.1 The multi-role model

`hark` is a single executable that operates in one of several roles, selected
by its arguments:

- **Register role** — the user-facing path. Parses inputs, resolves the trigger
  (time, file event, or process exit), and arranges for delivery: a transient
  systemd timer for time, or a transient systemd background service for events.
  For a zero-delay time it delivers inline.
- **Deliver role** — invoked by a timer (hidden `deliver` subcommand). Sends the
  message to the recipient's terminal, then exits.
- **Watch roles** — invoked by a background service (hidden `watch-file` /
  `watch-pid`). Block until the event, then deliver, then exit.
- **Run role** — `run -- <cmd>`. Runs a command in the foreground and, on its
  exit, delivers an outcome message, then exits with the command's code.

This keeps a single binary while cleanly separating "decide and register" from
"watch/act."

### 2.2 Delegated waiting

Rather than a process that sleeps until the trigger (which would violate FR-17
and imply a daemon, NFR-4), the wait is delegated to the per-user **systemd**
instance via `systemd-run --user`:

- **Time triggers** create a **transient timer** (`--on-active`). systemd owns
  the wait, fires punctually, and garbage-collects the unit afterwards.
- **Event triggers** create a **transient service** (no `--wait`) that runs a
  watch role. The register command returns immediately (FR-31); systemd
  supervises the watcher, captures its output to the journal, and GCs the unit
  when it exits successfully (FR-33).

The **run role** is the deliberate exception: it does *not* delegate, because
being the command's parent is precisely what lets it read the real exit status
(FR-32).

### 2.3 Component diagram

```
                       register role                         act role
  user ──► hark ─┬─► time   ─► systemd-run --on-active ─► hark deliver ─┐
                 │            (transient timer)                         │
                 ├─► --on-file ─► systemd-run (service) ─► hark watch-file ─► deliver
                 ├─► --on-exit ─► systemd-run (service) ─► hark watch-pid  ─► deliver
                 │                                                      │
                 └─► run -- cmd ─► (foreground child) ──► on exit ──► deliver
                                                                       ▼
                                                          deliver ─► write(1)
                                                                  └─ direct TTY
                                                                       ▼
                                                            recipient terminal(s)
```

Time/event paths delegate the wait to systemd and return immediately; `run`
stays in the foreground so it can read the child's exit status.

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
| `main.rs` | CLI definition, role/trigger dispatch, input validation |
| `timeparse.rs` | Resolve absolute/relative "when" to a local timestamp |
| `schedule.rs` | Register the one-shot systemd transient timer (or deliver now) |
| `event.rs` | File/process triggers: register background watchers; the watch loops |
| `runcmd.rs` | `run` role: execute a command and notify on its exit |
| `list.rs` | `list` role: query systemd and report registered notifications |
| `cancel.rs` | `cancel` role: stop (remove) registered notification units |
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
       - neither → default to `Local::now()`, i.e. immediate delivery
         (FR-5, FR-27);
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

**Absolute parsing (FR-7).** `parse_naive` trims input and tries, in order,
the date-time formats `"%Y-%m-%d %H:%M:%S"` then `"%Y-%m-%d %H:%M"`; failing
those, the time-only formats `"%H:%M:%S"` then `"%H:%M"`, combining the time
with `Local::now().date_naive()` (today). The resulting naive time is mapped to
`Local`. For DST edge cases where the local time is ambiguous or nonexistent,
the earliest valid interpretation is chosen rather than failing (robustness).
Unparseable input returns an error (FR-11). A time-only value that has already
passed today yields a past instant, rejected downstream by `schedule` (FR-12).

**Relative parsing (FR-8..FR-10).**

1. Lowercase and trim; strip a trailing `later` or `from now` (FR-9).
2. Split into a leading numeric part (digits and optional sign) and a trailing
   unit, tolerating an optional space (`90s`, `3 min`).
3. Reject empty or negative quantities (FR-11). A zero quantity is accepted and
   denotes immediate delivery (FR-27).
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

1. Compute `delay = target - now` in whole seconds. Reject `< 0` as a past time
   (FR-12). If `== 0`, deliver inline immediately by calling `deliver::deliver`,
   print a "Delivered now" confirmation, and return without creating a timer
   (FR-27). Otherwise (`> 0`) proceed to register a timer.
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

### 3.5 `event.rs` — file and process triggers

Defines `FileEvent { Created, Modified, Removed }` (a clap `ValueEnum`) and two
pairs of functions: `register_*` (register role) and `watch_*` (watch role).

**Registration (background watcher).** `register_file_watch` /
`register_pid_watch` build a unique unit name `hark-watch-<ts>-<pid>` and call a
shared `spawn_watcher`, which runs `systemd-run --user --unit=… <self>
watch-file|watch-pid …` with no `--wait`. `register_file_watch` first resolves
the path to absolute (`absolute_path`) against the caller's working directory:
the watcher runs under `systemd --user` with a different CWD, so a relative path
would otherwise resolve to the wrong place. It then verifies the path's parent
directory exists and **fails fast** if not — inotify can only watch an existing
directory, so this surfaces the error to the caller at registration instead of
registering a watcher that would immediately fail in the background. Creating
the directory first is left to the user. The transient **service** starts and
the call returns immediately (FR-31). `register_pid_watch` first rejects a PID
whose `/proc/<pid>` is absent (FR-30).

**`watch_file` (FR-29).** inotify watches the **parent directory** and filters
events by the target's basename, which handles a not-yet-existing path and
removal uniformly. Event → mask:
`Created → CREATE|MOVED_TO`, `Modified → CLOSE_WRITE|MODIFY`,
`Removed → DELETE|MOVED_FROM|DELETE_SELF`. It reads events blocking until a
matching name appears (or, for `Removed`, a nameless `DELETE_SELF`), then
delivers. As a special case, `Created` on an already-existing path delivers
immediately.

**`watch_pid` (FR-30).** Polls `Path::new("/proc/<pid>").exists()` at ~1 s until
the entry disappears, then delivers. No exit status is read (it is not a child;
§1.2).

**Design choice (`/proc` polling vs `pidfd`).** Polling `/proc` is
dependency-free and adequate for the long-running jobs this targets; `pidfd`
would be event-driven and reuse-proof but needs raw syscalls or another crate.
The ~1 s latency and PID-reuse caveat are documented and acceptable for the MVP.

### 3.6 `runcmd.rs` — run-and-notify (FR-32)

`run(cmd, recipient, prefix)` splits `cmd` into program + args, runs it
foreground with inherited stdio, and `wait`s. It composes the outcome from the
`ExitStatus`: `SUCCESS`, `FAILED (exit N)`, or `terminated by signal N` (via
`ExitStatusExt`). The message is `<prefix-or-command-line> <outcome>`. Delivery
is best-effort — a failed notification is logged but must not change the exit
code. Finally it `process::exit`s with the command's own code, so `hark run`
is transparent in scripts and pipelines.

### 3.7 `list.rs` — listing registered notifications (FR-34)

`hark` stores no state of its own, so `list` reconstructs the registered set
from systemd:

1. `systemctl --user list-units --all --plain --no-legend 'hark-*'` enumerates
   the units. `*.timer` units are time triggers (details live on the paired
   `*.service`); `hark-watch-*.service` units are event watchers; the bare
   timer-paired service is skipped (covered via its timer).
2. For each, `systemctl --user show <service> -p Description -p ExecStart`
   yields the structured Description and the command line.
3. `Desc::parse` reads the Description we write at registration —
   `hark-job kind=<time|file|pid> to=<user> [at=<rfc3339>] [event=<e>] [pid=<n>]
   [path=<...>]`. Key/value tokens are space-separated; `path=` is always last
   so a path containing spaces is taken as the remainder.
4. `message_from_exec` recovers the free-form message: `--message` is always the
   final argument in every `hark` invocation, so everything after `--message `
   in the `argv[]` field (up to the next ` ; ` record boundary) is the message.

**Design choice (encode details in the unit, not a state file).** Keeping the
trigger/recipient in the unit Description and reading the message back from
ExecStart preserves the "systemd is the single source of truth" invariant — no
sidecar store to keep in sync, and `list` cannot drift from reality. The
`systemctl show` output flattens `argv[]` (losing word boundaries), which is
why details that may contain spaces (message, path) are positioned last so they
can be taken as a remainder.

### 3.8 `cancel.rs` — cancelling notifications (FR-35)

`cancel(units, all)` removes registered notifications by stopping their units:

- With `--all`, `hark_units` lists the registered units (`parse_unit_names`
  keeps `*.timer` and `hark-watch-*.service`; the timer-paired inactive service
  is omitted because stopping the timer cancels the job).
- Otherwise each named unit is checked to start with `hark-` (refusing to act on
  foreign units) and stopped via `systemctl --user stop`.
- Each unit is stopped individually so a failure on one is reported without
  aborting the rest; a final `reset-failed hark-*` clears any failed leftovers
  so `list` stays clean.

Stopping a timer before it elapses prevents the delivery; stopping a watcher
service ends the watch. This reuses systemd as the source of truth — no
separate registry to update.

---

## 4. Data Design

`hark` holds no persistent application state. The only "state" is the set of
pending transient units owned by systemd: timer units (`hark-…`) for time
triggers and service units (`hark-watch-…`) for active event watchers.

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
| Event watchers as transient systemd **services** | Background, supervised, journal-logged, GC'd; register call returns at once (FR-31) | systemd `.path` units (no transient form via `systemd-run`, can't watch removal); foreground blocking (ties up shell) |
| inotify on the **parent directory** | Uniformly handles not-yet-existing paths, creation, and removal by basename filter | watching the file node directly (misses creation; awkward for removal) |
| `/proc` polling for PID exit | Dependency-free; ~1 s latency fine for long jobs | `pidfd` (event-driven, reuse-proof, but raw syscalls / extra crate) |
| `run -- <cmd>` is the command's parent | Only way to read the real exit status (FR-32) | Attach to an external PID (no exit code obtainable) |

---

## 7. Testing Strategy

- **Unit tests** (`#[cfg(test)]`):
  - `timeparse`: absolute with/without seconds, garbage rejection; relative
    variants (`3 minutes later`, `3 min`, `3m`, `90s`, `2 hours`, `1 day from
    now`, bare number), and rejection of malformed/zero/unknown-unit input.
  - `schedule`: `humanize` formatting across day/hour/minute/second mixes.
  - `list`: `Desc::parse` (time/file/pid, path with spaces), `message_from_exec`
    (multi-word message, none), `property` extraction.
  - `cancel`: `parse_unit_names` (keeps timers/watchers, omits paired service).
- **Integration / manual** (host-dependent, require a logged-in terminal and
  systemd `--user`):
  - Schedule a short self-notify; verify the timer fires within ~1s (NFR-1),
    the message reaches the terminal(s), the service result is success, and the
    unit is garbage-collected (FR-13, FR-15, FR-18).
  - Verify confirmation output and immediate return (FR-16, FR-17).
  - Verify rejection paths: missing message, past time, malformed time, and two
    triggers at once (FR-2, FR-11, FR-12, FR-28).
  - Event triggers: register `--on-file`/`--on-exit`, confirm prompt return and
    a running `hark-watch-*` unit, cause the event, and confirm delivery plus
    unit GC (FR-29, FR-30, FR-31, FR-33).
  - `run -- <cmd>`: a passing and a failing command; confirm the outcome message
    and that hark exits with the command's code (FR-32).

---

## 8. Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI parsing and validation |
| `chrono` | Local date/time and duration arithmetic |
| `anyhow` | Error propagation with context |
| `whoami` | Current username and host for default recipient and banner |
| `inotify` (default features off) | Filesystem-event watching (blocking API; no async/tokio) |

External commands: `systemd-run`, `systemctl` (scheduling/watching); `write`,
`who` (delivery). Kernel/OS: `inotify`, `/proc`.

---

## 9. Traceability (SRS → design)

| SRS | Design element |
|-----|----------------|
| FR-1..FR-6 | `main.rs` CLI model + validation |
| FR-28 | clap `trigger` arg group (mutually exclusive) |
| FR-29 | `event::watch_file` (inotify on parent dir) + `register_file_watch` |
| FR-30 | `event::watch_pid` (`/proc` poll) + `register_pid_watch` (PID precheck) |
| FR-31, FR-33 | `event::spawn_watcher` (transient service, no `--wait`); systemd GC |
| FR-32 | `runcmd::run` (foreground, exit-code propagation, outcome message) |
| FR-34 | `list::list` (systemctl query; `Desc::parse` + `message_from_exec`); structured Descriptions set in `schedule`/`event` |
| FR-35 | `cancel::cancel` (`systemctl stop`; `parse_unit_names`; hark-only guard) |
| FR-7 | `timeparse::parse_absolute` |
| FR-8..FR-10 | `timeparse::parse_duration` |
| FR-11, FR-12 | `timeparse` errors; `schedule` past-time check |
| FR-27 | `main.rs` no-time default to `Local::now()`; `schedule` `secs == 0` inline-delivery branch; `timeparse` accepts zero |
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
