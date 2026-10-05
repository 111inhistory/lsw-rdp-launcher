#!/usr/bin/env bash
# uninstall-host.sh - Cleanly uninstalls rdp-launcher from Linux host

set -euo pipefail

echo "===================================================="
echo "  rdp-launcher Linux Host Uninstaller"
echo "===================================================="

# 1. Stop and disable service
if systemctl is-active --quiet rdp-lifecycle.service 2>/dev/null; then
    echo "Stopping rdp-lifecycle.service..."
    sudo systemctl stop rdp-lifecycle.service || true
fi

if [ -f /etc/systemd/system/rdp-lifecycle.service ]; then
    echo "Removing /etc/systemd/system/rdp-lifecycle.service..."
    sudo rm -f /etc/systemd/system/rdp-lifecycle.service
    sudo systemctl daemon-reload
fi

# 2. Remove binary
if [ -f /usr/local/bin/rdp-launcher ]; then
    echo "Removing /usr/local/bin/rdp-launcher..."
    sudo rm -f /usr/local/bin/rdp-launcher
fi

# 3. Clean desktop entries (optional prompt)
read -r -p "Do you want to remove generated RemoteApp .desktop entries and icon caches? [y/N] " response
if [[ "$response" =~ ^([yY][eE][sS]|[yY])$ ]]; then
    echo "Removing desktop shortcuts in ~/.local/share/applications/remoteapp-*.desktop..."
    rm -f ~/.local/share/applications/remoteapp-*.desktop
    echo "Removing cached icons in ~/.local/share/icons/hicolor/256x256/apps/remoteapp-*.png..."
    rm -f ~/.local/share/icons/hicolor/256x256/apps/remoteapp-*.png
    echo "Desktop entries cleaned."
fi

echo "===================================================="
echo "rdp-launcher uninstalled successfully."
echo "===================================================="
