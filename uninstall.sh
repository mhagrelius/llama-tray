#!/usr/bin/env bash
#
# Reverses install.sh. Leaves llama-server itself alone.

set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
UNITS="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

systemctl --user disable --now llama-tray.service 2>/dev/null || true
rm -f "$UNITS/llama-tray.service"
rm -f "$PREFIX/bin/llama-tray"
rm -f "$PREFIX"/share/icons/hicolor/symbolic/apps/us.hagreli.LlamaTray*.svg
rm -f "$PREFIX"/share/icons/hicolor/scalable/apps/us.hagreli.LlamaTray.svg
rm -f "$PREFIX/share/applications/us.hagreli.LlamaTray.desktop"

systemctl --user daemon-reload
gtk4-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true

echo "Removed."
