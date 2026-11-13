#!/usr/bin/env bash
set -euo pipefail

# Linux Docker Engine validator for the three already-built test images. It uses
# the local bridge IP so the containers can stay on an internal, outbound-free
# network. It never accepts or logs real credentials; optional login validation
# uses a fixed reserved `.invalid` address. This script requires Linux Docker
# Engine because it reaches containers through their internal bridge IP.

images=(
  "${IPOD_VALIDATION_AMD64_IMAGE:-ipod-wrapper-validation:native-amd64}"
  "${IPOD_VALIDATION_ARM64_IMAGE:-ipod-wrapper-validation:native-arm64}"
  "${IPOD_VALIDATION_ASAHI_IMAGE:-ipod-wrapper-validation:asahi-16k}"
)
for command in docker readelf curl jq; do
  command -v "$command" >/dev/null || { echo "required command missing: $command" >&2; exit 2; }
done

tmp=$(mktemp -d "${TMPDIR:-/tmp}/ipod-portable-images.XXXXXX")
network="ipod-portable-validation-$$"
containers=()
success=0
cleanup() {
  local status=$?
  local container
  for container in "${containers[@]}"; do docker rm -f "$container" >/dev/null 2>&1 || true; done
  docker network rm "$network" >/dev/null 2>&1 || true
  if [[ "$success" == 1 ]]; then
    rm -rf "$tmp"
  else
    echo "validation logs and extracted ELFs kept at $tmp" >&2
  fi
  return "$status"
}
trap cleanup EXIT

kernel_page_size=$(getconf PAGESIZE)
compat_page_size=${IPOD_VALIDATION_ASAHI_PAGE_SIZE:-$kernel_page_size}
if [[ "$compat_page_size" != "$kernel_page_size" ]]; then
  echo "Asahi page guard simulation: configured $compat_page_size bytes; actual host kernel is $kernel_page_size bytes"
fi

machine() {
  readelf -h "$1" | awk -F: '/Machine:/{gsub(/^ +/, "", $2); print $2}'
}

check_elf() {
  local path=$1 expected=$2 label=$3 actual
  actual=$(machine "$path")
  if [[ "$actual" != "$expected" ]]; then
    echo "$label: expected $expected, got $actual" >&2
    exit 1
  fi
  printf '%s: %s\n' "$label" "$actual"
}

check_image() {
  local image=$1 profile=$2 platform=$3 native_machine=$4 worker_machine=$5
  local container root="$tmp/$profile"
  mkdir -p "$root"
  local actual_platform
  actual_platform=$(docker image inspect "$image" --format '{{.Os}}/{{.Architecture}}')
  [[ "$actual_platform" == "$platform" ]] || {
    echo "$profile: expected image platform $platform, got $actual_platform" >&2
    exit 1
  }
  printf '\n%s (%s)\n' "$profile" "$actual_platform"

  container=$(docker create "$image")
  containers+=("$container")
  if [[ "$profile" == asahi-16k ]]; then
    docker cp "$container:/app/wrapper-x86" "$root/launcher"
    docker cp "$container:/app/rootfs/system/bin/qemu-x86_64-static" "$root/qemu-inner"
    docker cp "$container:/usr/local/bin/qemu-x86_64-static" "$root/qemu-outer"
  else
    docker cp "$container:/app/wrapper" "$root/launcher"
  fi
  docker cp "$container:/app/wrapperd" "$root/wrapperd"
  docker cp "$container:/app/rootfs/system/bin/main" "$root/main"
  docker rm "$container" >/dev/null

  if [[ "$profile" == asahi-16k ]]; then
    check_elf "$root/launcher" "$worker_machine" "$profile launcher"
    check_elf "$root/qemu-inner" "$native_machine" "$profile chroot QEMU"
    check_elf "$root/qemu-outer" "$native_machine" "$profile outer QEMU"
  else
    check_elf "$root/launcher" "$native_machine" "$profile launcher"
  fi
  check_elf "$root/wrapperd" "$native_machine" "$profile supervisor"
  check_elf "$root/main" "$worker_machine" "$profile Android worker"

  if [[ "$profile" == asahi-16k ]]; then
    readelf -Ws "$root/main" | awk '$NF == "madvise" && $5 == "GLOBAL" && $7 != "UND" { found=1 } END { exit !found }' || {
      echo "$profile: expected exported madvise compatibility guard" >&2
      exit 1
    }
    docker run --rm --entrypoint /bin/sh "$image" -c \
      'test -x /app/wrapper && test -x /usr/local/bin/qemu-x86_64-static && test -x /app/rootfs/system/bin/qemu-x86_64-static && test ! -e /app/rootfs/data'
    echo "$profile: QEMU files and exported page guard present"
  else
    if readelf -Ws "$root/main" | awk '$NF == "madvise" && $5 == "GLOBAL" && $7 != "UND" { found=1 } END { exit found ? 0 : 1 }'; then
      echo "$profile: native worker unexpectedly exports madvise guard" >&2
      exit 1
    fi
    docker run --rm --entrypoint /bin/sh "$image" -c \
      'test ! -e /usr/local/bin/qemu-x86_64-static && test ! -e /app/rootfs/system/bin/qemu-x86_64-static && test ! -e /app/rootfs/data && ! find /app -name "qemu*" | grep -q .'
    echo "$profile: no QEMU files or exported page guard"
  fi
}

