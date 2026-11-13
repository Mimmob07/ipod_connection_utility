#!/usr/bin/env bash
set -euo pipefail

readonly UPSTREAM_URL="https://github.com/glomatico/wrapper-v2.git"
readonly UPSTREAM_REVISION="100e0a864e883e03a3ac450a780dd9563fff5271"
readonly BINFMT_IMAGE="tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0"

usage() {
    cat <<'EOF'
Usage: setup-backend.sh --apkm PATH [options]

Build the local Apple Music backend from a user-supplied Apple Music APK/APKM.
Apple binaries and account state are never copied into this repository.

Options:
  --apkm PATH          Apple Music 3.6.0-beta-1109 APK or APKM (required)
  --profile NAME       auto (default), native-amd64, native-arm64, asahi-16k
  --backend-dir PATH   Override the profile-specific source checkout
  --page-size BYTES    Override the detected Docker Linux page size
  --skip-python        Do not create/update the gamdl 3.9.1 virtual environment
  --configure-emulation
                       If ARM cannot run the amd64 build stage, register static
                       QEMU using a privileged container on this Docker server
  -h, --help           Show this help
EOF
}

die() {
    echo "setup-backend: $*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

write_state() {
    local destination="$1" value="$2"
    local temporary="${destination}.tmp.$$"
    printf '%s\n' "$value" > "$temporary"
    mv -f "$temporary" "$destination"
}

script_dir="$(cd "${BASH_SOURCE[0]%/*}" && pwd)"
patch_file=""
for candidate in \
    "$script_dir/../backend/patches/wrapper-portable.patch" \
    "$script_dir/../share/ipod-connection-utility/wrapper-portable.patch"
do
    if [[ -f "$candidate" ]]; then
        patch_file="$candidate"
        break
    fi
done
[[ -n "$patch_file" ]] || die "wrapper-portable.patch was not found beside this installation"
elf_checker=""
for candidate in \
    "$script_dir/../tools/check_elf_pages.py" \
    "$script_dir/../share/ipod-connection-utility/check_elf_pages.py"
do
    if [[ -f "$candidate" ]]; then elf_checker="$candidate"; break; fi
done

xdg_data_home="${XDG_DATA_HOME:-}"
if [[ -n "$xdg_data_home" && "$xdg_data_home" != /* ]]; then
    xdg_data_home=""
fi
if [[ "$(uname -s)" == "Darwin" ]]; then
    data_base="${xdg_data_home:-${HOME:?HOME is required}/Library/Application Support}"
else
    data_base="${xdg_data_home:-${HOME:?HOME is required}/.local/share}"
fi
data_home="$data_base/ipod-connection-utility"

apkm=""
profile="auto"
backend_dir=""
page_size=""
install_python=1
configure_emulation=0
while (($#)); do
    case "$1" in
        --apkm|--profile|--backend-dir|--page-size)
            (($# >= 2)) && [[ -n "$2" ]] || die "$1 requires a value"
            option="$1"
            case "$option" in
                --apkm) apkm="$2" ;;
                --profile) profile="$2" ;;
                --backend-dir) backend_dir="$2" ;;
                --page-size) page_size="$2" ;;
            esac
            shift 2
            ;;
        --skip-python) install_python=0; shift ;;
        --configure-emulation) configure_emulation=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

[[ -f "$apkm" ]] || die "--apkm must name a readable APK or APKM file"
[[ -z "$page_size" || "$page_size" =~ ^[0-9]+$ ]] || \
    die "--page-size must be an integer"
if ((install_python)); then
    need python3
    need ffmpeg
    need ffprobe
    python3 -c 'import sys; raise SystemExit(sys.version_info < (3, 10))' || \
        die "Python 3.10 or newer is required for gamdl 3.9.1; select a compatible python3 on PATH or use --skip-python"
fi
for command in docker git jq unzip install file; do
    need "$command"
done
docker compose version >/dev/null 2>&1 || die "Docker Compose v2 is required"
docker info >/dev/null 2>&1 || die "Docker is not running or is not accessible"

server_arch="$(docker version --format '{{.Server.Arch}}')"
case "$server_arch" in
    amd64|x86_64) server_arch="amd64" ;;
    arm64|aarch64) server_arch="arm64" ;;
    *) die "unsupported Docker server architecture: $server_arch" ;;
esac

probe_page_size() {
    local platform="$1"
    docker run --rm --platform "$platform" debian:13.2 getconf PAGESIZE \
        | tr -d '[:space:]'
}

normalize_page_size() {
    local value="$1"
    [[ "$value" =~ ^[0-9]+$ ]] || return 1
    # Bash 3.2 treats leading-zero arithmetic operands as octal. Strip them
    # before comparisons so values from flags and Docker behave consistently.
    value="$(printf '%s' "$value" | sed 's/^0*//')"
    [[ -n "$value" ]] || value=0
    printf '%s\n' "$value"
}

is_power_of_two() {
    local value="$1"
    ((value > 0)) || return 1
    while ((value > 1)); do
        ((value % 2 == 0)) || return 1
        value=$((value / 2))
    done
}

[[ -n "$page_size" ]] || page_size="$(probe_page_size "linux/$server_arch")"
page_size="$(normalize_page_size "$page_size")" || die "Docker Linux page size is not numeric: $page_size"
if (( ${#page_size} > 5 )) || ((page_size > 65536)); then
    die "Docker Linux page size above 65536 bytes is unsupported: $page_size"
fi
is_power_of_two "$page_size" || die "Docker Linux page size must be a positive power of two: $page_size"
((page_size >= 4096)) || die "Docker Linux page size below 4096 bytes is unsupported: $page_size"

if [[ "$profile" == "auto" ]]; then
    if [[ "$server_arch" == "amd64" ]]; then
        profile="native-amd64"
    else
        if [[ "$page_size" -gt 4096 ]]; then
            profile="asahi-16k"
        else
            profile="native-arm64"
        fi
    fi
fi

case "$profile" in
    native-amd64)
        [[ "$server_arch" == "amd64" ]] || \
            die "native-amd64 requires an amd64 Docker server; this server is $server_arch"
        [[ "$page_size" == "4096" ]] || \
            die "native-amd64 is supported only with 4096-byte Docker Linux pages"
        target_arch="x86_64"
        runtime_platform="linux/amd64"
        worker_mode="native"
        ;;
    native-arm64)
        [[ "$server_arch" == "arm64" ]] || \
            die "native-arm64 requires an ARM64 Docker server; this server is $server_arch"
        [[ "$page_size" == "4096" ]] || \
            die "native-arm64 is supported only with 4096-byte Docker Linux pages; use asahi-16k for large-page ARM hosts"
        target_arch="arm64-v8a"
        runtime_platform="linux/arm64"
        worker_mode="native"
        ;;
    asahi-16k)
        [[ "$server_arch" == "arm64" ]] || \
            die "asahi-16k requires an ARM64 Docker server; this server is $server_arch"
        target_arch="x86_64"
        runtime_platform="linux/arm64"
        worker_mode="qemu_x86_64"
        if ((page_size <= 4096)); then
            die "asahi-16k requires a Docker Linux page size greater than 4096"
        fi
        ;;
    *) die "unknown profile '$profile'; use auto, native-amd64, native-arm64, or asahi-16k" ;;
