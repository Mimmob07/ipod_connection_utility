#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin"
: > "$fixture/input.apkm"

cat > "$fixture/bin/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$IPOD_TEST_EMULATION_STATE/docker.log"
case "$1" in
  info|compose) exit 0 ;;
  version) printf '%s\n' "$IPOD_TEST_SERVER_ARCH" ;;
  run)
    if [[ "$*" == *--privileged* ]]; then
      [[ "$*" == *tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0* ]]
      [[ "$*" == *'--install amd64'* ]]
      touch "$IPOD_TEST_EMULATION_STATE/registered"
    elif [[ "$*" == *'--platform linux/amd64'* ]]; then
      [[ -f "$IPOD_TEST_EMULATION_STATE/registered" ]]
    fi
    ;;
  *) exit 91 ;;
esac
EOF
cat > "$fixture/bin/git" <<'EOF'
#!/usr/bin/env bash
touch "$IPOD_TEST_EMULATION_STATE/clone-reached"
exit 73
EOF
for dependency in jq unzip file; do
  printf '#!/usr/bin/env bash\nexit 0\n' > "$fixture/bin/$dependency"
done
chmod +x "$fixture/bin/"*

run_case() {
  local name=$1 arch=$2 pages=$3 allowed=$4
  local state="$fixture/$name"
  mkdir -p "$state"
  local flags=(--skip-python)
  if [[ "$allowed" == yes ]]; then flags+=(--configure-emulation); fi
  if PATH="$fixture/bin:$PATH" XDG_DATA_HOME="$state/data" \
    IPOD_TEST_EMULATION_STATE="$state" IPOD_TEST_SERVER_ARCH="$arch" \
    bash "$repo_root/scripts/setup-backend.sh" --apkm "$fixture/input.apkm" \
      --page-size "$pages" "${flags[@]}" > "$state/output" 2>&1; then
    echo "setup unexpectedly passed the failing mock clone" >&2
    exit 1
  fi
  if [[ "$arch" == arm64 && "$allowed" == yes ]]; then
    [[ -f "$state/registered" && -f "$state/clone-reached" ]]
    grep -F 'Registering static amd64 build emulation' "$state/output" >/dev/null
  else
    [[ ! -f "$state/registered" && ! -f "$state/clone-reached" ]]
    if grep -q -- --privileged "$state/docker.log"; then
      echo "unexpected privileged registration without ARM opt-in" >&2
      exit 1
    fi
    grep -F 'cannot execute the amd64 NDK build stage' "$state/output" >/dev/null
  fi
}

run_case arm-no-consent arm64 16384 no
run_case arm-configured arm64 16384 yes
run_case amd64-no-registration amd64 4096 yes
echo "setup emulation opt-in and pinned registration checks passed"
