# Downloader setup

## Basic AAC (default)

No Docker is needed. The installer's Basic AAC mode, or the standalone
`ipod-connection-utility-setup-downloader` command, creates the private gamdl
environment. In the app's Downloader tab, press **o** to open Apple Music and
sign in, **c** to import a Netscape `cookies.txt`, and **n** to queue a URL. See
[QUICKSTART.md](QUICKSTART.md) for details.

## Advanced ALAC (optional Docker backend)

The full installer prepares Docker with Compose, `ffmpeg`/`ffprobe`, Python,
Git, `jq`, `unzip`, the patched backend image, and a private gamdl environment:

```sh
./scripts/install.sh --apkm /path/to/apple-music.apkm --install-deps
```

From an extracted archive use `bin/ipod-connection-utility-install` with the
same arguments. `--dry-run` shows the plan, `--yes` accepts system changes, and
`--prefix` chooses the app install location. The backend and persistent session
state still use your app-data directory. For core sync without Docker use
`--sync-only`; a source install may combine it with `--install-deps` for Rust.

The installer supports Arch/ALARM packages, available Debian/Ubuntu/Fedora
packages, and Homebrew on macOS. It stops with instructions when the required
package manager or Compose package is unavailable. Run as your normal user;
only package/service/group changes use administrator access. Docker must be
usable by the account running the TUI. If local rootful Docker needs group
access, the installer explains the access grant and activates it for setup
without making its socket world-writable. New login sessions inherit the group.
The installed app calls POSIX `setsid()` directly, so macOS does not need the
Linux util-linux `setsid` command.

If the app and host prerequisites are already installed, the standalone setup
command builds the matching backend and installs the host Python environment:

```sh
ipod-connection-utility-setup-backend --apkm /path/to/apple-music.apkm
```

From a source checkout use `./scripts/setup-backend.sh`. It creates `gamdl` under
the app data directory (`$XDG_DATA_HOME/ipod-connection-utility` when set on
either OS; otherwise `~/.local/share/ipod-connection-utility` on Linux or
`~/Library/Application Support/ipod-connection-utility` on macOS). `gamdl` has
a native Rust extension; use a supported Python and recreate the environment on
each CPU rather than copying it. Python 3.10 or newer is required; a fresh
Python 3.14 environment with the published x86_64 ABI3 wheel is known to work.
The helper also installs mutagen and Pillow.
`XDG_DATA_HOME` must be absolute; empty or relative values use the OS fallback.

### Backend

On ARM64 Linux (including Asahi and Arch Linux ARM), use the
`aarch64-unknown-linux-gnu` app archive, or build from source on the machine.
The app is native ARM64; its explicit 64 KiB ELF alignment supports hosts with
16 KiB pages. Docker's ARM64/16 KiB detection selects
`asahi-16k`: ARM64 supervisor, x86 launcher/Android worker through packaged
QEMU, and the guard set to the actual kernel page size. The setup tool checks
the supervisor/QEMU ELF layout and native host Python libraries before saving
the configuration. These structural checks do not replace live testing on
an actual large-page machine.

The NDK compilation stage still needs amd64 execution. Full installation with
`--install-deps` can register static build emulation on an ARM Docker server
only when the probe fails. Standalone setup requires `--configure-emulation`
for that privileged operation; it uses a pinned emulator image and retries the
probe. The image's explicit worker emulator handles normal backend startup
without host binfmt registration. Docker documents the host registration
requirements in its [multi-platform build guide](https://docs.docker.com/build/building/multi-platform/).

See [backend/README.md](backend/README.md) for the pinned wrapper-v2 source and
portable patch. Setup detects the Docker server CPU and Docker Linux page size,
then selects `native-amd64`, `native-arm64`, or the packaged-QEMU `asahi-16k`
compatibility profile. Each profile gets an isolated checkout, image tag, and
session directory. The selected tuple is saved for the app; independent
`TARGET_ARCH`/`RUNTIME_PLATFORM` environment values are intentionally not mixed.

Use `--profile NAME` to explicitly select a profile compatible with the Docker
server's CPU and page size. A remote Docker context runs the backend remotely:
the app's loopback connections require local port forwarding, and session bind
paths must exist on the Docker server. For the compatibility profile,
`--page-size` can supply the actual Docker Linux page size.

Keep wrapper HTTP/decrypt ports on host loopback. Preserve its `./data` session
bind mount across stops and never commit it. Enter account credentials and 2FA
only through the app's masked local prompts. Do not dump `/me` responses: they
contain tokens. The app restores saved sessions and manages backend startup and
shutdown.

### Run and use the app

```sh
ipod-connection-utility -l /path/to/library -p /path/to/ipod
```

The app detects the setup tool's app-data `gamdl` and the legacy
`~/.local/share/ipod-gamdl/bin/gamdl` path, plus its sibling Python.
`IPOD_GAMDL`, `IPOD_PYTHON`, and `IPOD_WRAPPER_DIR` override these locations.
Setup saves a custom `--backend-dir` by profile; `IPOD_WRAPPER_DIR` takes
precedence over that saved path. Runtime `auto` uses the saved Docker profile.
Wrapper ports default to 8080/10020; match `IPOD_WRAPPER_HTTP_PORT` and
`IPOD_WRAPPER_DECRYPT_PORT` with Compose's `HTTP_PORT` and `DECRYPT_PORT` when
changing host bindings.

Downloader keys: `c` signs in, `n` queues a song/playlist, `v` selects quality,
`b` toggles Rockbox artwork, and `t` retries failed/library-blocked rows.
Playlists expand into song rows with individual statuses. The queue supports
Up/Down, PgUp/PgDn and Home/End; its progress bar counts processed songs, while
active transfers show backend percentages and verification/saving phases.
Completed songs can be scanned with `s` and synced with `a`.

The library must be writable. Final saves use exclusive byte copies for exFAT
compatibility. A write-and-sync check pauses entries as `Library blocked` if the
library cannot be written. Fix its filesystem/mount issue before pressing `t`.
Normal jobs have a one-second spacing; detected rate limits trigger the separate
five-minute backoff. A recovered limit does not make a verified/saved song
Partial.

Use an account with the required subscription and review the upstream backend's
terms and setup requirements.
