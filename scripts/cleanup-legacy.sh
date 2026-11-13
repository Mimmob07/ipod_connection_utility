#!/usr/bin/env bash
# Find and remove leftovers from earlier iPod Connection Utility installs.
# Never touches music, iPods, settings, manifests, or Apple sign-in data.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: ipod-connection-utility-cleanup [--prefix PATH] ACTION

Actions (choose at least one):
  --scan                 List leftovers from earlier installs; changes nothing
  --remove-legacy-files  Remove old app-owned files outside the current install:
                         the pre-installer ~/.local/share/ipod-gamdl venv and a
                         cargo-installed ipod_connection_utility binary
  --remove-old-backend   Remove the advanced (ALAC) wrapper backend: its Docker
                         containers and wrapper-v2 images, and source checkouts.
                         Reinstall later with the app's advanced setup (key u).

Kept always: music libraries, iPods, settings, library manifests, the Basic
AAC sign-in (web-auth) and the advanced sign-in data (wrapper-data).
  --prefix PATH          Current install prefix (default ~/.local); never cleaned
  -h, --help             Show this help
EOF
}
die() { echo "cleanup: $*" >&2; exit 1; }

prefix="${HOME:?HOME is required}/.local"
scan=0 legacy=0 backend=0
while (($#)); do
    case "$1" in
        --prefix) (($# >= 2)) && [[ -n "$2" ]] || die '--prefix requires a path'; prefix=$2; shift 2 ;;
        --scan) scan=1; shift ;;
        --remove-legacy-files) legacy=1; shift ;;
        --remove-old-backend) backend=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done
((scan || legacy || backend)) || { usage >&2; exit 2; }
case "$prefix" in \~/*) prefix="$HOME/${prefix:2}" ;; esac

xdg_data_home="${XDG_DATA_HOME:-}"
[[ "$xdg_data_home" == /* ]] || xdg_data_home=""
if [[ "$(uname -s)" == Darwin ]]; then
    data_base="${xdg_data_home:-$HOME/Library/Application Support}"
else
    data_base="${xdg_data_home:-$HOME/.local/share}"
fi
data_home="$data_base/ipod-connection-utility"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
size_of() { du -sh -- "$1" 2>/dev/null | cut -f1 || echo '?'; }
same_dir() { [[ -d "$1" && -d "$2" && "$1" -ef "$2" ]]; }

# --- Legacy app-owned files -------------------------------------------------
legacy_items=()   # paths
legacy_venv="$HOME/.local/share/ipod-gamdl"
if [[ -f "$legacy_venv/pyvenv.cfg" && ! -L "$legacy_venv" ]] &&
    compgen -G "$legacy_venv/lib/python*/site-packages/gamdl" >/dev/null; then
    legacy_items+=("$legacy_venv")
fi
cargo_owned=0
if ! same_dir "$cargo_home/bin" "$prefix/bin"; then
    for meta in "$cargo_home/.crates2.json" "$cargo_home/.crates.toml"; do
        if [[ -f "$meta" ]] && grep -q 'ipod_connection_utility' "$meta"; then cargo_owned=1; fi
    done
    if ((cargo_owned)); then
        for name in ipod_connection_utility ipod-connection-utility; do
            if [[ -f "$cargo_home/bin/$name" && ! -L "$cargo_home/bin/$name" ]]; then
                legacy_items+=("$cargo_home/bin/$name")
            fi
        done
    fi
fi

