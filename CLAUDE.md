# llama-tray

A GNOME panel control for the `llama-server.service` user unit. No windows, no
GTK — a bare glib main loop serving two D-Bus interfaces.

## Stack

glib 0.22 + gio 0.22 (`v2_80`), Rust edition 2021 (MSRV 1.80). Installed glib is
2.88. `serde_json` parses `/v1/models` and nothing else.

Deliberately not a GTK app: it never draws. Do not add `gtk4` to reach for a
widget — if something needs a window, it belongs in `familiar`, which is the
llama-server *client*.

## Commands

- `./test.sh` — fmt check, clippy with `-D warnings`, then `cargo test
  --all-targets`. This is the gate; run it, not bare `cargo test`.
- `./install.sh` — release build under `~/.local`, plus enabling the user
  service. `./uninstall.sh` reverses it.

Tests need a session bus (the tray opt-out test opens one) but no display.

## Layout

- `server.rs` — the only module that touches anything outside the process:
  systemd on the session bus, the server over a socket, `nvidia-smi` over a
  pipe. Every call carries a timeout. Probes are synchronous on purpose; the
  module header says why.
- `status.rs` — pure. `Snapshot` in, menu rows and icon name out. No I/O.
- `lifetime.rs` — pure. Banks the `/metrics` counters across server restarts,
  since they reset every time the toggle is used. Counter-reset detection and
  the number formatting live here.
- `tray.rs` — `org.kde.StatusNotifierItem` + `com.canonical.dbusmenu`, adapted
  from `stickies/src/tray.rs`. A fix to the dbusmenu marshalling here probably
  applies there too.

## Conventions

- The sibling apps (stickies, brain, familiar, planner, magpie) share this
  layout and these scripts; a pattern established in one is the pattern here.
- Edit files with the Edit tool. Do not rewrite Rust sources through
  `python3 - <<PY` heredocs or `sed -i`.
- Menu wording is Header Capitalization for actions ("Stop Server") and lower
  case for status rows ("qwen3.6-27b · running"), per the GNOME HIG.
