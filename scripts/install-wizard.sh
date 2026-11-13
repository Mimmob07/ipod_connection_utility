#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
    -h|--help)
        echo 'Usage: ipod-connection-utility-install-wizard [--basic|--advanced]'
        echo 'Runs an interactive terminal installer. Choose Basic AAC (default), sync-only or advanced wrapper setup, a prefix, dependency consent, then optional app launch.'
        echo 'Automation: use ipod-connection-utility-install --sync-only or --apkm PATH with explicit options instead.'
        exit 0 ;;
    ''|--basic|--advanced) ;;
    *) echo 'Unknown wizard argument. Use --help, or run without arguments in a terminal.' >&2; exit 1 ;;
esac
[[ "$(id -u)" != 0 ]] || { echo 'Run as your normal user, not with sudo; the installer asks for sudo only for package changes.' >&2; exit 1; }
if [[ ! -t 0 ]]; then
    echo 'Installer needs a terminal. Open Install.sh, or run ipod-connection-utility-install --interactive in a terminal. For automation, use --sync-only or --apkm PATH with explicit options.' >&2
    exit 1
fi
finish() {
    status=$?
    if [[ "${IPOD_INSTALLER_HOLD:-0}" == 1 && -t 0 ]]; then
        printf '\nPress Enter to close this installer terminal. '
        IFS= read -r _ || :
    fi
    exit "$status"
}
trap finish EXIT
script_dir=$(cd "${BASH_SOURCE[0]%/*}" && pwd)
installer="$script_dir/ipod-connection-utility-install"
[[ -f "$installer" ]] || installer="$script_dir/install.sh"
[[ -f "$installer" ]] || { echo 'Installer helper missing: extract the complete archive.' >&2; exit 1; }
prompt() {
    printf '%s ' "$1"
    if ! IFS= read -r answer; then echo 'Input closed; installation cancelled.' >&2; exit 1; fi
}
echo 'iPod Connection Utility — guided installation'
echo '1) Basic AAC (recommended): app + private downloader; no Docker, APKM, git or NDK.'
echo '2) Sync-only: local library/iPod sync; no Docker or Apple account needed.'
echo '3) Advanced wrapper: Docker backend + gamdl; requires your own APKM.'
echo 'Asahi / Arch Linux ARM (ALARM): use the ARM64 Linux release (aarch64-unknown-linux-gnu).'
echo 'Backend profile is selected from the Docker server CPU/page size, not guessed from this terminal.'
echo 'Full setup builds from source and downloads dependencies/images; it can take time and disk space.'
echo 'Apple inputs are not redistributed and are never downloaded by this wizard.'
echo 'After setup, sign in locally in the app Downloader tab; Apple login/2FA requires you, not unattended setup.'
case "${1:-}" in
    --basic) answer=1 ;;
    --advanced) answer=3 ;;
    *) prompt 'Choose setup [1=Basic AAC, 2=sync-only, 3=advanced; default 1]:' ;;
