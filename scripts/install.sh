#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: ipod-connection-utility-install [options]

Install the app for the current user. With --apkm, also install host
dependencies and build the Apple Music backend. The default remains an
app-only install; use --sync-only to make that intent explicit.

Options:
  --interactive        Open the terminal wizard (use alone; requires a terminal)
  --basic              Install app + private AAC downloader (no Docker/APKM)
  --apkm PATH          Apple Music APKM bundle; enables advanced wrapper setup
  --sync-only          Install only the library sync app, without backend setup
  --prefix PATH        Install under PATH (default: ~/.local)
  --install-deps       Install missing system packages after one confirmation
  --yes                Accept package changes (implies --install-deps)
  --dry-run            Show the host/package plan without making changes
  --cleanup-legacy     After installing, remove old app files from earlier
                       installs (see ipod-connection-utility-cleanup --scan)
  --remove-old-backend After installing, remove the old advanced (ALAC) Docker
                       backend; sign-in data and music are kept
  -h, --help           Show this help
EOF
}
die() { echo "install: $*" >&2; exit 1; }

if [[ "${1:-}" == --interactive ]]; then
    (($# == 1)) || die '--interactive must be used alone; use the wizard prompts or explicit scripting options'
    wizard_dir=$(cd "${BASH_SOURCE[0]%/*}" && pwd)
    for wizard in "$wizard_dir/install-wizard.sh" "$wizard_dir/ipod-connection-utility-install-wizard"; do
        if [[ -f "$wizard" ]]; then exec bash "$wizard"; fi
    done
    die 'interactive helper missing; extract the complete release archive or use explicit scripting options'
fi

prefix="${HOME:?HOME is required}/.local"
apkm=""
basic=0 sync_only=0 install_deps=0 assume_yes=0 dry_run=0
cleanup_legacy=0 remove_old_backend=0
while (($#)); do
    case "$1" in
        --prefix) (($# >= 2)) || die '--prefix requires a path'; [[ -n "$2" ]] || die '--prefix cannot be empty'; prefix=$2; shift 2 ;;
        --apkm) (($# >= 2)) || die '--apkm requires a path'; [[ -n "$2" ]] || die '--apkm cannot be empty'; apkm=$2; shift 2 ;;
        --basic) basic=1; shift ;;
        --sync-only) sync_only=1; shift ;;
        --install-deps) install_deps=1; shift ;;
        --yes) assume_yes=1; install_deps=1; shift ;;
        --dry-run) dry_run=1; shift ;;
        --cleanup-legacy) cleanup_legacy=1; shift ;;
        --remove-old-backend) remove_old_backend=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