esac

if [[ "$profile" == asahi-16k ]]; then
    need python3
    [[ -n "$elf_checker" ]] || die "check_elf_pages.py is missing; reinstall the complete app package"
fi

# The NDK worker stage is always amd64. Docker Desktop supplies emulation, but
# native ARM Docker engines may need a static binfmt registration after reboot.
if ! docker run --rm --platform linux/amd64 debian:13.2 /bin/true >/dev/null 2>&1; then
    if [[ "$server_arch" == arm64 ]] && ((configure_emulation)); then
        echo "Registering static amd64 build emulation on the selected Docker server."
        docker run --privileged --rm "$BINFMT_IMAGE" --install amd64 || \
            die "static amd64 emulation registration failed"
        docker run --rm --platform linux/amd64 debian:13.2 /bin/true >/dev/null 2>&1 || \
            die "Docker still cannot execute the amd64 NDK build stage after static emulation registration"
    else
        die "Docker cannot execute the amd64 NDK build stage. On ARM Linux, rerun with --configure-emulation to register static QEMU on this Docker server"
    fi
fi
if ! docker run --rm --platform "$runtime_platform" debian:13.2 /bin/true >/dev/null 2>&1; then
    die "Docker cannot execute the selected runtime platform $runtime_platform"
fi

[[ -n "$backend_dir" ]] || backend_dir="$data_home/backends/$profile"
session_dir="$data_home/wrapper-data/$profile"
mkdir -p "$data_home/backends" "$session_dir"

