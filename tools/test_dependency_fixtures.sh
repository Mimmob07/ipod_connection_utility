#!/usr/bin/env bash
# Hermetic dependency plans: no host package manager or Docker can execute.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bash_path=$BASH
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/home"
export IPOD_DOCKER_DESKTOP_APP="$tmp/Docker.app"
cat > "$tmp/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo "${TEST_OS:-Linux}" ;; -m) echo aarch64 ;; esac
EOF
cat > "$tmp/bin/getconf" <<'EOF'
#!/bin/sh
echo 16384
EOF
for name in pacman apt-get dnf brew sudo systemctl service open; do
    cat > "$tmp/bin/$name" <<'EOF'
#!/bin/sh
printf '%s %s\n' "$0" "$*" >> "$TEST_EFFECTS"
exit 91
EOF
done
chmod +x "$tmp/bin/"*
: > "$tmp/effects"
run_plan() {
    local id="$1" version="$2" expected="$3"
    printf 'ID=%s\nVERSION_ID="%s"\n' "$id" "$version" > "$tmp/os-release"
    TEST_EFFECTS="$tmp/effects" HOME="$tmp/home" TEST_OS=Linux \
        IPOD_OS_RELEASE_FILE="$tmp/os-release" IPOD_DEPENDENCY_PATH="$tmp/bin" \
        PATH="$tmp/bin" "$bash_path" "$repo_root/scripts/install-dependencies.sh" \
        --dry-run --yes --source --start-docker > "$tmp/output" 2>&1
    if ! grep -F "$expected" "$tmp/output" >/dev/null; then
        cat "$tmp/output" >&2
        echo "missing package plan for $id: $expected" >&2
        return 1
    fi
    grep -F "Host: Linux aarch64 ($id)" "$tmp/output" >/dev/null
    grep -F 'Host page size: 16384 bytes' "$tmp/output" >/dev/null
    [[ ! -s "$tmp/effects" ]]
    # Include dotfiles without depending on host ls or external find tools.
    shopt -s nullglob dotglob
    local home_files=("$tmp/home/"*)
    ((${#home_files[@]} == 0))
    shopt -u nullglob dotglob
    echo "PASS dependency plan: $id $version (no effects)"
}
run_plan ubuntu 24.04 'sudo apt-get install -y --no-upgrade docker.io git jq unzip file python3 python3-venv ffmpeg docker-compose-v2 docker-buildx curl build-essential'
for id in arch alarm archarm; do
    run_plan "$id" '' 'sudo pacman -S --needed docker docker-compose docker-buildx git jq unzip file python ffmpeg curl base-devel'
done
run_plan debian 13 'sudo apt-get install -y --no-upgrade docker.io git jq unzip file python3 python3-venv ffmpeg docker-compose docker-buildx curl build-essential'
run_plan debian 12 'sudo apt-get install -y --no-upgrade docker.io git jq unzip file python3 python3-venv ffmpeg curl build-essential'
if grep -F 'docker-compose' "$tmp/output" >/dev/null; then
    echo 'Debian 12 must not promise a repository Compose v2 package' >&2
    exit 1
fi
run_plan fedora 43 'sudo dnf install -y moby-engine docker-compose docker-buildx git jq unzip file python3 ffmpeg-free curl gcc make'

# Docker is present but must not even be queried in a dry run.
cp "$tmp/bin/sudo" "$tmp/bin/docker"
TEST_EFFECTS="$tmp/effects" HOME="$tmp/home" TEST_OS=Darwin \
    IPOD_DEPENDENCY_PATH="$tmp/bin" PATH="$tmp/bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run --yes --start-docker > "$tmp/output"
grep -F 'brew install git jq unzip python ffmpeg docker-compose docker-buildx' "$tmp/output" >/dev/null
[[ ! -s "$tmp/effects" ]]
echo 'PASS macOS existing Docker plan (no effects)'
rm "$tmp/bin/docker"

# Installed Desktop location is explicit too, independent of real Mac apps.
mkdir -p "$IPOD_DOCKER_DESKTOP_APP"
TEST_EFFECTS="$tmp/effects" HOME="$tmp/home" TEST_OS=Darwin \
    IPOD_DEPENDENCY_PATH="$tmp/bin" PATH="$tmp/bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run --yes > "$tmp/output"
if grep -F 'install --cask' "$tmp/output" >/dev/null; then
    cat "$tmp/output" >&2
    echo 'reinstalled an explicitly present Docker Desktop fixture' >&2
    exit 1
fi
rmdir "$IPOD_DOCKER_DESKTOP_APP"
echo 'PASS macOS existing Desktop location'

# Cask-only plan: all helper commands are present, Docker is not.
for name in git jq unzip file python3 ffmpeg ffprobe; do
    printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/$name"
    chmod +x "$tmp/bin/$name"
done
TEST_EFFECTS="$tmp/effects" HOME="$tmp/home" TEST_OS=Darwin \
    IPOD_DEPENDENCY_PATH="$tmp/bin" PATH="$tmp/bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run --yes > "$tmp/output"
if ! grep -F 'brew install --cask docker-desktop' "$tmp/output" >/dev/null; then
    cat "$tmp/output" >&2
    echo 'missing macOS cask-only package plan' >&2
    exit 1
fi
[[ ! -s "$tmp/effects" ]]
echo 'PASS macOS cask-only plan (no effects)'

# A cask-only install must execute exactly the cask, not an empty brew install.
cat > "$tmp/bin/brew" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >> "$TEST_EFFECTS"
[ "$*" = 'install --cask docker-desktop' ] || exit 92
printf '#!/bin/sh\nexit 0\n' > "$TEST_BIN/docker"
/bin/chmod +x "$TEST_BIN/docker"
EOF
TEST_BIN="$tmp/bin" TEST_EFFECTS="$tmp/effects" HOME="$tmp/home" TEST_OS=Darwin \
    IPOD_DEPENDENCY_PATH="$tmp/bin" PATH="$tmp/bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --yes > "$tmp/output" 2>&1 || {
        cat "$tmp/output" >&2
        echo 'cask-only install did not satisfy Docker prerequisites' >&2
        exit 1
    }
grep -Fx 'install --cask docker-desktop' "$tmp/effects" >/dev/null
[[ $(wc -l < "$tmp/effects") -eq 1 ]]
echo 'PASS macOS cask-only mocked installation'

# Fixture input is data, never shell code, and absent explicit files fail closed.
printf 'ID="alarm"\nVERSION_ID=\0472026\047\nprintf hacked > "%s"\n' "$tmp/executed" > "$tmp/os-release"
TEST_OS=Linux IPOD_OS_RELEASE_FILE="$tmp/os-release" IPOD_DEPENDENCY_PATH="$tmp/bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run > "$tmp/output"
grep -F 'Host: Linux aarch64 (alarm)' "$tmp/output" >/dev/null
[[ ! -e "$tmp/executed" ]]
for invalid in '' "$tmp/absent" "$tmp/home"; do
    if TEST_OS=Linux IPOD_OS_RELEASE_FILE="$invalid" IPOD_DEPENDENCY_PATH="$tmp/bin" \
        "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run > "$tmp/output" 2>&1; then
        echo 'accepted an invalid explicit os-release fixture' >&2
        exit 1
    fi
    grep -F 'IPOD_OS_RELEASE_FILE must name a readable regular file' "$tmp/output" >/dev/null
done
if IPOD_DEPENDENCY_PATH='' "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run > "$tmp/output" 2>&1; then
    echo 'accepted an empty explicit tool path' >&2
    exit 1
fi
grep -F 'IPOD_DEPENDENCY_PATH must not be empty' "$tmp/output" >/dev/null
echo 'PASS literal fixture parsing and invalid-input rejection'

# Existing remote/rootless engines must not trigger package/service/group work.
cat > "$tmp/bin/docker" <<'EOF'
#!/bin/sh
printf 'docker %s\n' "$*" >> "$TEST_EFFECTS"
case "$*" in
    'context show') echo rootless ;;
    'compose version'|'buildx version'|info) exit 0 ;;
    *) exit 93 ;;
