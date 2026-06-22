# hark

A CUI (no-GUI) notifier. Deliver a text message to a local Linux user when a
**time** arrives or when an **event** happens — a file is created/modified/
removed, or a process exits. Built for "kick off a long job, get pinged when it
finishes." One-shot, callable many times.

## How it works

`hark` resolves a *trigger* and then delivers a message:

- **Time triggers** register a one-shot **systemd `--user` transient timer**
  (`systemd-run`) that re-invokes the binary later. The timer self-destructs
  after firing (`RemainAfterElapse=no`) and fires punctually (`AccuracySec=1s`).
- **Event triggers** (`--on-file`, `--on-exit`) register a background systemd
  `--user` *service* that watches (inotify for files, `/proc` polling for a
  PID), then delivers when the event fires. The command returns immediately.
- **`run -- <cmd>`** executes a command in the foreground and notifies on exit,
  **including its success/failure status** (it is the command's parent, so it
  has the real exit code).

Delivery (hidden `deliver` role) sends the text to the recipient's terminal via
`write(1)`, falling back to writing directly to the recipient's TTYs when
`write(1)` is unusable.

No root, no daemon of its own, no persistent state — each invocation is
independent.

## Usage

```sh
# Relative time, to yourself (default recipient = current user)
hark --in "3 minutes later" -m "stretch your legs"
hark --in 90s -m "tea is ready"

# Send now — delivered inline, no timer created.
# `--in 0s` and omitting the time entirely are equivalent.
hark --in 0s -m "ping"
hark -m "ping"

# Absolute date/time
hark --at "2026-06-22 15:30" -m "standup" -t alice

# Time only -> today at that time (rejected if it already passed today)
hark --at "15:30" -m "standup"
```

| Flag | Meaning |
|------|---------|
| `--in <DURATION>` | relative delay: `3 minutes later`, `90s`, `2 hours`, `1 day`; `0s` = now |
| `--at <DATETIME>` | absolute local time: `YYYY-MM-DD HH:MM[:SS]`, or `HH:MM[:SS]` = today |
| `-m, --message`   | the message text (required) |
| `-t, --to <USER>` | recipient username (defaults to current user) |

`--in`, `--at`, `--on-file`, and `--on-exit` are mutually exclusive triggers. If
none is given, the message is delivered immediately.

## Event triggers

```sh
# Notify when a build finishes — with its success/failure status (best option).
hark run -- bitbake core-image-minimal
hark run -m "nightly" -- make -j8        # custom message; outcome appended

# Notify when an already-running process exits (no exit status available).
hark --on-exit 48213 -m "build done"

# Notify on a filesystem event (created | modified | removed; default modified).
hark --on-file ./tmp/deploy/image.wic --event created -m "image ready"
hark --on-file build.log --event modified -m "log changed"
```

`run -- <cmd>` runs in the foreground and exits with the command's own exit
code. `--on-file`/`--on-exit` return immediately and watch in the background.

| Trigger | Meaning |
|---------|---------|
| `run -- <cmd>` | run a command, notify on exit with SUCCESS / FAILED (exit N) |
| `--on-exit <PID>` | notify when an existing process exits |
| `--on-file <PATH> --event <E>` | notify on `created`/`modified`/`removed` |

## Listing / cancelling jobs

`hark list` shows everything currently registered — time jobs and event
watchers — with the unit name you need to cancel one:

```sh
$ hark list
TYPE  TRIGGER                  TO      STATE      MESSAGE             UNIT
time  2026-06-22 19:06:00      joukan  scheduled  tea time, please    hark-2026...timer
file  created /tmp/img.wic     joukan  running    image ready         hark-watch-...service
proc  pid 48213                joukan  running    build done          hark-watch-...service
```

`hark` keeps no state of its own; the list is reconstructed from systemd. Time
jobs are transient timer units `hark-<timestamp>-<pid>`; event watchers are
transient service units `hark-watch-<timestamp>-<pid>`. To cancel one, stop its
unit:

```sh
systemctl --user stop hark-<...>.timer             # cancel a time job
systemctl --user stop hark-watch-<...>.service     # cancel an event watcher
```

## Delivery requirements & caveats

Delivery uses terminal messaging, so the recipient must be **logged in on a
terminal** (tty/pts) at delivery time.

- Sending **to yourself** works out of the box: the fallback writes directly to
  your own terminals even if `write(1)` is broken. Note that some terminals
  (e.g. tmux) disable messaging (`mesg n`) — the direct fallback still reaches
  terminals you own.
- Sending **to another user** requires a working, set-gid `tty` `write(1)`
  binary (`-rwxr-sr-x root tty`) and that the recipient has `mesg y`. On systems
  where `write` has lost its set-gid bit, cross-user delivery won't work without
  elevated privileges.
- The recipient's `--user` systemd manager must be running. Time jobs and event
  watchers both live inside it, so if the user logs out before the trigger
  fires, they stop. Enable lingering to survive logout:
  `loginctl enable-linger <user>`.
- `--on-exit` watches an *existing* process and cannot read its exit status (it
  is not a child), so it reports only that the process ended; it also polls
  `/proc` at ~1s, and a recycled PID could in principle mislead it. Use
  `run -- <cmd>` when you need the actual success/failure.

## Build & test

```sh
cargo build --release
cargo test
```

## License

Licensed under the [MIT License](LICENSE). Dependencies are permissively
licensed (MIT / Apache-2.0 / ISC); if you redistribute compiled binaries,
include their license notices as attribution.