if [[ ! -e "$backend_dir" ]]; then
    mkdir -p "$(dirname "$backend_dir")"
    git clone "$UPSTREAM_URL" "$backend_dir"
    git -C "$backend_dir" checkout --detach "$UPSTREAM_REVISION"
elif [[ ! -d "$backend_dir/.git" ]]; then
    die "backend path exists but is not a Git checkout: $backend_dir"
fi

actual_revision="$(git -C "$backend_dir" rev-parse HEAD)"
[[ "$actual_revision" == "$UPSTREAM_REVISION" ]] || \
    die "backend checkout is at $actual_revision, expected $UPSTREAM_REVISION"

if git -C "$backend_dir" apply --reverse --check "$patch_file" >/dev/null 2>&1; then
    echo "Portable backend patch is already applied."
else
    [[ -z "$(git -C "$backend_dir" status --porcelain)" ]] || \
        die "backend checkout has unrelated changes; move it aside or choose --backend-dir"
    git -C "$backend_dir" apply "$patch_file"
fi

existing_profile=""
[[ -f "$backend_dir/.ipod-profile" ]] && existing_profile="$(<"$backend_dir/.ipod-profile")"
[[ -z "$existing_profile" || "$existing_profile" == "$profile" ]] || \
    die "checkout was staged for $existing_profile; use a separate --backend-dir"

bash "$backend_dir/tools/extract-libs.sh" --bundle "$apkm" --arch "$target_arch"
bash "$backend_dir/tools/stage-system.sh" --arch "$target_arch"

case "$target_arch" in
    x86_64) expected_machine="x86-64" ;;
    arm64-v8a) expected_machine="ARM aarch64" ;;
esac
for staged in \
    "$backend_dir/rootfs/system/bin/linker64" \
    "$backend_dir/rootfs/system/lib64/libc++_shared.so"
do
    file "$staged" | grep -q "$expected_machine" || \
        die "staged file has the wrong architecture: $staged"
done
printf '%s\n' "$profile" > "$backend_dir/.ipod-profile"

export TARGET_ARCH="$target_arch"
export RUNTIME_PLATFORM="$runtime_platform"
export WRAPPER_WORKER_MODE="$worker_mode"
export WRAPPER_IMAGE_TAG="$profile"
export WRAPPER_HOST_PAGE_SIZE="$page_size"
export WRAPPER_DATA_DIR="$session_dir"
export COMPOSE_PROJECT_NAME="ipod-wrapper-$profile"

build_log="$data_home/backend-build-$profile.log"
(
    cd "$backend_dir"
    docker compose build wrapper
) 2>&1 | tee "$build_log"

image_arch="$(docker image inspect "wrapper-v2:$profile" --format '{{.Architecture}}')"
expected_image_arch="${runtime_platform#linux/}"
[[ "$image_arch" == "$expected_image_arch" ]] || \
    die "built image architecture is $image_arch, expected $expected_image_arch"

inspection_dir="$(mktemp -d)"
inspection_container="$(docker create --platform "$runtime_platform" "wrapper-v2:$profile")"
cleanup_inspection() {
    docker rm -f "$inspection_container" >/dev/null 2>&1 || true
    rm -rf "$inspection_dir"
}
trap cleanup_inspection EXIT
docker cp "$inspection_container:/app/wrapperd" "$inspection_dir/wrapperd"
docker cp "$inspection_container:/app/rootfs/system/bin/main" "$inspection_dir/main"
if [[ "$worker_mode" == "native" ]]; then
    docker cp "$inspection_container:/app/wrapper" "$inspection_dir/wrapper"