check_runtime() {
  local image=$1 profile=$2 name address response status page_size login_pid login_active
  name="ipod-portable-${profile}-$$"
  containers+=("$name")
  page_size=$kernel_page_size
  [[ "$profile" != asahi-16k ]] || page_size=$compat_page_size
  docker run -d --name "$name" --network "$network" \
    --cap-add SYS_ADMIN --cap-add SYS_CHROOT --cap-add SYS_PTRACE \
    --security-opt apparmor=unconfined \
    -e WRAPPER_HOST=0.0.0.0 -e WRAPPER_PORT=80 \
    -e WRAPPER_DECRYPT_HOST=0.0.0.0 -e WRAPPER_DECRYPT_PORT=10020 \
    -e WRAPPER_BASE_DIR=/tmp/ipod-portable-validation-state \
    -e WRAPPER_HOST_PAGE_SIZE="$page_size" "$image" >/dev/null
  address=$(docker inspect "$name" --format "{{(index .NetworkSettings.Networks \"$network\").IPAddress}}")
  for _ in $(seq 1 30); do
    response=$(curl --max-time 2 -fsS "http://$address:80/health?deep=1" 2>/dev/null) && break
    sleep 1
  done
  jq -e '.status == "ok" and .mode == "rust-supervisor" and .worker_ipc.reachable == true and .worker_ipc.status == 200' \
    <<<"${response:-null}" >/dev/null || {
      docker logs "$name" >"$tmp/$profile-runtime.log" 2>&1 || true
      echo "$profile: deep health failed; sanitized startup log: $tmp/$profile-runtime.log" >&2
      exit 1
    }
  echo "$profile: deep health passed"
  if [[ "${IPOD_VALIDATION_PENDING_LOGIN:-0}" == 1 ]]; then
    curl --max-time 45 -sS -o "$tmp/$profile-login-response.json" \
      -H 'Content-Type: application/json' \
      -d '{"apple_id":"offline-test@example.invalid","password":"not-a-real-password"}' \
      "http://$address:80/login" >"$tmp/$profile-login-status" 2>/dev/null &
    login_pid=$!
    for _ in $(seq 1 30); do
      docker logs "$name" 2>&1 | grep -q 'http: POST /login' && break
      sleep 0.1
    done
    sleep 0.1
    if kill -0 "$login_pid" 2>/dev/null; then
      login_active=1
    else
      login_active=0
    fi
  fi
  docker stop --time 2 "$name" >/dev/null
  status=$(docker inspect "$name" --format '{{.State.Status}}')
  [[ "$status" == exited ]] || { echo "$profile: container did not stop ($status)" >&2; exit 1; }
  local exit_code
  exit_code=$(docker inspect "$name" --format '{{.State.ExitCode}}')
  [[ "$exit_code" == 0 ]] || {
    docker logs "$name" >"$tmp/$profile-shutdown.log" 2>&1 || true
    echo "$profile: expected clean exit 0, got $exit_code; see $tmp/$profile-shutdown.log" >&2
    exit 1
  }
  echo "$profile: cleanly stopped (exit 0)"
  if [[ -n "${login_pid:-}" ]]; then
    wait "$login_pid" 2>/dev/null || true
    if [[ -f "$tmp/$profile-login-status" ]]; then
      local login_status
      login_status=$(cat "$tmp/$profile-login-status")
      if [[ "$login_status" == 401 ]]; then
        echo "$profile: dummy offline login completed with HTTP 401"
      elif [[ -z "$login_status" || "$login_status" == 000 ]]; then
        if [[ "$login_active" == 1 ]]; then
          echo "$profile: pending dummy offline login was interrupted by shutdown"
        else
          echo "$profile: dummy offline login had no HTTP result before shutdown"
        fi
      else
        echo "$profile: unexpected dummy login HTTP status $login_status" >&2
        exit 1
      fi
    fi
  fi
}

docker network create --internal "$network" >/dev/null
check_image "${images[0]}" native-amd64 linux/amd64 "Advanced Micro Devices X86-64" "Advanced Micro Devices X86-64"
check_image "${images[1]}" native-arm64 linux/arm64 AArch64 AArch64
check_image "${images[2]}" asahi-16k linux/arm64 AArch64 "Advanced Micro Devices X86-64"

check_runtime "${images[0]}" native-amd64
check_runtime "${images[1]}" native-arm64
check_runtime "${images[2]}" asahi-16k

echo "all portable image CPU/content and network-isolated runtime checks passed"
success=1