# Guard uses `id -u` (EUID is readonly in bash) so tests can stub it on PATH.
[[ "$(id -u)" != 0 ]] || die 'Run as your normal user, not with sudo; the installer asks for sudo only for package changes.'
(( !basic || (!sync_only && ${#apkm} == 0) )) || die '--basic cannot be combined with --sync-only or --apkm'
(( !remove_old_backend || ${#apkm} == 0 )) || die '--remove-old-backend cannot be combined with --apkm'
case "$prefix" in \~/*) prefix="$HOME/${prefix:2}" ;; esac
case "$apkm" in \~/*) apkm="$HOME/${apkm:2}" ;; esac
[[ "$sync_only" == 0 || -z "$apkm" ]] || die '--sync-only cannot be combined with --apkm'
if [[ "$(uname -s)" == Darwin ]]; then
    for brew_path in /opt/homebrew/bin/brew /usr/local/bin/brew; do
        if [[ -x "$brew_path" ]]; then
            PATH="${brew_path%/*}:$PATH"
            export PATH
            break
        fi
    done
fi
if [[ -n "$apkm" ]]; then
    [[ -f "$apkm" && -r "$apkm" ]] || die "APKM is not a readable file: $apkm"
    sync_only=0
fi

script_dir=$(cd "${BASH_SOURCE[0]%/*}" && pwd)
installer_script="$script_dir/${BASH_SOURCE[0]##*/}"
repo_root=$(cd "$script_dir/.." && pwd)
source_install=0
if [[ -f "$repo_root/Cargo.toml" && -f "$repo_root/scripts/setup-backend.sh" ]]; then
    source_install=1
    app_source=$repo_root
    setup_script=$repo_root/scripts/setup-backend.sh
    downloader_script=$repo_root/scripts/setup-downloader.sh
    deps_script=$repo_root/scripts/install-dependencies.sh
    patch_file=$repo_root/backend/patches/wrapper-portable.patch
    readme_file=$repo_root/README.md
    downloader_file=$repo_root/DOWNLOADER_SETUP.md
    checker_file=$repo_root/tools/check_elf_pages.py
    cleanup_script=$repo_root/scripts/cleanup-legacy.sh
else
    package_root=$(cd "$script_dir/.." && pwd)
    app_source=$package_root/bin/ipod-connection-utility
    setup_script=$script_dir/ipod-connection-utility-setup-backend
    downloader_script=$script_dir/ipod-connection-utility-setup-downloader
    deps_script=$script_dir/ipod-connection-utility-install-dependencies
    patch_file=$package_root/share/ipod-connection-utility/wrapper-portable.patch
    readme_file=$package_root/share/ipod-connection-utility/README.md
    downloader_file=$package_root/share/ipod-connection-utility/DOWNLOADER_SETUP.md
    checker_file=$package_root/share/ipod-connection-utility/check_elf_pages.py
    cleanup_script=$script_dir/ipod-connection-utility-cleanup
    [[ -x "$app_source" ]] || die 'could not find the packaged app binary beside this installer'
fi
[[ -x "$setup_script" ]] || die "backend setup helper is missing: $setup_script"
if ((source_install)) || [[ -n "$apkm" ]]; then
    [[ -x "$deps_script" ]] || die "dependency helper is missing: $deps_script"
fi

if ((dry_run)); then
    if ((basic)); then
        basic_args=(--dry-run)
        (( !install_deps )) || basic_args+=(--install-deps)
        (( !assume_yes )) || basic_args+=(--yes)
        "$downloader_script" "${basic_args[@]}"
    fi
    if [[ -n "$apkm" ]] || ((source_install)); then
        dep_args=(--dry-run)
        if [[ -n "$apkm" ]]; then dep_args+=(--start-docker --source)
        elif ((source_install)); then dep_args+=(--source-only); fi
        (( !install_deps )) || dep_args+=(--install)
        "$deps_script" "${dep_args[@]}"
    else
        echo 'Host dependencies: none required for packaged library sync.'
    fi
    if [[ -n "$apkm" ]]; then
        echo "Backend setup: build profile will be selected from the Docker server when setup runs; APKM: $apkm"
    fi
    if ((source_install)); then
        echo "App build/install: cargo install --path '$repo_root' --locked --root '$prefix' --force"
    else
        echo "App install: copy packaged binary to '$prefix/bin'"
    fi
    if ((cleanup_legacy || remove_old_backend)) && [[ -f "$cleanup_script" ]]; then
        echo 'Leftovers from earlier installs that cleanup would consider:'
        bash "$cleanup_script" --prefix "$prefix" --scan
    fi
    echo 'Dry run complete; no files, packages, Docker services, or backend images were changed.'
    exit 0
fi

dep_args=()
if ((source_install)); then
    if [[ -n "$apkm" ]]; then dep_args+=(--source)
    elif ((install_deps)); then dep_args+=(--source-only); fi
fi
# Starting a system service can require sudo too; keep it behind package/system consent.
if [[ -n "$apkm" ]] && ((install_deps)); then dep_args+=(--start-docker); fi
(( !install_deps )) || dep_args+=(--install)
(( !assume_yes )) || dep_args+=(--yes)
if [[ -n "$apkm" ]] || { ((source_install && install_deps)); }; then
    # Bash 3.2 + nounset treats an empty array expansion as an unset variable.
    if ((${#dep_args[@]})); then "$deps_script" "${dep_args[@]}"
    else "$deps_script"; fi
fi

mkdir -p "$prefix/bin" "$prefix/share/ipod-connection-utility"
installed_paths=()
install_file() {
    local mode="$1" source="$2" destination="$3"
    installed_paths+=("$destination")
    if [[ -e "$destination" && "$source" -ef "$destination" ]]; then return 0; fi
    install -m "$mode" "$source" "$destination"
}
if ((source_install)); then
    PATH="${HOME:?HOME is required}/.cargo/bin:$PATH"
    export PATH
    command -v cargo >/dev/null 2>&1 || die 'Rust 1.89+ is required; rerun with --install-deps or install Rust from https://rustup.rs'
    app_page_flags="${RUSTFLAGS:-}"
    if [[ "$(uname -s)" == Linux && ( "$(uname -m)" == aarch64 || "$(uname -m)" == arm64 ) ]] && \
        command -v getconf >/dev/null 2>&1; then
        host_pages=$(getconf PAGESIZE)
        if [[ "$host_pages" =~ ^[0-9]+$ ]] && ((host_pages > 4096)); then
            app_page_flags="${app_page_flags:+$app_page_flags }-C link-arg=-Wl,-z,max-page-size=65536"
            echo "Building the app for a $host_pages-byte ARM64 Linux host (ELF maximum page size 65536)."
        fi
    fi
    if [[ -n "$app_page_flags" ]]; then
        RUSTFLAGS="$app_page_flags" cargo install --path "$repo_root" --locked --root "$prefix" --force
    else
        cargo install --path "$repo_root" --locked --root "$prefix" --force
    fi
    installed_paths+=("$prefix/bin/ipod-connection-utility")
    install_file 0755 "$deps_script" "$prefix/bin/ipod-connection-utility-install-dependencies"
else
    "$app_source" --version >/dev/null 2>&1 || die "packaged app is not runnable on this OS/CPU: $app_source"
    install_file 0755 "$app_source" "$prefix/bin/ipod-connection-utility"
    if [[ -x "$deps_script" ]]; then
        install_file 0755 "$deps_script" "$prefix/bin/ipod-connection-utility-install-dependencies"
    fi
    "$prefix/bin/ipod-connection-utility" --version >/dev/null 2>&1 || \
        die "installed app is not runnable on this OS/CPU: $prefix/bin/ipod-connection-utility"
fi
install_file 0755 "$setup_script" "$prefix/bin/ipod-connection-utility-setup-backend"
install_file 0755 "$installer_script" "$prefix/bin/ipod-connection-utility-install"
install_file 0644 "$patch_file" "$prefix/share/ipod-connection-utility/wrapper-portable.patch"
install_file 0644 "$readme_file" "$prefix/share/ipod-connection-utility/README.md"
install_file 0644 "$downloader_file" "$prefix/share/ipod-connection-utility/DOWNLOADER_SETUP.md"
if [[ -f "$checker_file" ]]; then
    install_file 0644 "$checker_file" "$prefix/share/ipod-connection-utility/check_elf_pages.py"
fi

if [[ -f "$cleanup_script" ]]; then
    install_file 0755 "$cleanup_script" "$prefix/bin/ipod-connection-utility-cleanup"
fi

if [[ -f "$downloader_script" ]]; then
    install_file 0755 "$downloader_script" "$prefix/bin/ipod-connection-utility-setup-downloader"
elif ((basic)); then
    die "downloader setup helper missing: $downloader_script"
fi

# `icu` is a convenience name, not a reason to replace an unrelated command.
# Only manage the alias in this explicit install prefix; never sweep other
# prefixes or user data while upgrading.
icu_alias="$prefix/bin/icu"
installed_app="$prefix/bin/ipod-connection-utility"
if [[ ! -e "$icu_alias" && ! -L "$icu_alias" ]]; then
    ln -s ipod-connection-utility "$icu_alias"
elif [[ -L "$icu_alias" && "$(readlink "$icu_alias")" == "ipod-connection-utility" ]]; then
    : # Already owned by this installer; retain it across reinstall.
else
    echo "Not creating icu alias: $icu_alias already exists and is not this install's alias." >&2
    icu_alias=
fi

for wizard_source in "$script_dir/install-wizard.sh" "$script_dir/ipod-connection-utility-install-wizard"; do
    if [[ -f "$wizard_source" ]]; then
        install_file 0755 "$wizard_source" "$prefix/bin/ipod-connection-utility-install-wizard"
        break
    fi
done

[[ -z "$icu_alias" ]] || installed_paths+=("$icu_alias")

# Upgrade cleanup inside this prefix: remove files a previous version of this
# installer recorded but this version no longer ships. Only plain files or
# symlinks under bin/ and share/ipod-connection-utility/ are eligible.
manifest="$prefix/share/ipod-connection-utility/installed-files"
new_manifest=$(for path in ${installed_paths[@]+"${installed_paths[@]}"}; do
    printf '%s\n' "${path#"$prefix"/}"
done | sort -u)
if [[ -f "$manifest" && ! -L "$manifest" ]]; then
    while IFS= read -r old; do
        [[ "$old" =~ ^(bin|share/ipod-connection-utility)/[^/]+$ ]] || continue
        [[ "$old" != */.. && "$old" != */. ]] || continue
        grep -qxF -- "$old" <<<"$new_manifest" && continue
        # The alias check above found a bin/icu that is not ours: the user's file.
        [[ "$old" == bin/icu && -z "$icu_alias" ]] && continue
        if [[ -L "$prefix/$old" || -f "$prefix/$old" ]]; then
            rm -f -- "$prefix/$old"
            echo "Removed obsolete file from the previous version: $prefix/$old"
        fi
    done < "$manifest"
fi
printf '%s\n' "$new_manifest" > "$manifest.tmp"
mv -f -- "$manifest.tmp" "$manifest"

if [[ "$(uname -s)" == Linux ]]; then
    applications_dir="${XDG_DATA_HOME:-}"
    if [[ "$applications_dir" != /* ]]; then applications_dir="${HOME:?HOME is required}/.local/share"; fi
    applications_dir="$applications_dir/applications"
    installed_app="$(cd "$prefix/bin" && pwd)/ipod-connection-utility"
    desktop_exec_escape() {
        local value="$1"
        value=${value//\\/\\\\}
        value=${value//\"/\\\"}
        value=${value//\$/\\\$}
        value=${value//\`/\\\`}
        value=${value//%/%%}
        # Exec has two escape layers: argument quoting, then desktop key-file
        # string escaping. Preserve the argument layer through key-file parsing.
        value=${value//\\/\\\\}
        printf '%s' "$value"
    }
    mkdir -p "$applications_dir"
    {
        printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=iPod Connection Utility' 'Comment=Sync music libraries with an iPod' 'Terminal=true'
        # Keep the executable token fixed: some desktop parsers validate that
        # token before expanding %% in it. The installed path is a literal argv
        # item to env, never shell code or an environment assignment.
        printf 'Exec=/usr/bin/env -- "%s"\n' "$(desktop_exec_escape "$installed_app")"
        printf '%s\n' 'Categories=AudioVideo;Utility;' 'StartupNotify=true'
    } > "$applications_dir/ipod-connection-utility.desktop"
    chmod 0644 "$applications_dir/ipod-connection-utility.desktop"
fi

echo "Installed to $prefix/bin."
if ((cleanup_legacy || remove_old_backend)); then
    cleanup_args=(--prefix "$prefix")
    (( !cleanup_legacy )) || cleanup_args+=(--remove-legacy-files)
    (( !remove_old_backend )) || cleanup_args+=(--remove-old-backend)
    bash "$prefix/bin/ipod-connection-utility-cleanup" "${cleanup_args[@]}"
fi
if ((basic)); then
    basic_args=()
    (( !install_deps )) || basic_args+=(--install-deps)
    (( !assume_yes )) || basic_args+=(--yes)
    "$downloader_script" ${basic_args[@]+"${basic_args[@]}"}
elif [[ -z "$apkm" ]]; then
    echo 'The app is ready for library sync. Add Basic AAC with --basic or advanced wrapper setup with --apkm PATH.'
else
    setup_args=(--apkm "$apkm")
    if [[ "$(uname -s)" == Linux && ( "$(uname -m)" == aarch64 || "$(uname -m)" == arm64 ) ]] && ((install_deps)); then
        setup_args+=(--configure-emulation)
    fi
    run_setup() { "$setup_script" "${setup_args[@]}"; }
    setup_needs_sg=0
    if [[ "$(uname -s)" == Linux && -z "${DOCKER_HOST:-}" ]] && command -v docker >/dev/null 2>&1 &&
        ! docker info >/dev/null 2>&1 && command -v sg >/dev/null 2>&1; then
        docker_context=$(docker context show 2>/dev/null || echo default)
        if [[ "$docker_context" == default ]] && {
            ((install_deps)) || id -nG 2>/dev/null | tr ' ' '\n' | grep -qx docker;
        }; then
            setup_needs_sg=1
        fi
    fi
    if ((setup_needs_sg)); then
        shell_quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
        setup_command="exec $(shell_quote "$setup_script")"
        for setup_arg in "${setup_args[@]}"; do setup_command="$setup_command $(shell_quote "$setup_arg")"; done
        sg docker -c "$setup_command"
    else
        run_setup
    fi
fi

echo "For Apple Music downloads, see: $prefix/share/ipod-connection-utility/DOWNLOADER_SETUP.md"
