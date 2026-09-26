#!/bin/sh
# Installs the specified or latest Skillvolution release, then runs `skillvolution setup`,
# which detects the installed AI clients (Claude Code, OpenCode, Devin CLI) and asks which
# ones to configure. Skip client configuration with SKILLVOLUTION_NO_SETUP=1; skip
# the questions (configure every detected client) with SKILLVOLUTION_NO_INPUT=1.
#
# Environment variables:
#   SKILLVOLUTION_VERSION     Version to install (e.g. 0.3.0 or v0.3.0); if unset, installs latest.
#   SKILLVOLUTION_NO_SETUP    Set to 1 to skip the setup step after installation.
#   SKILLVOLUTION_NO_INPUT    Set to 1 to configure every detected client without prompting.
#   SKILLVOLUTION_INSTALL_DIR Installation directory; defaults to $HOME/.local/bin.
#   SKILLVOLUTION_NO_MODIFY_PATH  Set to 1 to skip PATH modifications (handled by cargo-dist installer).
set -eu

# Determine installer URL based on version pinning.
if [ -n "${SKILLVOLUTION_VERSION:-}" ]; then
    version="${SKILLVOLUTION_VERSION#v}"  # Strip leading 'v' if present.
    installer_url="https://github.com/KikeKnox/Skillvolution/releases/download/v${version}/skillvolution-installer.sh"
else
    installer_url="https://github.com/KikeKnox/Skillvolution/releases/latest/download/skillvolution-installer.sh"
fi

installer=$(mktemp)
trap 'rm -f "$installer"' EXIT INT TERM

curl --proto '=https' --tlsv1.2 -LsSf "$installer_url" -o "$installer"
sh "$installer"

# Verify the binary was installed successfully.
bin="${SKILLVOLUTION_INSTALL_DIR:-$HOME/.local/bin}/skillvolution"
if [ ! -x "$bin" ]; then
    bin=$(command -v skillvolution || true)
    if [ -z "$bin" ]; then
        echo "Error: skillvolution binary not found after installation" >&2
        exit 1
    fi
fi

if [ "${SKILLVOLUTION_NO_SETUP:-0}" = 1 ]; then
    echo "Skipped client setup; run 'skillvolution setup' when ready."
    exit 0
fi

# Under `curl | sh` stdin is the download pipe; reconnect it to the terminal so
# setup can ask which clients to configure. With no terminal at all, setup falls
# back to configuring every detected client. Use a subshell to test /dev/tty
# availability; avoids false positives with [ -r /dev/tty ] which can pass
# even without a controlling terminal (e.g., in Docker, setsid, cron).
if (: </dev/tty) 2>/dev/null; then
    "$bin" setup </dev/tty
else
    "$bin" setup
fi
