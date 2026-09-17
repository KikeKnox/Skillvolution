#!/bin/sh
# Installs the latest Skillvolution release, then runs `skillvolution setup`, which
# detects the installed AI clients (Claude Code, OpenCode, Devin CLI) and asks which
# ones to configure. Skip client configuration with SKILLVOLUTION_NO_SETUP=1; skip
# the questions (configure every detected client) with SKILLVOLUTION_NO_INPUT=1.
set -eu

installer_url="https://github.com/KikeKnox/Skillvolution/releases/latest/download/skillvolution-installer.sh"
installer=$(mktemp)
trap 'rm -f "$installer"' EXIT

curl --proto '=https' --tlsv1.2 -LsSf "$installer_url" -o "$installer"
sh "$installer"

if [ "${SKILLVOLUTION_NO_SETUP:-0}" = 1 ]; then
    echo "Skipped client setup; run 'skillvolution setup' when ready."
    exit 0
fi

# Under `curl | sh` stdin is the download pipe; reconnect it to the terminal so
# setup can ask which clients to configure. With no terminal at all, setup falls
# back to configuring every detected client.
bin="${SKILLVOLUTION_INSTALL_DIR:-$HOME/.local/bin}/skillvolution"
if [ -r /dev/tty ]; then
    "$bin" setup < /dev/tty
else
    "$bin" setup
fi