esac
case "${answer:-1}" in
    1) args=(--basic) ;;
    2) args=(--sync-only) ;;
    3)
        echo 'Supply your own Apple Music 3.6.0-beta build 1109 APKM (matching library hashes required).'
        echo 'Obtain the bundle yourself from a trusted source; enter its full path below without shell quotes.'
        echo 'Tip: copy the file location from your file manager. Spaces and apostrophes are accepted literally.'
        prompt 'APKM file path (blank cancels):'
        [[ -n "$answer" ]] || { echo 'Cancelled; no Apple input supplied.'; exit 0; }
        case "$answer" in \~/*) answer="$HOME/${answer:2}" ;; esac
        [[ -f "$answer" && -r "$answer" ]] || { echo "APKM is not readable: $answer. Check the path and retry; no automatic download is performed." >&2; exit 1; }
        args=(--apkm "$answer") ;;
    *) echo 'Invalid choice. Run again and choose 1, 2 or 3.' >&2; exit 1 ;;
esac
prompt "Install prefix [${HOME:?HOME is required}/.local]:"
prefix=${answer:-$HOME/.local}
case "$prefix" in \~/*) prefix="$HOME/${prefix:2}" ;; esac
[[ "$prefix" == /* ]] || { echo 'Use an absolute prefix path (for example /home/you/.local).' >&2; exit 1; }
args+=(--prefix "$prefix")
if [[ "${args[0]}" == --apkm ]]; then
echo 'Dependency consent allows package/Rust installation and Docker startup/access setup (sudo may ask for your password).'
echo 'Docker group access is root-equivalent. On ARM, consent also allows privileged amd64 binfmt registration if the backend build probe needs it.'
echo 'Choose No to check existing prerequisites only; full setup still downloads backend source/images and Python packages.'
else
    echo 'Basic AAC uses Python 3.10+, ffmpeg/ffprobe and a private per-user gamdl environment; no Docker or APKM setup.'
    echo 'Dependency consent allows missing host packages (and Rust/compiler for a source app build). Python packages are downloaded during Basic setup.'
fi
prompt 'Allow missing dependencies and the system changes above? [y/N]:'
if [[ "$answer" == y || "$answer" == Y ]]; then args+=(--install-deps --yes); fi
cleanup="$script_dir/ipod-connection-utility-cleanup"
[[ -f "$cleanup" ]] || cleanup="$script_dir/cleanup-legacy.sh"
if [[ -f "$cleanup" ]] && scan=$(bash "$cleanup" --prefix "$prefix" --scan 2>/dev/null); then
    legacy_lines=$(grep '^LEGACY' <<<"$scan" || :)
    backend_lines=$(grep '^BACKEND' <<<"$scan" || :)
    note_lines=$(grep '^NOTE' <<<"$scan" || :)
    show() { while IFS=$'\t' read -r _ what size; do printf '  - %s (%s)\n' "$what" "$size"; done <<<"$1"; }
    if [[ -n "$legacy_lines" ]]; then
        echo 'Found files from an earlier install that this version no longer uses:'
        show "$legacy_lines"
        prompt 'Remove them? Music, settings and sign-ins are kept. [Y/n]:'
        [[ "$answer" == n || "$answer" == N ]] || args+=(--cleanup-legacy)
    fi
    if [[ -n "$backend_lines" && "${args[0]}" != --apkm ]]; then
        echo 'Found the old advanced (ALAC) Docker backend from an earlier install:'
        show "$backend_lines"
        echo 'Basic AAC does not need it. Removing frees disk space; ALAC sign-in data is kept,'
        echo 'and you can reinstall it later from the app (Downloader tab, key u) with your APKM.'
        prompt 'Remove the old ALAC backend? Recommended if you only use AAC. [y/N]:'
        [[ "$answer" != y && "$answer" != Y ]] || args+=(--remove-old-backend)
    fi
    [[ -z "$note_lines" ]] || show "$note_lines"
fi
printf 'Install into %s\n' "$prefix"
prompt 'Proceed? [y/N]:'
[[ "$answer" == y || "$answer" == Y ]] || { echo 'Cancelled; nothing installed.'; exit 0; }
if bash "$installer" "${args[@]}"; then
    echo 'Installation completed. Your music library and iPod have not been modified.'
else
    status=$?
    echo "Installation failed (status $status). Read the error above, fix the missing dependency/Docker access or APKM hash mismatch, then retry this wizard with the same prefix." >&2
    echo 'Some app files or backend build state may already exist; rerunning is supported. Do not run the whole installer with sudo.' >&2
    exit "$status"
fi
if [[ -L "$prefix/bin/icu" && "$(readlink "$prefix/bin/icu")" == ipod-connection-utility ]]; then
    if [[ ":$PATH:" == *":$prefix/bin:"* ]]; then
        echo 'Launch from a terminal by typing: icu'
    else
        printf 'Launch from a terminal: %q  (add %q to PATH to type just icu)\n' "$prefix/bin/icu" "$prefix/bin"
    fi
else
    printf 'Launch from a terminal: %q\n' "$prefix/bin/ipod-connection-utility"
fi
[[ "${IPOD_INSTALLER_NO_LAUNCH:-0}" != 1 ]] || exit 0
prompt 'Launch app now? [y/N]:'
if [[ "$answer" == y || "$answer" == Y ]]; then exec "$prefix/bin/ipod-connection-utility"; fi
