#!/bin/sh
# Install a published tdv binary. No Rust, GitHub login, or sudo required.
set -eu

main() {
    version=${TDV_VERSION:-latest}
    bin_dir=${TDV_BIN_DIR:-"$HOME/.local/bin"}
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --version|--bin-dir)
                [ "$#" -ge 2 ] || { echo "tdv: $1 requires a value" >&2; exit 1; }
                case "$1" in
                    --version) version=$2 ;;
                    --bin-dir) bin_dir=$2 ;;
                esac
                shift 2 ;;
            -h|--help)
                echo 'Usage: sh install.sh [--version v0.1.0] [--bin-dir DIRECTORY]'
                echo 'Default: latest release, installed to ~/.local/bin/tdv'
                echo 'Environment: TDV_VERSION, TDV_BIN_DIR'
                exit 0 ;;
            *) echo "tdv: unknown option: $1" >&2; exit 1 ;;
        esac
    done
    [ -n "$bin_dir" ] || { echo 'tdv: installation directory cannot be empty' >&2; exit 1; }
    case "$version" in
        ''|*[!A-Za-z0-9._-]*) echo 'tdv: invalid release version' >&2; exit 1 ;;
        latest|v[0-9]*) ;;
        *) echo 'tdv: version must be latest or a tag such as v0.1.0' >&2; exit 1 ;;
    esac
    for required in curl tar mktemp install; do
        command -v "$required" >/dev/null 2>&1 || { echo "tdv: missing required command: $required" >&2; exit 1; }
    done
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64|Darwin/aarch64) target=aarch64-apple-darwin ;;
        Darwin/x86_64) target=x86_64-apple-darwin ;;
        Linux/x86_64) target=x86_64-unknown-linux-musl ;;
        Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-musl ;;
        *) echo 'tdv: unsupported platform; download a matching release or build with Cargo' >&2; exit 1 ;;
    esac
    if command -v sha256sum >/dev/null 2>&1; then
        checksum=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        checksum=shasum
    else
        echo 'tdv: sha256sum or shasum is required to verify the download' >&2; exit 1
    fi
    repository=https://github.com/baselsoftwaredev/terminal-diff-viewer
    if [ "$version" = latest ]; then
        base_url=$repository/releases/latest/download
    else
        base_url=$repository/releases/download/$version
    fi
    archive=tdv-$target.tar.gz
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/tdv-install.XXXXXXXX")
    staged_binary=
    trap 'rm -rf "$work_dir"; if [ -n "$staged_binary" ]; then rm -f "$staged_binary"; fi' EXIT
    trap 'exit 1' HUP INT TERM
    echo "Downloading tdv ($target, $version)…"
    curl --fail --silent --show-error --location --proto '=https' --retry 3 --connect-timeout 15 --max-time 180 "$base_url/$archive" -o "$work_dir/$archive"
    curl --fail --silent --show-error --location --proto '=https' --retry 3 --connect-timeout 15 --max-time 180 "$base_url/SHA256SUMS" -o "$work_dir/SHA256SUMS"
    digest=$(awk -v asset="$archive" '$2 == asset { print $1 }' "$work_dir/SHA256SUMS")
    case "$digest" in
        ''|*[!0-9a-fA-F]*) echo 'tdv: missing or invalid release checksum' >&2; exit 1 ;;
    esac
    [ "${#digest}" -eq 64 ] || { echo 'tdv: invalid checksum length' >&2; exit 1; }
    (
        cd "$work_dir"
        if [ "$checksum" = sha256sum ]; then
            printf '%s  %s\n' "$digest" "$archive" | sha256sum -c -
        else
            printf '%s  %s\n' "$digest" "$archive" | shasum -a 256 -c -
        fi
    )
    tar -xzf "$work_dir/$archive" -C "$work_dir" tdv
    [ -f "$work_dir/tdv" ] && [ ! -L "$work_dir/tdv" ] || { echo 'tdv: release does not contain a regular tdv binary' >&2; exit 1; }
    mkdir -p "$bin_dir"
    [ ! -d "$bin_dir/tdv" ] || { echo 'tdv: destination is a directory' >&2; exit 1; }
    staged_binary=$(mktemp "$bin_dir/.tdv-install.XXXXXXXX")
    install -m 755 "$work_dir/tdv" "$staged_binary"
    mv -f "$staged_binary" "$bin_dir/tdv"
    staged_binary=
    echo "Installed: $bin_dir/tdv"
    case ":$PATH:" in
        *:"$bin_dir":*) echo 'Run: tdv --demo' ;;
        *)
            echo "Add $bin_dir to your shell's PATH, then run: tdv --demo"
            if [ "$bin_dir" = "$HOME/.local/bin" ]; then
                echo 'For zsh, add this line to ~/.zshrc: export PATH="$HOME/.local/bin:$PATH"'
            fi ;;
    esac
}

# Keep execution at the end so a truncated piped download cannot start installing.
main "$@"
