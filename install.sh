#!/bin/sh
# Installs the latest Skillvolution release, then configures the AI clients found on this machine.
# Skip client configuration with SKILLVOLUTION_NO_SETUP=1.
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

"${SKILLVOLUTION_INSTALL_DIR:-$HOME/.local/bin}/skillvolution" setup