# --- Old advanced backend -----------------------------------------------------
backend_dirs=()
if [[ -d "$data_home/backends" ]]; then
    for dir in "$data_home/backends"/*/; do
        dir=${dir%/}
        [[ -f "$dir/compose.yaml" && ! -L "$dir" ]] && backend_dirs+=("$dir")
    done
fi
docker_ok=0
backend_images=()
# Only consult Docker when an old backend checkout exists, so Basic/sync-only
# installs on clean machines never invoke Docker at all.
if ((${#backend_dirs[@]})) && command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    docker_ok=1
    while IFS= read -r image; do
        [[ "$image" == wrapper-v2:* ]] && backend_images+=("$image")
    done < <(docker image ls --format '{{.Repository}}:{{.Tag}}' wrapper-v2 2>/dev/null || :)
fi

if ((scan)); then
    found=0
    for item in ${legacy_items[@]+"${legacy_items[@]}"}; do
        printf 'LEGACY\t%s\t%s\n' "$item" "$(size_of "$item")"; found=1
    done
    for dir in ${backend_dirs[@]+"${backend_dirs[@]}"}; do
        printf 'BACKEND\t%s\t%s\n' "$dir" "$(size_of "$dir")"; found=1
    done
    for image in ${backend_images[@]+"${backend_images[@]}"}; do
        printf 'BACKEND\tDocker image %s\t%s\n' "$image" \
            "$(docker image ls --format '{{.Size}}' "$image" 2>/dev/null | head -n1)"; found=1
    done
    if ((${#backend_dirs[@]} && !docker_ok)); then
        printf 'NOTE\tDocker is not reachable; old backend containers/images could not be listed\t-\n'
    fi
    # Report (never delete) other copies that could shadow the new command.
    IFS=: read -r -a path_dirs <<<"${PATH:-}"
    for dir in ${path_dirs[@]+"${path_dirs[@]}"}; do
        [[ -n "$dir" ]] || continue
        same_dir "$dir" "$prefix/bin" && continue
        same_dir "$dir" "$cargo_home/bin" && ((cargo_owned)) && continue
        for name in ipod-connection-utility ipod_connection_utility; do
            [[ -x "$dir/$name" ]] && printf 'NOTE\tOther copy on PATH (not removed): %s\t-\n' "$dir/$name"
        done
    done
    ((found)) || echo 'No leftovers from earlier installs found.'
fi

if ((legacy)); then
    for item in ${legacy_items[@]+"${legacy_items[@]}"}; do
        if [[ "$item" == "$cargo_home/bin/"* ]] && command -v cargo >/dev/null 2>&1; then
            cargo uninstall --root "$cargo_home" ipod_connection_utility >/dev/null 2>&1 || rm -f -- "$item"
            rm -f -- "$item"
        else
            rm -rf -- "$item"
        fi
        echo "Removed old app files: $item"
    done
fi

if ((backend)); then
    if ((${#backend_dirs[@]} || ${#backend_images[@]})); then
        ((docker_ok)) || die 'Docker is not reachable; start Docker (or fix access) so old containers can be removed first. Nothing was deleted.'
    fi
    for dir in ${backend_dirs[@]+"${backend_dirs[@]}"}; do
        # Stop/remove this checkout's containers and network. No -v: keep any volumes.
        (cd "$dir" && docker compose down --remove-orphans >/dev/null 2>&1) ||
            die "could not remove containers for $dir; nothing further was deleted"
    done
    for image in ${backend_images[@]+"${backend_images[@]}"}; do
        if docker image rm "$image" >/dev/null 2>&1; then
            echo "Removed Docker image $image"
        else
            echo "Kept Docker image $image (still used by another container)" >&2
        fi
    done
    for dir in ${backend_dirs[@]+"${backend_dirs[@]}"}; do
        if [[ -d "$dir/data" ]] && [[ -n "$(ls -A "$dir/data" 2>/dev/null)" ]]; then
            echo "Kept $dir: it contains a data/ folder that may hold an Apple session." >&2
            continue
        fi
        rm -rf -- "$dir"
        echo "Removed old backend checkout $dir"
    done
    rmdir "$data_home/backends" 2>/dev/null || :
    rm -f -- "$data_home/backend-profile" "$data_home/backend-page-size"
    echo 'Advanced sign-in data (wrapper-data) was kept. Rerun advanced setup to use ALAC again.'
fi
