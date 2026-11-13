#!/usr/bin/env bash
# Build archives from an explicit binary and the current source helpers.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
binary=""; target=""; output=""
while (($#)); do
    case "$1" in
        --binary|--target|--output)
            (($# >= 2)) || { echo "Missing value for $1" >&2; exit 2; }
            case "$1" in --binary) binary=$2 ;; --target) target=$2 ;; --output) output=$2 ;; esac
            shift 2 ;;
        --help|-h) echo 'Usage: package-release.sh --binary PATH --target TARGET --output DIRECTORY'; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ -f "$binary" && -x "$binary" ]] || { echo 'An executable --binary is required' >&2; exit 2; }
case "$target" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|aarch64-apple-darwin) ;;
    *) echo 'Unsupported --target' >&2; exit 2 ;;
esac
[[ -n "$output" ]] || { echo '--output is required' >&2; exit 2; }
if [[ "$target" == aarch64-unknown-linux-gnu ]]; then
    python3 "$root/tools/check_elf_pages.py" --page-size 16384 --machine aarch64 "$binary"
fi
mkdir -p "$output"
output=$(cd "$output" && pwd)
package="ipod-connection-utility-$target"
lock="$output/.$package.lock"
mkdir "$lock" 2>/dev/null || { echo "Another packager owns $lock; inspect before removing a stale lock" >&2; exit 1; }
staging=""
cleanup() {
    [[ -z "$staging" ]] || rm -rf "$staging"
    rmdir "$lock"
}
trap cleanup EXIT
if [[ -e "$output/$package.tar.gz" || -e "$output/$package.tar.gz.sha256" ]]; then
    echo 'Refusing to overwrite an existing archive or checksum; choose a fresh output directory' >&2
    exit 1
fi
staging=$(mktemp -d "$output/.$package.XXXXXX")
payload="$staging/$package"
mkdir -p "$payload/bin" "$payload/share/ipod-connection-utility"
cp "$binary" "$payload/bin/ipod-connection-utility"
cp "$root/scripts/install.sh" "$payload/bin/ipod-connection-utility-install"
cp "$root/scripts/install-dependencies.sh" "$payload/bin/ipod-connection-utility-install-dependencies"
cp "$root/scripts/setup-backend.sh" "$payload/bin/ipod-connection-utility-setup-backend"
cp "$root/scripts/setup-downloader.sh" "$payload/bin/ipod-connection-utility-setup-downloader"
cp "$root/scripts/cleanup-legacy.sh" "$payload/bin/ipod-connection-utility-cleanup"
cp "$root/scripts/install-wizard.sh" "$payload/bin/ipod-connection-utility-install-wizard"
cp "$root/scripts/open-installer.sh" "$payload/Install.sh"
chmod 755 "$payload/Install.sh" "$payload/bin/"*
cp "$root/backend/patches/wrapper-portable.patch" "$payload/share/ipod-connection-utility/"
cp "$root/tools/check_elf_pages.py" "$payload/share/ipod-connection-utility/"
cp "$root/LICENSE" "$payload/"
cp "$root/README.md" "$root/DOWNLOADER_SETUP.md" "$payload/share/ipod-connection-utility/"
cp "$root/QUICKSTART.md" "$payload/START_HERE.md"
cp "$root/QUICKSTART.md" "$payload/share/ipod-connection-utility/"
tar -czf "$staging/$package.tar.gz" -C "$staging" "$package"
(
    cd "$staging"
    if command -v sha256sum >/dev/null; then
        sha256sum "$package.tar.gz" > "$package.tar.gz.sha256"
    else
        shasum -a 256 "$package.tar.gz" > "$package.tar.gz.sha256"
    fi
)
mv "$staging/$package.tar.gz" "$staging/$package.tar.gz.sha256" "$output/"
printf 'Archive: %s/%s.tar.gz\n' "$output" "$package"
