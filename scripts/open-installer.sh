#!/usr/bin/env bash
# Copy unchanged to the release root as Install.sh.
set -euo pipefail
script_dir=$(cd "${BASH_SOURCE[0]%/*}" && pwd)
wizard=""
for candidate in "$script_dir/bin/ipod-connection-utility-install-wizard" \
    "$script_dir/ipod-connection-utility-install-wizard" "$script_dir/install-wizard.sh"; do
    if [[ -f "$candidate" ]]; then wizard=$candidate; break; fi
done
if [[ -z "$wizard" ]]; then
    echo 'Installer wizard missing. Extract the entire release archive before opening Install.sh.' >&2
    exit 1
fi
if [[ -t 0 && -t 1 ]]; then exec bash "$wizard"; fi
export IPOD_INSTALLER_HOLD=1
if command -v konsole >/dev/null 2>&1; then
    exec konsole -e bash "$wizard"
elif command -v x-terminal-emulator >/dev/null 2>&1; then
    exec x-terminal-emulator -e bash "$wizard"
elif command -v gnome-terminal >/dev/null 2>&1; then
    exec gnome-terminal -- bash "$wizard"
elif command -v xfce4-terminal >/dev/null 2>&1; then
    exec xfce4-terminal -x bash "$wizard"
elif command -v mate-terminal >/dev/null 2>&1; then
    exec mate-terminal -x bash "$wizard"
elif command -v kitty >/dev/null 2>&1; then
    exec kitty bash "$wizard"
elif command -v alacritty >/dev/null 2>&1; then
    exec alacritty -e bash "$wizard"
elif command -v foot >/dev/null 2>&1; then
    exec foot bash "$wizard"
elif command -v xterm >/dev/null 2>&1; then
    exec xterm -e bash "$wizard"
fi
message="No supported terminal found. Install Konsole, xterm, or another terminal, then run: bash $(printf '%q' "$wizard")"
printf '%s\n' "$message" >&2
if command -v zenity >/dev/null 2>&1; then zenity --error --text="$message" || :
elif command -v kdialog >/dev/null 2>&1; then kdialog --error "$message" || :; fi
exit 1
