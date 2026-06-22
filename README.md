# hark

A CUI (no-GUI) timer notifier. Schedule a text message to be delivered to a
local Linux user at a given time. One-shot, callable many times.

## How it works

`hark` is a single binary with two roles:

1. **Schedule** (what you run): parses *when* + *message* + *recipient*, then
   registers a one-shot **systemd `--user` transient timer** (`systemd-run`)
   that re-invokes the binary later. The timer self-destructs after firing
   (`RemainAfterElapse=no`) and fires punctually (`AccuracySec=1s`).
2. **Deliver** (run by the timer, hidden `deliver` subcommand): sends the text
   to the recipient's terminal via `write(1)`, falling back to writing directly
   to the recipient's TTYs when `write(1)` is unusable.

No root, no daemon, no persistent state — each invocation is independent.

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
```

| Flag | Meaning |
|------|---------|
| `--in <DURATION>` | relative delay: `3 minutes later`, `90s`, `2 hours`, `1 day`; `0s` = now |
| `--at <DATETIME>` | absolute local time: `YYYY-MM-DD HH:MM[:SS]` |
| `-m, --message`   | the message text (required) |
| `-t, --to <USER>` | recipient username (defaults to current user) |

`--in` and `--at` are mutually exclusive. If neither is given, the message is
delivered immediately.

## Inspecting / cancelling scheduled jobs

Each job is a transient systemd user unit named `hark-<timestamp>-<pid>`:

```sh
systemctl --user list-timers 'hark-*'      # pending jobs
systemctl --user stop hark-<...>.timer     # cancel one
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
- The recipient's `--user` systemd manager must be running. If you schedule for
  a user who may log out before delivery, enable lingering:
  `loginctl enable-linger <user>`.

## Build & test

```sh
cargo build --release
cargo test
```
