#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bash_path=$BASH
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
# Every mocked Linux dependency run uses a literal distro fixture, not the host.
printf 'ID=arch\n' > "$tmp/os-release"
printf 'ID=alarm\n' > "$tmp/alarm-os-release"
export IPOD_OS_RELEASE_FILE="$tmp/os-release"
"$bash_path" "$repo_root/tools/test_dependency_fixtures.sh"
export XDG_DATA_HOME="$tmp/xdg data"
: > "$tmp/git-calls"
: > "$tmp/docker-calls"
: > "$tmp/input.apkm"

cat > "$tmp/bin/docker" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$IPOD_TEST_DOCKER_LOG"
case "$1" in
    compose) exit 0 ;;
    info) exit 0 ;;
    version) printf '%s\n' "$IPOD_TEST_SERVER_ARCH" ;;
    run)
        case "$*" in
            *"getconf PAGESIZE"*) printf '%s\n' "$IPOD_TEST_PAGE_SIZE" ;;
            *) exit 0 ;;
        esac
        ;;
    *) echo "unexpected docker command: $*" >&2; exit 90 ;;
esac
EOF
cat > "$tmp/bin/git" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$IPOD_TEST_GIT_LOG"
exit 73
EOF
cat > "$tmp/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$IPOD_TEST_CARGO_LOG"
EOF
chmod +x "$tmp/bin/docker" "$tmp/bin/git" "$tmp/bin/cargo"

install_prefix="$tmp/install prefix"
PATH="$tmp/bin:$PATH" HOME="$tmp/home" IPOD_TEST_CARGO_LOG="$tmp/cargo-calls" \
    bash "$repo_root/scripts/install.sh" --prefix "$install_prefix" \
    > "$tmp/install-output"
[[ -x "$install_prefix/bin/ipod-connection-utility-setup-backend" ]]
cmp "$repo_root/backend/patches/wrapper-portable.patch" \
    "$install_prefix/share/ipod-connection-utility/wrapper-portable.patch"
cmp "$repo_root/README.md" "$install_prefix/share/ipod-connection-utility/README.md"
grep -F -- "--root $install_prefix" "$tmp/cargo-calls" >/dev/null
[[ -x "$install_prefix/bin/ipod-connection-utility-install" ]]
[[ -x "$install_prefix/bin/ipod-connection-utility-install-dependencies" ]]
cmp "$repo_root/tools/check_elf_pages.py" \
    "$install_prefix/share/ipod-connection-utility/check_elf_pages.py"

if bash "$repo_root/scripts/install.sh" --prefix "" > "$tmp/install-output" 2>&1; then
    echo "installer unexpectedly accepted an empty prefix" >&2
    exit 1
fi

# These programs are only preflight dependencies in the mocked setup runs.
# Stub them so the test does not depend on optional packages on macOS runners.
for command in jq unzip file ffmpeg ffprobe; do
    cat > "$tmp/bin/$command" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$tmp/bin/$command"
done

cat > "$tmp/bin/python3" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
chmod +x "$tmp/bin/python3"
: > "$tmp/docker-calls"
if PATH="$tmp/bin:$PATH" HOME="$tmp/home" \
    IPOD_TEST_SERVER_ARCH=amd64 IPOD_TEST_PAGE_SIZE=4096 \
    IPOD_TEST_GIT_LOG="$tmp/git-calls" IPOD_TEST_DOCKER_LOG="$tmp/docker-calls" \
    bash "$repo_root/scripts/setup-backend.sh" --apkm "$tmp/input.apkm" \
    > "$tmp/output" 2>&1; then
    echo "setup unexpectedly accepted an unsupported Python interpreter" >&2
    exit 1
fi
grep -F "Python 3.10 or newer is required" "$tmp/output" >/dev/null
[[ ! -s "$tmp/docker-calls" ]] || {
    echo "setup began Docker work before checking Python support" >&2
    exit 1
}

