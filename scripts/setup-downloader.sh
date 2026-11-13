#!/usr/bin/env bash
# Wrapper-free AAC setup. Never probes or configures Docker/backend state.
set -euo pipefail
usage() {
    cat <<'EOF'
Usage: ipod-connection-utility-setup-downloader [options]
Create/update the private per-user gamdl 3.9.1 environment for Basic AAC.
  --install-deps  Allow missing Python/ffmpeg host packages (asks for consent)
  --yes           Accept host package changes (implies --install-deps)
  --dry-run       Show the plan without writes, downloads or service calls
  -h, --help      Show this help
EOF
}
die() { echo "setup-downloader: $*" >&2; exit 1; }
install_deps=0 assume_yes=0 dry_run=0
while (($#)); do
    case "$1" in
        --install-deps) install_deps=1; shift ;;
        --yes) install_deps=1; assume_yes=1; shift ;;
        --dry-run) dry_run=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
script_dir=$(cd "${BASH_SOURCE[0]%/*}" && pwd)
xdg_data_home="${XDG_DATA_HOME:-}"
[[ "$xdg_data_home" == /* ]] || xdg_data_home=""
if [[ "$(uname -s)" == Darwin ]]; then
    data_base="${xdg_data_home:-${HOME:?HOME is required}/Library/Application Support}"
else
    data_base="${xdg_data_home:-${HOME:?HOME is required}/.local/share}"
fi
python_env="$data_base/ipod-connection-utility/gamdl"
if ((install_deps || dry_run)); then
    deps_script="$script_dir/install-dependencies.sh"
    [[ -f "$deps_script" ]] || deps_script="$script_dir/ipod-connection-utility-install-dependencies"
    [[ -f "$deps_script" ]] || die 'dependency helper missing; extract the complete archive'
    args=(--basic)
    (( !install_deps )) || args+=(--install)
    (( !assume_yes )) || args+=(--yes)
    (( !dry_run )) || args+=(--dry-run)
    bash "$deps_script" "${args[@]}"
fi
if ((dry_run)); then
    echo "Basic AAC: create/update private Python environment at $python_env"
    echo 'Install gamdl==3.9.1 mutagen pillow; validate gamdl --help. No wrapper required.'
    exit 0
fi
if ! command -v python3 >/dev/null 2>&1 ||
    ! python3 -c 'import sys; raise SystemExit(sys.version_info < (3, 10))'; then
    die 'Python 3.10+ required; rerun with --install-deps'
fi
python3 -c 'import ensurepip' 2>/dev/null || die 'Python venv support required; rerun with --install-deps'
for tool in ffmpeg ffprobe; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool required; rerun with --install-deps"
done
python3 -m venv "$python_env"
"$python_env/bin/python" -m pip install 'gamdl==3.9.1' mutagen pillow
"$python_env/bin/gamdl" --help >/dev/null
echo "Basic AAC downloader ready: $python_env (sign in locally in the app)."