esac
EOF
for endpoint in '' 'ssh://fixture.invalid'; do
    : > "$tmp/effects"
    DOCKER_HOST="$endpoint" TEST_EFFECTS="$tmp/effects" TEST_OS=Linux HOME="$tmp/home" \
        IPOD_OS_RELEASE_FILE="$tmp/os-release" IPOD_DEPENDENCY_PATH="$tmp/bin" \
        "$bash_path" "$repo_root/scripts/install-dependencies.sh" --yes --start-docker > "$tmp/output"
    grep -F 'Host dependency checks passed.' "$tmp/output" >/dev/null
    if grep -v '^docker ' "$tmp/effects" | grep . >/dev/null; then
        echo 'changed packages/services for an existing engine' >&2
        exit 1
    fi
done
echo 'PASS existing rootless/remote Docker (no package or service effects)'

# Source-only with a sufficient toolchain has an empty plan even on Bash 3.2.
printf '#!/bin/sh\necho "cargo 1.90.0"\n' > "$tmp/bin/cargo"
printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/cc"
chmod +x "$tmp/bin/cargo" "$tmp/bin/cc"
ln -s "$(command -v sed)" "$tmp/bin/sed"
: > "$tmp/effects"
TEST_EFFECTS="$tmp/effects" TEST_OS=Linux HOME="$tmp/home" \
    IPOD_OS_RELEASE_FILE="$tmp/os-release" IPOD_DEPENDENCY_PATH="$tmp/bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --source-only --dry-run --yes > "$tmp/output"
grep -F 'Required commands are present' "$tmp/output" >/dev/null
[[ ! -s "$tmp/effects" ]]
echo 'PASS source-only sufficient toolchain (empty plan, no effects)'

# No Homebrew with only Docker missing still needs an actionable plan.
rm "$tmp/bin/docker" "$tmp/bin/brew"
TEST_EFFECTS="$tmp/effects" TEST_OS=Darwin HOME="$tmp/home" IPOD_DEPENDENCY_PATH="$tmp/bin" \
    "$bash_path" "$repo_root/scripts/install-dependencies.sh" --dry-run --yes > "$tmp/output"
if ! grep -F 'install Homebrew from https://brew.sh' "$tmp/output" >/dev/null; then
    cat "$tmp/output" >&2
    echo 'missing Homebrew recovery for a cask-only plan' >&2
    exit 1
fi
echo 'PASS macOS missing Homebrew cask-only guidance'