run_until_clone() {
    local arch="$1" pages="$2" expected_profile="$3"
    : > "$tmp/git-calls"
    : > "$tmp/docker-calls"
    if PATH="$tmp/bin:$PATH" \
        HOME="$tmp/home" \
        IPOD_TEST_SERVER_ARCH="$arch" \
        IPOD_TEST_PAGE_SIZE="$pages" \
        IPOD_TEST_GIT_LOG="$tmp/git-calls" \
        IPOD_TEST_DOCKER_LOG="$tmp/docker-calls" \
        bash "$repo_root/scripts/setup-backend.sh" --apkm "$tmp/input.apkm" --skip-python \
        > "$tmp/output" 2>&1; then
        echo "setup unexpectedly got past the mocked clone" >&2
        return 1
    fi
    grep -F "backends/$expected_profile" "$tmp/git-calls" >/dev/null || {
        cat "$tmp/output" >&2
        echo "profile $expected_profile was not selected for $arch/$pages" >&2
        return 1
    }
    grep -F -- "--platform linux/$arch" "$tmp/docker-calls" >/dev/null || {
        echo "page size was not probed on Docker server architecture $arch" >&2
        return 1
    }
}

run_xdg_fallback() {
    local xdg_value="$1" home="$tmp/home with spaces"
    : > "$tmp/git-calls"
    if PATH="$tmp/bin:$PATH" HOME="$home" XDG_DATA_HOME="$xdg_value" \
        IPOD_TEST_SERVER_ARCH=amd64 IPOD_TEST_PAGE_SIZE=4096 \
        IPOD_TEST_GIT_LOG="$tmp/git-calls" IPOD_TEST_DOCKER_LOG="$tmp/docker-calls" \
        bash "$repo_root/scripts/setup-backend.sh" --apkm "$tmp/input.apkm" \
        --skip-python > "$tmp/output" 2>&1; then
        echo "setup unexpectedly got past the mocked clone" >&2
        return 1
    fi
    if [[ "$(uname -s)" == "Darwin" ]]; then
        expected="$home/Library/Application Support/ipod-connection-utility/backends/native-amd64"
    else
        expected="$home/.local/share/ipod-connection-utility/backends/native-amd64"
    fi
    grep -F "$expected" "$tmp/git-calls" >/dev/null || {
        cat "$tmp/output" >&2
        echo "setup did not use the HOME fallback for XDG_DATA_HOME='$xdg_value'" >&2
        return 1
    }
}

run_rejected() {
    local arch="$1" pages="$2" profile="$3" expected="$4"
    : > "$tmp/git-calls"
    if PATH="$tmp/bin:$PATH" \
        HOME="$tmp/home" \
        IPOD_TEST_SERVER_ARCH="$arch" \
        IPOD_TEST_PAGE_SIZE="$pages" \
        IPOD_TEST_GIT_LOG="$tmp/git-calls" \
        IPOD_TEST_DOCKER_LOG="$tmp/docker-calls" \
        bash "$repo_root/scripts/setup-backend.sh" --apkm "$tmp/input.apkm" \
        --profile "$profile" --page-size "$pages" --skip-python > "$tmp/output" 2>&1; then
        echo "setup unexpectedly accepted $profile on $arch/$pages" >&2
        return 1
    fi
    grep -F "$expected" "$tmp/output" >/dev/null || {
        cat "$tmp/output" >&2
        echo "missing expected rejection: $expected" >&2
        return 1
    }
    [[ ! -s "$tmp/git-calls" ]] || {
        echo "setup reached Git after an invalid profile was rejected" >&2
        return 1
    }
}

run_until_clone amd64 4096 native-amd64
run_until_clone arm64 4096 native-arm64
run_until_clone arm64 16384 asahi-16k
run_xdg_fallback ""
run_xdg_fallback relative/app-data