else
    docker cp "$inspection_container:/app/wrapper-x86" "$inspection_dir/wrapper"
fi
case "$runtime_platform" in
    linux/amd64) runtime_machine="x86-64" ;;
    linux/arm64) runtime_machine="ARM aarch64" ;;
esac
file "$inspection_dir/wrapperd" | grep -q "$runtime_machine" || \
    die "wrapperd does not match runtime $runtime_platform"
file "$inspection_dir/main" | grep -q "$expected_machine" || \
    die "Android worker does not match $target_arch"
file "$inspection_dir/wrapper" | grep -q "$expected_machine" || \
    die "Linux worker launcher does not match $target_arch"
if [[ "$profile" == asahi-16k ]]; then
    docker cp "$inspection_container:/usr/local/bin/qemu-x86_64-static" "$inspection_dir/qemu"
    python3 "$elf_checker" --page-size "$page_size" --machine aarch64 \
        "$inspection_dir/wrapperd" "$inspection_dir/qemu" \
        > "$data_home/backend-elf-check-$profile.log" || \
        die "backend native executables are incompatible with $page_size-byte Docker pages; see $data_home/backend-elf-check-$profile.log"
fi
cleanup_inspection
trap - EXIT

if [[ "$worker_mode" == "native" ]]; then
    docker run --rm --platform "$runtime_platform" --entrypoint /bin/sh \
        "wrapper-v2:$profile" -c \
        'test -x /app/wrapper && test ! -e /usr/local/bin/qemu-x86_64-static'
else
    docker run --rm --platform "$runtime_platform" --entrypoint /bin/sh \
        "wrapper-v2:$profile" -c \
        'test -x /app/wrapper && test -x /app/wrapper-x86 && test -x /usr/local/bin/qemu-x86_64-static'
fi

if ((install_python)); then
    python_env="$data_home/gamdl"
    python3 -m venv "$python_env"
    "$python_env/bin/python" -m pip install --upgrade pip
    "$python_env/bin/python" -m pip install 'gamdl==3.9.1' mutagen pillow
    "$python_env/bin/gamdl" --help >/dev/null
    case "$(uname -s):$(uname -m)" in
        Linux:aarch64|Linux:arm64)
            native_page_size=$(getconf PAGESIZE)
            if ((native_page_size > 4096)); then
                [[ -n "$elf_checker" ]] || die "check_elf_pages.py is missing; reinstall the complete app package"
                if "$python_env/bin/python" - "$elf_checker" "$native_page_size" "$python_env" \
                    > "$data_home/python-elf-check.log" <<'PY'
import sys
from pathlib import Path

sys.path.insert(0, str(Path(sys.argv[1]).resolve().parent))
from check_elf_pages import EM_AARCH64, check_image, parse_elf

page_size = int(sys.argv[2])
count = 0
failed = False
for path in sorted(Path(sys.argv[3]).rglob("*")):
    if ".so" not in path.name or not path.is_file():
        continue
    data = path.read_bytes()
    if not data.startswith(b"\x7fELF"):
        continue
    count += 1
    try:
        errors = check_image(parse_elf(data), page_size, EM_AARCH64)
    except ValueError as error:
        errors = [str(error)]
    for error in errors:
        print(f"FAIL {path}: {error}")
        failed = True
print(f"Checked {count} Python native libraries against {page_size}-byte host pages")
raise SystemExit(1 if failed else 0)
PY
                then
                    echo "Python native libraries support $native_page_size-byte host pages."
                else
                    die "Python native libraries do not match this host's page size; see $data_home/python-elf-check.log"
                fi
            fi
            ;;
    esac
fi

resolved_backend_dir="$(cd "$backend_dir" && pwd -P)"
write_state "$data_home/backend-dir-$profile" "$resolved_backend_dir"
write_state "$data_home/backend-page-size" "$page_size"
write_state "$data_home/backend-profile" "$profile"

echo
echo "Backend ready: wrapper-v2:$profile"
echo "Source: $backend_dir"
echo "Session data: $session_dir"
echo "Build log: $build_log"
echo "Open the Downloader tab to start it."
