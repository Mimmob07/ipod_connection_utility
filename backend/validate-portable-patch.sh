#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
reference=${1:-"$repo_root/.wrapper-v2"}
reference=$(cd "$reference" && pwd)
base=100e0a864e883e03a3ac450a780dd9563fff5271
tmp=$(mktemp -d "${TMPDIR:-/tmp}/ipod-wrapper-patch.XXXXXX")
trap 'rm -rf "$tmp"' EXIT

if [[ $(git -C "$reference" rev-parse HEAD) != "$base" ]]; then
  echo "reference checkout must be at $base" >&2
  exit 1
fi

# Clone the base commit without carrying the reference checkout's dirty tree.
git clone --quiet "$reference" "$tmp/fresh"
git -C "$tmp/fresh" reset --hard --quiet "$base"
git -C "$tmp/fresh" apply --check "$repo_root/backend/patches/wrapper-portable.patch"
git -C "$tmp/fresh" apply "$repo_root/backend/patches/wrapper-portable.patch"
git -C "$tmp/fresh" diff --check

for path in \
  .dockerignore CMakeLists.txt Dockerfile compose.yaml \
  src/daemon/CMakeLists.txt src/daemon/apple/loader.cpp \
  src/daemon/page_compat.cpp src/launcher/wrapper.c \
  src/rust/main.rs src/rust/worker.rs \
  tools/start-worker.sh tools/test-dockerignore.sh tools/test-page-compat.cpp
do
  cmp "$reference/$path" "$tmp/fresh/$path"
done

c++ -std=c++17 -Wall -Wextra -Werror -O2 \
  "$tmp/fresh/tools/test-page-compat.cpp" \
  "$tmp/fresh/src/daemon/page_compat.cpp" \
  -o "$tmp/page-test"
"$tmp/page-test"

if command -v docker >/dev/null && docker buildx version >/dev/null 2>&1; then
  bash "$tmp/fresh/tools/test-dockerignore.sh"
fi

echo "portable patch applies cleanly, matches the reference files, and passes available backend checks"
