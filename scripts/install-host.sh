#!/usr/bin/env bash
# install-host.sh - Installs rdp-launcher on Linux host

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

CURRENT_USER="${SUDO_USER:-$USER}"
USER_HOME=$(getent passwd "$CURRENT_USER" | cut -d: -f6)
USER_UID=$(id -u "$CURRENT_USER")
WAYLAND_DISP="${WAYLAND_DISPLAY:-wayland-1}"

echo "===================================================="
echo "  rdp-launcher Linux Host Installer"
echo "===================================================="
echo "Installing for User: $CURRENT_USER (UID: $USER_UID, Home: $USER_HOME)"

# 1. Check/Install Binary
BIN_PATH=""
if [ -f "$ROOT_DIR/rdp-launcher" ]; then
    BIN_PATH="$ROOT_DIR/rdp-launcher"
elif [ -f "$ROOT_DIR/target/release/rdp-launcher" ]; then
    BIN_PATH="$ROOT_DIR/target/release/rdp-launcher"
else
    echo "Release binary not found. Compiling from source via cargo..."
    cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
    BIN_PATH="$ROOT_DIR/target/release/rdp-launcher"
fi

echo "[1/4] Installing binary to /usr/local/bin/rdp-launcher..."
sudo cp -f "$BIN_PATH" /usr/local/bin/rdp-launcher
sudo chmod 755 /usr/local/bin/rdp-launcher

# Set SELinux label if SELinux is active
if command -v restorecon &>/dev/null; then
    sudo restorecon -v /usr/local/bin/rdp-launcher || true
fi

# 2. Install SELinux module if on Fedora / RHEL
if [ -f "$SCRIPT_DIR/virtqemud_custom.pp" ] && command -v semodule &>/dev/null; then
    echo "[2/4] Installing SELinux policy module for virtqemud..."
    sudo semodule -i "$SCRIPT_DIR/virtqemud_custom.pp" || true
else
    echo "[2/4] Skipping SELinux module installation."
fi

# 3. Install systemd service unit
echo "[3/4] Configuring systemd service (rdp-lifecycle.service)..."
SERVICE_TMP=$(mktemp)
sed -e "s|%USER%|$CURRENT_USER|g" \
    -e "s|%HOME%|$USER_HOME|g" \
    -e "s|%UID%|$USER_UID|g" \
    -e "s|%WAYLAND_DISPLAY%|$WAYLAND_DISP|g" \
    "$SCRIPT_DIR/rdp-lifecycle.service" > "$SERVICE_TMP"

sudo cp -f "$SERVICE_TMP" /etc/systemd/system/rdp-lifecycle.service
sudo chmod 644 /etc/systemd/system/rdp-lifecycle.service
rm -f "$SERVICE_TMP"

sudo systemctl daemon-reload
# Ensure it does NOT autostart on boot (it is triggered by libvirt hooks)
sudo systemctl disable rdp-lifecycle.service 2>/dev/null || true

# 4. Prompt / Info for Libvirt Hook
echo "[4/4] Setup complete!"
echo "===================================================="
echo "rdp-launcher has been installed to /usr/local/bin/rdp-launcher."
echo ""
echo "To manage settings from CLI without editing files:"
echo "  rdp-launcher config                     # View all settings"
echo "  rdp-launcher config set host <IP>       # Update VM IP"
echo "  rdp-launcher config set-password        # Save RDP password"
echo ""
echo "To connect rdp-lifecycle to your VM lifecycle, ensure /etc/libvirt/hooks/qemu"
echo "calls 'systemctl start/stop rdp-lifecycle.service' on VM start/stop."
echo "===================================================="