custom_backend="$tmp/custom backend parent/deep/backend"
: > "$tmp/git-calls"
if PATH="$tmp/bin:$PATH" HOME="$tmp/home" \
    IPOD_TEST_SERVER_ARCH=amd64 IPOD_TEST_PAGE_SIZE=4096 \
    IPOD_TEST_GIT_LOG="$tmp/git-calls" IPOD_TEST_DOCKER_LOG="$tmp/docker-calls" \
    bash "$repo_root/scripts/setup-backend.sh" --apkm "$tmp/input.apkm" \
    --skip-python --backend-dir "$custom_backend" > "$tmp/output" 2>&1; then
    echo "setup unexpectedly got past the mocked custom checkout clone" >&2
    exit 1
fi
grep -F "$custom_backend" "$tmp/git-calls" >/dev/null
[[ -d "$tmp/custom backend parent/deep" ]]

run_rejected arm64 16384 native-arm64 "supported only with 4096-byte"
run_rejected amd64 4096 native-arm64 "requires an ARM64 Docker server"
run_rejected amd64 16384 native-amd64 "supported only with 4096-byte"
run_rejected arm64 16383 asahi-16k "positive power of two"
run_rejected arm64 999999999999999999999 asahi-16k "above 65536 bytes"

# Dry run must not call Docker, package managers, or services.
dry_bin="$tmp/dry-bin"
mkdir -p "$dry_bin"
cat > "$dry_bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo Linux ;; -m) echo aarch64 ;; esac
EOF
cat > "$dry_bin/getconf" <<'EOF'
#!/bin/sh
echo 16384
EOF
cat > "$dry_bin/pacman" <<'EOF'
#!/bin/sh
echo "$*" >> "$IPOD_TEST_PACKAGE_LOG"
EOF
chmod +x "$dry_bin"/*
: > "$tmp/package-calls"
PATH="$dry_bin" IPOD_TEST_PACKAGE_LOG="$tmp/package-calls" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run --install --start-docker \
    > "$tmp/dry-output"
grep -F 'Host: Linux aarch64' "$tmp/dry-output" >/dev/null
grep -F 'Host page size: 16384 bytes' "$tmp/dry-output" >/dev/null
grep -F 'Docker server profile: pending' "$tmp/dry-output" >/dev/null
grep -F 'sudo pacman -S --needed docker docker-compose docker-buildx git' "$tmp/dry-output" >/dev/null
[[ ! -s "$tmp/package-calls" ]]

# Rust alone is not enough for source builds: missing C linker should plan
# build tools without pulling in Docker or backend packages.
source_bin="$tmp/source-bin"
mkdir -p "$source_bin"
cat > "$source_bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; esac
EOF
cat > "$source_bin/cargo" <<'EOF'
#!/bin/sh
echo 'cargo 1.90.0'
EOF
cat > "$source_bin/pacman" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod +x "$source_bin/"*
ln -s "$(command -v sed)" "$source_bin/sed"
PATH="$source_bin" "$bash_path" "$repo_root/scripts/install-dependencies.sh" \
    --source-only --dry-run --install > "$tmp/source-dry-output"
grep -F 'Missing: C compiler/linker' "$tmp/source-dry-output" >/dev/null
grep -F 'sudo pacman -S --needed base-devel' "$tmp/source-dry-output" >/dev/null
if grep -F docker "$tmp/source-dry-output" >/dev/null; then
    echo 'source-only prerequisite plan unexpectedly included Docker' >&2
    exit 1
fi

# A mocked pacman install makes its prerequisites appear, proving the helper
# clears stale preflight failures and rechecks the installed command set.
install_bin="$tmp/install-bin"
mkdir -p "$install_bin"
cat > "$install_bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo Linux ;; -m) echo aarch64 ;; esac
EOF
cat > "$install_bin/sudo" <<'EOF'
#!/bin/sh
printf 'sudo %s\n' "$*" >> "$IPOD_TEST_PACKAGE_LOG"
[ "$1" = -v ] && exit 0
exec "$@"
EOF
cat > "$install_bin/pacman" <<'EOF'
#!/bin/sh
printf 'pacman %s\n' "$*" >> "$IPOD_TEST_PACKAGE_LOG"
for name in docker git jq unzip file python3 ffmpeg ffprobe; do
    case "$name" in
        docker)
            cat > "$IPOD_TEST_MOCK_BIN/docker" <<'DOCKER'
#!/bin/sh
case "$*" in 'compose version'|info) exit 0 ;; *) exit 0 ;; esac
DOCKER
            ;;
        python3)
            cat > "$IPOD_TEST_MOCK_BIN/python3" <<'PYTHON'
#!/bin/sh
exit 0
PYTHON
            ;;
        *) printf '#!/bin/sh\nexit 0\n' > "$IPOD_TEST_MOCK_BIN/$name" ;;
    esac
    /bin/chmod +x "$IPOD_TEST_MOCK_BIN/$name"
done
EOF
chmod +x "$install_bin/"*
ln -s "$(command -v cat)" "$install_bin/cat"
: > "$tmp/package-calls"
if PATH="$install_bin" IPOD_TEST_PACKAGE_LOG="$tmp/package-calls" \
    IPOD_OS_RELEASE_FILE="$tmp/alarm-os-release" \
    IPOD_TEST_MOCK_BIN="$install_bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --install --yes \
    > "$tmp/package-install-output" 2>&1; then
    :
else
    cat "$tmp/package-install-output" >&2
    exit 1
fi
grep -F 'pacman -S --needed --noconfirm' "$tmp/package-calls" >/dev/null
if grep -F -- '-Sy' "$tmp/package-calls" >/dev/null; then
    echo 'package installer unexpectedly requested a partial pacman upgrade' >&2
    exit 1
fi

# Fresh rootful Docker socket permissions get one explicit group grant, then
# the helper immediately checks Docker through sg rather than waiting for it.
group_bin="$tmp/group-bin"
mkdir -p "$group_bin"
cat > "$group_bin/docker" <<'EOF'
#!/bin/sh
case "$*" in
    'context show') echo default ;;
    'compose version') exit 0 ;;
    info) exit 1 ;;
    *) exit 0 ;;
esac
EOF
cat > "$group_bin/sudo" <<'EOF'
#!/bin/sh
printf 'sudo %s\n' "$*" >> "$IPOD_TEST_GROUP_LOG"
exit 0
EOF
cat > "$group_bin/sg" <<'EOF'
#!/bin/sh
printf 'sg %s\n' "$*" >> "$IPOD_TEST_GROUP_LOG"
case "$*" in
    *'id -nG'*) echo 'test-user docker' ;;
    *'docker compose version && docker info'*) exit 0 ;;
    *) exit 90 ;;
esac
EOF
cat > "$group_bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; esac
EOF
chmod +x "$group_bin/"*
# Only explicitly selected utilities are visible; package/service commands
# cannot fall through to the host even when an optional dependency is absent.
for name in git jq unzip file python3 ffmpeg ffprobe pacman; do
    printf '#!/bin/sh\nexit 0\n' > "$group_bin/$name"
    chmod +x "$group_bin/$name"
done
for name in tr grep cat; do
    ln -s "$(command -v "$name")" "$group_bin/$name"
done
python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])' \
    "$tmp/private-docker.sock"
chmod 000 "$tmp/private-docker.sock"
: > "$tmp/group-calls"
PATH="$group_bin" IPOD_TEST_GROUP_LOG="$tmp/group-calls" \
    IPOD_DOCKER_SOCKET_OVERRIDE="$tmp/private-docker.sock" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --yes --start-docker \
    > "$tmp/group-output"
grep -F 'Added your user to the docker group' "$tmp/group-output" >/dev/null
grep -F 'sg docker -c id -nG' "$tmp/group-calls" >/dev/null
grep -F 'sg docker -c docker compose version && docker info' "$tmp/group-calls" >/dev/null

# On macOS, a missing helper package must not reinstall an already functional
# Docker Desktop/remote CLI or its cask.
brew_bin="$tmp/brew-bin"
mkdir -p "$brew_bin"
cat > "$brew_bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo Darwin ;; -m) echo arm64 ;; esac
EOF
cat > "$brew_bin/docker" <<'EOF'
#!/bin/sh
case "$*" in 'compose version'|info|*) exit 0 ;; esac
EOF
cat > "$brew_bin/brew" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$IPOD_TEST_BREW_LOG"
if [ "$1" = install ]; then
    for name in "$@"; do
        if [ "$name" = jq ]; then printf '#!/bin/sh\nexit 0\n' > "$IPOD_TEST_BREW_BIN/jq"; /bin/chmod +x "$IPOD_TEST_BREW_BIN/jq"; fi
    done
fi
EOF
for name in git unzip file python3 ffmpeg ffprobe; do
    cat > "$brew_bin/$name" <<'EOF'
#!/bin/sh
exit 0
EOF
done
chmod +x "$brew_bin/"*
: > "$tmp/brew-calls"
PATH="$brew_bin" IPOD_DEPENDENCY_PATH="$brew_bin" \
    IPOD_TEST_BREW_LOG="$tmp/brew-calls" IPOD_TEST_BREW_BIN="$brew_bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --install --yes \
    > "$tmp/brew-output"
grep -Fx 'install jq' "$tmp/brew-calls" >/dev/null
if grep -F 'docker-desktop' "$tmp/brew-calls" >/dev/null; then
    echo 'installer unexpectedly requested Docker Desktop despite a working Docker CLI' >&2
    exit 1
fi

# Packaged installer layout mirrors release archives and works from any cwd.
package_root="$tmp/release package"
mkdir -p "$package_root/bin" "$package_root/share/ipod-connection-utility"
cp "$repo_root/scripts/install.sh" "$package_root/bin/ipod-connection-utility-install"
cp "$repo_root/scripts/install-dependencies.sh" \
    "$package_root/bin/ipod-connection-utility-install-dependencies"
cat > "$package_root/bin/ipod-connection-utility" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$package_root/bin/ipod-connection-utility-setup-backend" <<'EOF'
#!/bin/sh
exit 0
EOF
cp "$repo_root/backend/patches/wrapper-portable.patch" \
    "$package_root/share/ipod-connection-utility/wrapper-portable.patch"
cp "$repo_root/README.md" "$package_root/share/ipod-connection-utility/README.md"
cp "$repo_root/DOWNLOADER_SETUP.md" "$package_root/share/ipod-connection-utility/DOWNLOADER_SETUP.md"
cp "$repo_root/tools/check_elf_pages.py" "$package_root/share/ipod-connection-utility/check_elf_pages.py"
chmod +x "$package_root/bin/"*
packaged_prefix="$tmp/packaged install"
(cd / && "$package_root/bin/ipod-connection-utility-install" \
    --sync-only --prefix "$packaged_prefix" > "$tmp/packaged-output")
[[ -x "$packaged_prefix/bin/ipod-connection-utility" ]]
[[ -x "$packaged_prefix/bin/ipod-connection-utility-setup-backend" ]]
[[ -x "$packaged_prefix/bin/ipod-connection-utility-install-dependencies" ]]
[[ -x "$packaged_prefix/bin/ipod-connection-utility-install" ]]
cmp "$package_root/share/ipod-connection-utility/wrapper-portable.patch" \
    "$packaged_prefix/share/ipod-connection-utility/wrapper-portable.patch"
cmp "$package_root/share/ipod-connection-utility/check_elf_pages.py" \
    "$packaged_prefix/share/ipod-connection-utility/check_elf_pages.py"
"$packaged_prefix/bin/ipod-connection-utility-install" --sync-only \
    --prefix "$packaged_prefix" > "$tmp/packaged-rerun-output"
cat > "$packaged_prefix/bin/ipod-connection-utility-install-dependencies" <<'EOF'
#!/bin/sh
exit 90
EOF
chmod +x "$packaged_prefix/bin/ipod-connection-utility-install-dependencies"
"$packaged_prefix/bin/ipod-connection-utility-install" --sync-only \
    --install-deps --yes --prefix "$packaged_prefix" > "$tmp/packaged-core-deps-output"
"$package_root/bin/ipod-connection-utility-install" --sync-only --dry-run \
    --prefix "$packaged_prefix" > "$tmp/packaged-dry-output"
grep -F 'Host dependencies: none required for packaged library sync.' \
    "$tmp/packaged-dry-output" >/dev/null

bad_package="$tmp/bad release"
cp -R "$package_root" "$bad_package"
cat > "$bad_package/bin/ipod-connection-utility" <<'EOF'
#!/bin/sh
exit 126
EOF
chmod +x "$bad_package/bin/ipod-connection-utility"
if "$bad_package/bin/ipod-connection-utility-install" --sync-only \
    --prefix "$tmp/bad install" > "$tmp/bad-package-output" 2>&1; then
    echo 'packaged installer unexpectedly accepted an unrunnable app binary' >&2
    exit 1
fi
grep -F 'packaged app is not runnable on this OS/CPU' "$tmp/bad-package-output" >/dev/null

# Full packaged ARM install routes the newly granted Docker group through sg
# and preserves spaces/quotes in the APKM while enabling build emulation only
# for the explicit package-install path.
flow_package="$tmp/flow release"
mkdir -p "$flow_package/bin" "$flow_package/share/ipod-connection-utility" "$tmp/flow-bin"
cp "$repo_root/scripts/install.sh" "$flow_package/bin/ipod-connection-utility-install"
cat > "$flow_package/bin/ipod-connection-utility" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$flow_package/bin/ipod-connection-utility-install-dependencies" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$flow_package/bin/ipod-connection-utility-setup-backend" <<'EOF'
#!/bin/sh
printf '%s\n' "$@" > "$IPOD_TEST_SETUP_ARGS"
EOF
cp "$repo_root/backend/patches/wrapper-portable.patch" \
    "$flow_package/share/ipod-connection-utility/wrapper-portable.patch"
cp "$repo_root/README.md" "$flow_package/share/ipod-connection-utility/README.md"
cp "$repo_root/DOWNLOADER_SETUP.md" "$flow_package/share/ipod-connection-utility/DOWNLOADER_SETUP.md"
cp "$repo_root/tools/check_elf_pages.py" "$flow_package/share/ipod-connection-utility/check_elf_pages.py"
cat > "$tmp/flow-bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo Linux ;; -m) echo aarch64 ;; esac
EOF
cat > "$tmp/flow-bin/docker" <<'EOF'
#!/bin/sh
case "$*" in
    info) exit 1 ;;
    'context show') echo default ;;
    *) exit 0 ;;
esac
EOF
cat > "$tmp/flow-bin/sg" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$IPOD_TEST_SG_LOG"
[ "$1" = docker ] && [ "$2" = -c ] || exit 90
shift 2
exec /bin/sh -c "$1"
EOF
chmod +x "$flow_package/bin/"* "$tmp/flow-bin/"*
flow_apkm="$tmp/APKM with 'quote'.apkm"
: > "$flow_apkm"
: > "$tmp/setup-args"
: > "$tmp/sg-calls"
IPOD_TEST_SETUP_ARGS="$tmp/setup-args" IPOD_TEST_SG_LOG="$tmp/sg-calls" \
    PATH="$tmp/flow-bin:$PATH" "$flow_package/bin/ipod-connection-utility-install" \
    --apkm "$flow_apkm" --install-deps --yes --prefix "$tmp/flow-prefix" \
    > "$tmp/flow-output"
grep -F -- "--apkm" "$tmp/setup-args" >/dev/null
grep -Fx -- "$flow_apkm" "$tmp/setup-args" >/dev/null
grep -Fx -- '--configure-emulation' "$tmp/setup-args" >/dev/null
grep -F 'docker -c exec ' "$tmp/sg-calls" >/dev/null

if bash "$repo_root/scripts/install.sh" --apkm "$tmp/missing.apkm" \
    --install-deps --yes > "$tmp/missing-apkm-output" 2>&1; then
    echo 'installer unexpectedly accepted a missing APKM' >&2
    exit 1
fi
grep -F 'APKM is not a readable file' "$tmp/missing-apkm-output" >/dev/null

echo "setup/install script regression checks passed"
