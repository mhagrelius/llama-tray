# llama-tray

A panel control for the local `llama-server` user service.

The 5090 cannot hold Qwen3.6-27B and a game at the same time, so the server has
to come down before playing and back up afterwards. This puts that toggle in the
GNOME panel, along with enough numbers to know what the card is doing.

```
▾ (panel icon)
┌──────────────────────────────┐
│ qwen3.6-27b · running        │
│ 93.6 tok/s · 0 active        │
│ VRAM 29.4 / 31.8 GiB         │
├──────────────────────────────┤
│ All time: 9.2M written ·     │
│           3.2M read          │
│ ≈ 6.9M words in 23h 15m      │
│   of thinking                │
├──────────────────────────────┤
│ Stop Server                  │
│ Open Web UI                  │
│ View Logs                    │
├──────────────────────────────┤
│ Quit                         │
└──────────────────────────────┘
```

The icon is filled while the server is up and hollow while it is down, so the
state reads without opening anything.

## Install

```sh
./install.sh     # builds, installs under ~/.local, enables the user service
./uninstall.sh   # reverses it; leaves llama-server alone
./test.sh        # the gate: fmt, clippy -D warnings, tests
```

It runs as `llama-tray.service`, wanted by `graphical-session.target`, so it
comes up at login and goes away with the session.

## Getting the icon back

`Quit` in the menu is a clean exit, which is exactly the case `Restart=on-failure`
declines to act on — so the icon stays gone until something asks for it again.
That something is the **Llama Tray** entry in the app grid: it restarts the unit
and nothing else. Restart rather than start, because it also has to work when
the process is wedged rather than absent.

## How it works

Three sources, none of them polled in the background:

| Row | Source |
| --- | --- |
| state, start/stop | `org.freedesktop.systemd1` on the **session** bus, where the user manager lives |
| throughput, queue | `GET /metrics` on `127.0.0.1:8080` (the server runs with `--metrics`) |
| model name | `GET /v1/models` |
| VRAM | `nvidia-smi --query-gpu=memory.used,memory.total` |
| all-time totals | the same `/metrics` counters, banked across restarts (see below) |

## All-time totals

`/metrics` counters restart at zero with the server process — which this app
makes happen every time you go and play something — so they are banked to
`~/.local/state/llama-tray/lifetime.json` to survive it.

The rule is the one any counter-scraper uses: a counter that went *down* means
a new process is behind it, so the last reading of the old one is final and
gets added to the running total. A server that has gone away banks its numbers
the same way, which is the only chance to do it — the counters die with the
process.

This undercounts on purpose. Sampling only happens while the menu is open, so a
run that starts and ends entirely between two openings contributes whatever was
last seen rather than its true final value. Guessing at the gap would be worse
than being slightly conservative. The rows stay hidden until something has
actually been generated.

Everything is read on `AboutToShow` and then every two seconds for as long as
the menu stays open, so an idle panel costs nothing. A menu left open for two
minutes stops polling, because not every host sends the `closed` event that
would otherwise be the only thing to stop it.

The tray itself is `org.kde.StatusNotifierItem` plus `com.canonical.dbusmenu`,
implemented by hand on the same `gio` D-Bus connection used for systemd —
lifted from `stickies`, and for the same reason: `ksni` would drag in zbus and a
tokio runtime to serve a menu with a dozen rows in it.

`main` watches for the `StatusNotifierWatcher` bus name rather than checking for
it once, so the icon appears whenever GNOME Shell claims it and is rebuilt after
a shell restart.

Requires the `ubuntu-appindicators` extension, which is what owns that name on
Ubuntu. Without it there is no tray to appear in.

## If "Open Web UI" does nothing

Check `mountpoint /run/user/1000/doc` and run `fix-portal`. The default browser
is a flatpak, and when the document portal's FUSE mount drops, every launch
fails inside `bwrap` — after `launch_default_for_uri` has already returned
success, so nothing here can report it. Ctrl-clicking a link anywhere else on
the desktop will be dead at the same time, which is the giveaway.

## Layout

`server.rs` is the only module that touches anything outside the process, and
every call it makes carries a timeout. `status.rs` is pure: snapshot in, menu
rows out. `tray.rs` is transport. Set `LLAMA_TRAY_NO_TRAY=1` to suppress the
icon.
