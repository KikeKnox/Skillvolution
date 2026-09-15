#!/usr/bin/env bash
set -euo pipefail

usage() {
    printf '%s\n' 'Usage: bash launch.sh --project PATH [options]' \
        '  --project PATH       Existing project to configure (required)' \
        '  --client CLIENT      both (default), opencode, or claude-code' \
        '  --bin-dir PATH       Binary directory (default: $HOME/.local/bin)' \
        '  --db PATH            Database (default: $XDG_DATA_HOME/skillvolution/vault.sqlite3)' \
        '  --install-rust       Opt in to downloading/installing rustup if Rust is missing' \
        '  --help               Show this help without building or installing anything'
}
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
project=''
client=both
bin_dir="${HOME}/.local/bin"
db="${XDG_DATA_HOME:-${HOME}/.local/share}/skillvolution/vault.sqlite3"
install_rust=false
help=false
while (($#)); do
    case "$1" in
        --project|--client|--bin-dir|--db)
            (($# >= 2)) && [[ -n "$2" && "$2" != --* ]] || fail "$1 requires a value"
            case "$1" in
                --project) project=$2 ;;
                --client) client=$2 ;;
                --bin-dir) bin_dir=$2 ;;
                --db) db=$2 ;;
            esac
            shift 2 ;;
        --install-rust) install_rust=true; shift ;;
        --help|-h) help=true; shift ;;
        *) fail "Unknown option: $1" ;;
    esac
done
case "$client" in both|opencode|claude-code) ;; *) fail "Invalid client: $client" ;; esac
if "$help"; then usage; exit 0; fi
[[ -n "$project" ]] || fail '--project is required; use --help'
native_path() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}
check_no_links() {
    local path=$1
    while [[ "$path" != / && "$path" != . && -n "$path" ]]; do
        [[ ! -L "$path" ]] || fail "Symlink refused: $path"
        local parent
        parent=$(dirname -- "$path")
        [[ "$parent" != "$path" ]] || break
        path=$parent
    done
}
if command -v cygpath >/dev/null 2>&1; then
    project=$(cygpath -m "$project")
    bin_dir=$(cygpath -m "$bin_dir")
    db=$(cygpath -m "$db")
fi
[[ -d "$project" ]] || fail "Project must already exist: $project"
for path in "$project" "$bin_dir" "$db"; do check_no_links "$path"; done
[[ ! -e "$db" || -f "$db" ]] || fail "Database is not a regular file: $db"
[[ ! -e "$bin_dir" || -d "$bin_dir" ]] || fail "Binary directory is not a directory: $bin_dir"
project=$(cd -- "$project" && pwd -P)
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
[[ -f "$source_dir/Cargo.lock" ]] || fail 'Cargo.lock is required for a reproducible build'
if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    "$install_rust" || fail 'Rust is required. Install Rust and a C build toolchain, or opt in with --install-rust (Linux only).'
    [[ $(uname -s) == Linux ]] || fail '--install-rust is supported only on Linux; install Rust manually on this host'
    command -v curl >/dev/null 2>&1 || fail 'curl is required for --install-rust'
    printf '%s\n' 'Downloading rustup from https://sh.rustup.rs (explicit --install-rust opt-in).' >&2
    rustup_script=$(mktemp)
    trap 'rm -f -- "${rustup_script:-}" "${staged:-}"' EXIT
    curl --proto '=https' --tlsv1.2 --fail --show-error --location https://sh.rustup.rs -o "$rustup_script"
    sh "$rustup_script" -y --profile minimal --no-modify-path
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
fi
command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1 || fail 'Rust installation did not provide cargo and rustc'
build_dir=${CARGO_TARGET_DIR:-$source_dir/target}
[[ "$build_dir" == /* || "$build_dir" == ?:* ]] || build_dir="$PWD/$build_dir"
build_dir=$(native_path "$build_dir")
printf '%s\n' 'Building release binary with Cargo.lock; Cargo may download the locked dependencies.' >&2
cargo build --release --locked --manifest-path "$(native_path "$source_dir/Cargo.toml")" --target-dir "$build_dir"
binary_name=skillvolution
case $(uname -s) in MINGW*|MSYS*|CYGWIN*) binary_name=skillvolution.exe ;; esac
built="$build_dir/release/$binary_name"
[[ -f "$built" ]] || fail "Build did not produce $built"
mkdir -p -- "$bin_dir" "$(dirname -- "$db")"
bin_dir=$(cd -- "$bin_dir" && pwd -P)
db="$(cd -- "$(dirname -- "$db")" && pwd -P)/$(basename -- "$db")"
binary="$bin_dir/$binary_name"
check_no_links "$binary"
[[ ! -e "$binary" || -f "$binary" ]] || fail "Binary target is not a regular file: $binary"
if [[ ! -f "$binary" ]] || ! cmp -s -- "$built" "$binary"; then
    staged=$(mktemp "$bin_dir/.skillvolution-install.XXXXXX")
    trap 'rm -f -- "${rustup_script:-}" "${staged:-}"' EXIT
    install -m 755 -- "$built" "$staged"
    mv -f -- "$staged" "$binary"
fi
"$binary" --db "$(native_path "$db")" init
"$binary" setup --project "$(native_path "$project")" --client "$client" --bin "$(native_path "$binary")" --db "$(native_path "$db")"
printf 'Installed: %s\nDatabase: %s\nConfigured project: %s (%s)\n' "$binary" "$db" "$project" "$client"
printf '%s\n' 'Restart your client and review its workspace/MCP permissions before connecting.' 'Evolution review is instruction-driven, not a guaranteed lifecycle hook.'
