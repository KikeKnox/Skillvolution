# Installs the specified or latest Skillvolution release, then runs `skillvolution setup`,
# which detects the installed AI clients (Claude Code, OpenCode, Devin CLI) and asks which
# ones to configure. Skip client configuration with $env:SKILLVOLUTION_NO_SETUP = "1"; skip
# the questions (configure every detected client) with $env:SKILLVOLUTION_NO_INPUT = "1".
#
# Environment variables:
#   SKILLVOLUTION_VERSION     Version to install (e.g. 0.3.0 or v0.3.0); if unset, installs latest.
#   SKILLVOLUTION_NO_SETUP    Set to 1 to skip the setup step after installation.
#   SKILLVOLUTION_NO_INPUT    Set to 1 to configure every detected client without prompting.
#   SKILLVOLUTION_INSTALL_DIR Installation directory; defaults to $HOME\.local\bin.
#
#   powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/KikeKnox/Skillvolution/main/install.ps1 | iex"

$ErrorActionPreference = "Stop"

# Determine installer URL based on version pinning.
if ($env:SKILLVOLUTION_VERSION) {
    $version = $env:SKILLVOLUTION_VERSION -replace "^v", ""
    $installerUrl = "https://github.com/KikeKnox/Skillvolution/releases/download/v$version/skillvolution-installer.ps1"
} else {
    $installerUrl = "https://github.com/KikeKnox/Skillvolution/releases/latest/download/skillvolution-installer.ps1"
}

# Run the release's own installer: it verifies each archive's SHA-256 and adds the
# install directory to PATH, as `dist`'s shell installer does on Unix.
Invoke-Expression (Invoke-RestMethod $installerUrl)

# Verify the binary was installed successfully.
$installDir = if ($env:SKILLVOLUTION_INSTALL_DIR) { $env:SKILLVOLUTION_INSTALL_DIR } else { Join-Path $HOME ".local\bin" }
$bin = Join-Path $installDir "skillvolution.exe"
if (-not (Test-Path $bin -PathType Leaf)) {
    $found = Get-Command skillvolution -ErrorAction SilentlyContinue
    if (-not $found) {
        throw "skillvolution binary not found after installation"
    }
    $bin = $found.Source
}

if ($env:SKILLVOLUTION_NO_SETUP -eq "1") {
    Write-Output "Skipped client setup; run 'skillvolution setup' when ready."
    return
}

# Never `exit` here: under `irm | iex` it would close the user's PowerShell session.
& $bin setup
if ($LASTEXITCODE -ne 0) { throw "skillvolution setup failed with exit code $LASTEXITCODE" }
