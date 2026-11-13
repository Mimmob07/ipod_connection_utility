# iPod Connection Utility

A terminal app (`icu`) that previews and syncs a local music library to an iPod, with an optional Apple Music song/playlist downloader.

> **Verification status:** Basic AAC cookie downloads and sync to a real iPod have not yet been verified end-to-end with a real account or device for this release. Try it with scratch folders before pointing it at a library or iPod you care about.

## Requirements

The iPod must run firmware that browses files, such as Rockbox. This app copies audio files flat into the device's `Music/` folder and does not write an iTunesDB; stock Apple firmware will not show synced tracks.

## Features

- Scan a local library and an iPod, preview the plan, then sync after you confirm.
- Flat layout in the device's `Music/` folder with a manifest and resumable scans.
- Dry-run mode that scans and prints the plan without writing anything.
- Apple Music song and playlist downloads through [gamdl](https://github.com/glomatico/gamdl): Basic AAC with a browser sign-in and cookie import, no Docker.
- Optional Advanced ALAC downloads through a Docker backend you build yourself.
- Downloaded audio is checked with `ffprobe` and a full `ffmpeg` decode before it is saved.

## Status and platform support

| Platform | Archive | Status |
|---|---|---|
| Linux x86_64 | `ipod-connection-utility-x86_64-unknown-linux-gnu.tar.gz` | Tested in development |
| Linux ARM64 (incl. Asahi, Arch Linux ARM) | `ipod-connection-utility-aarch64-unknown-linux-gnu.tar.gz` | Tested under emulation; Asahi hardware not yet accepted. Binaries are 64 KiB aligned for 4 KiB and 16 KiB page kernels. |
| macOS, Apple Silicon | `ipod-connection-utility-aarch64-apple-darwin.tar.gz` | Built and smoke-tested in CI only |
| macOS, Intel | n/a | Not supported |
| Windows | n/a | Not supported (POSIX only; WSL untested) |

Checks so far are automated tests with synthetic cookies and scratch folders; authenticated end-to-end downloads and real-device sync have not been verified for this release.

## Install

1. Download your archive and its `.sha256` file from the [GitHub Releases](https://github.com/Mimmob07/ipod_connection_utility/releases) page (the release also has a combined `SHA256SUMS`).
2. Verify the checksum from the download directory:
   - Linux: `sha256sum -c <archive>.sha256`
   - macOS: `shasum -a 256 -c <archive>.sha256`
3. Extract the whole archive: `tar -xzf <archive>`.
4. **macOS only:** the binaries are not signed or notarized. Clear the quarantine flag first:
   `xattr -dr com.apple.quarantine <extracted-directory>`
5. Run the installer from the extracted directory: `bash ./Install.sh`. Do not run it with `sudo`.

The installer offers Basic AAC (app plus downloader), Sync-only, or Advanced (ALAC). The default prefix is `~/.local`; launch with `icu`. See [QUICKSTART.md](QUICKSTART.md) for the full walkthrough, including upgrade and uninstall.

## Usage

Run `icu` (or `ipod-connection-utility`) in a terminal to open the TUI. Nothing is initialized or synced when it opens.

Main TUI keys (press `?` for the full list):

| Key | Action |
|---|---|
| `1`-`4` | Switch tabs |
| `l` / `p` | Set the library folder / iPod path |
| `i` | Initialize the data file |
| `s` | Scan |
| `a` | Review and sync (asks for confirmation) |
| `r` | Reload |
| `j`/`k`, arrows, `g`/`G` | Scroll |
| `q` | Quit |

Command line:

```text
Terminal iPod library sync and Apple Music download utility

Usage: ipod-connection-utility [OPTIONS]

Options:
  -i, --initialize               Initialize the data.json file, run this the first time you use the program
  -l, --local-path <LOCAL_PATH>  Music library folder path
  -p, --path <PATH>              iPod mount or Music folder. Required for noninteractive sync; in the TUI it can be entered later with p
  -n, --no-tui                   Sync right away without opening the interface, this is also what happens when the program is not run from a terminal
      --dry-run                  Scan both libraries and print the plan without copying, deleting or saving anything
  -h, --help                     Print help
  -V, --version                  Print version
```

```sh
# Preview only
icu --dry-run -l ~/Music -p /path/to/ipod
# Sync without the TUI (--path is mandatory here)
icu -n -l ~/Music -p /path/to/ipod
```

### Safety model

- It never syncs automatically. You scan, review the plan, then confirm.
- The local library is the source of truth. This is not a bidirectional backup.
- Noninteractive or piped sync and dry-run require an explicit `--path`, even if the TUI saved one. There is no default writable device target.
- A device mount root resolves to its `Music/` folder, so firmware and non-audio files are left alone.
- The flat layout keeps one recording per case-insensitive filename. Distinct recordings that share a filename must be renamed.

## Apple Music downloads

**Basic AAC (default, no Docker).** Uses gamdl on the host. In the Downloader tab:

- `o` opens Apple Music in your browser. Sign in and complete 2FA there.
- Export your Apple Music cookies in Netscape `cookies.txt` format, then press `c` to import the file.
- `n` queues a song or playlist URL, `t` retries failed entries, `v` selects quality, `x` removes the imported web session, `d` runs dependency setup/repair.

Cookies stay on your machine; the app stores only the session credential it needs. Treat cookie files as account credentials and never paste them, passwords, or 2FA codes into issues or chat. Expired sessions need a fresh sign-in and import. Download queues are in memory and do not survive a restart.

**Advanced ALAC (optional).** Needs Docker and an Apple Music Android package (APKM) that you supply yourself; it is not distributed here. The backend is built locally from glomatico/wrapper-v2 plus this project's patch, and it can be slow. See [DOWNLOADER_SETUP.md](DOWNLOADER_SETUP.md) and [backend/README.md](backend/README.md).

## Build from source

Requires Rust 1.89+ and a C linker.

```sh
cargo build --release --locked
./target/release/ipod-connection-utility --help
```

Tests need a Python virtualenv with `mutagen` and `pillow`:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install mutagen pillow pytest
IPOD_PYTHON="$PWD/.venv/bin/python" cargo test --locked
```

[AGENTS.md](AGENTS.md) lists the main checks CI runs.

## Contributing

Issues and pull requests are welcome. Read [AGENTS.md](AGENTS.md) for the repository layout, verification commands, and invariants (for example: never commit APKM/`.so` files, cookies or session data; never log tokens).

## Disclaimer

This project is not affiliated with or endorsed by Apple. It is meant for personal use with your own Apple Music subscription. The downloader uses unofficial routes, not an Apple-supported export API. DRM and copyright laws vary by country, and you are responsible for complying with them and with Apple's terms.

## Credits

- [gamdl](https://github.com/glomatico/gamdl) by glomatico: installed at runtime into a private environment, not bundled.
- [wrapper-v2](https://github.com/glomatico/wrapper-v2) by glomatico (Unlicense): only a patch is distributed here; the upstream source is fetched at setup time.
- [ratatui](https://ratatui.rs) for the terminal UI.
- No Apple software is distributed.

## License

[MIT](LICENSE). Copyright Mimmob07.
