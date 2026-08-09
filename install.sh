#!/usr/bin/env bash
#
# Release build, installed under ~/.local, started now and at every login.

set -euo pipefail

cd "$(dirname "$0")"

PREFIX="${PREFIX:-$HOME/.local}"
ICONS="$PREFIX/share/icons/hicolor/symbolic/apps"
UNITS="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

cargo build --release

install -Dm755 target/release/llama-tray "$PREFIX/bin/llama-tray"
install -Dm644 -t "$ICONS" data/icons/hicolor/symbolic/apps/*.svg
install -Dm644 data/llama-tray.service "$UNITS/llama-tray.service"

# Without this the panel finds no icon by name and draws nothing at all.
gtk4-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null ||
    gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null ||
    true

systemctl --user daemon-reload
systemctl --user enable llama-tray.service
# restart, not `enable --now`: --now is a no-op on an already-running service,
# which would leave the previous binary in the panel after a reinstall.
systemctl --user restart llama-tray.service

echo "Installed. The icon appears in the panel once the shell claims the tray."
