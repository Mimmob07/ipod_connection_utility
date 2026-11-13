# Quickstart

Install iPod Connection Utility from a release archive, sign in to Apple Music
(optional), and sync a library to an iPod. Basic AAC cookie downloads and sync to
a real iPod have not yet been verified end-to-end with a real account or device
for this release; try it with scratch folders before pointing it at music you
care about.

## Requirements

The iPod must run firmware that browses files, such as Rockbox. This app copies
audio files flat into the device's `Music/` folder and does not write an
iTunesDB; stock Apple firmware will not show synced tracks.

## 1. Pick your archive

| Your computer | Archive |
|---|---|
| Linux, Intel/AMD 64-bit | `ipod-connection-utility-x86_64-unknown-linux-gnu.tar.gz` |
| Linux, ARM64 (including Asahi Linux and Arch Linux ARM) | `ipod-connection-utility-aarch64-unknown-linux-gnu.tar.gz` |
| macOS, Apple Silicon (M-series) | `ipod-connection-utility-aarch64-apple-darwin.tar.gz` |

Intel Macs are not supported (Homebrew no longer supports them).

The ARM64 Linux build works on both 4 KiB and 16 KiB page-size kernels.

## 2. Verify and extract

Keep the archive and its `.sha256` file together and run from that directory:

- Linux: `sha256sum -c <archive>.sha256`
- macOS: `shasum -a 256 -c <archive>.sha256`

Then extract the **whole** archive (`tar -xzf <archive>`). `Install.sh`, `bin/`
and `share/` must stay together.

**macOS only:** the binaries are not signed or notarized, so Gatekeeper blocks
them. Clear the quarantine flag on the extracted directory first:

```
xattr -dr com.apple.quarantine ipod-connection-utility-aarch64-apple-darwin
```

## 3. Install

- **Linux:** open `Install.sh` and choose Run/Execute, or in a terminal in the
  extracted directory run `bash ./Install.sh`.
- **macOS:** open Terminal in the extracted directory and run `bash ./Install.sh`.
  Double-clicking does not work on macOS.

The installer asks, in order:

1. **Setup type:** *Basic AAC* (default: app + Apple Music downloader, no
   Docker), *Sync-only* (no downloader, no Apple account, no extra
   dependencies), or *Advanced* (ALAC, see below).
2. **Install prefix:** default `~/.local`. Never run the installer with `sudo`.
3. **Dependency consent:** whether it may install what is missing. Basic needs
   Python 3.10+ (with `venv`) and `ffmpeg`/`ffprobe`:
   - Arch/Arch ARM: `pacman` (`python ffmpeg`)
   - Debian/Ubuntu: `apt` (`python3 python3-venv ffmpeg`)
   - Fedora: `dnf` (`python3 ffmpeg-free`)
   - macOS: `brew` (`python ffmpeg`). **Homebrew must already be installed**
     (https://brew.sh); the installer will not install it.

   Linux package installs use `sudo` and ask for your password. Basic setup
   also downloads Python packages (`gamdl`) into a private environment under
   your user data directory, so it needs internet access.
4. **Leftovers** from older installs, if any, with a prompt to remove them.
   Music, settings and sign-ins are kept.
5. **Confirmation**, then an optional "launch now".

## 4. Launch

- Type `icu` (or `ipod-connection-utility`). If that is not found, add
  `~/.local/bin` (your prefix's `bin/`) to `PATH`. This is common on macOS.
- Linux also gets an **iPod Connection Utility** entry in the app menu.

## First run

- Choose your local music folder when asked (**l** changes it later). Press **p**
  to set the iPod mount point.
- Launching never initializes or syncs by itself. Scan, **review the changes,
  then confirm sync**. The local library is the source of truth; this is not a
  two-way backup.
- Rename tracks that differ only by letter case before syncing; the iPod layout
  cannot hold two such names.

## Apple Music sign-in (Basic AAC)

You sign in on Apple's own website, in your own browser. The app never sees your
password or 2FA code.

1. On the **Downloader** tab press **o** to open https://music.apple.com and
   sign in.
2. While on that site, export cookies as **Netscape `cookies.txt`** with a
   browser extension of your choice (current site only). Such extensions can
   read sensitive sessions, so review their permissions.
3. Press **c** and enter the file's path (drag-and-drop into the terminal,
   quotes, `file://` and `~/` all work).
4. The app keeps only the Apple Music `media-user-token`, stores it privately on
   your machine, and checks your subscription. Other cookies in the file are
   ignored. Nothing is uploaded. Delete the exported file afterwards.
5. Press **n** to queue one song and listen to it before queuing more.

Opening the browser does not import anything by itself. If the session
expires, repeat the export and import. **x** forgets the imported session.
Never share cookies, tokens or passwords in bug reports.

This relies on unofficial Apple Music web APIs, not an Apple-supported export
feature; review Apple's terms and the law where you live.

## Advanced ALAC (optional)

Press **u** on the Downloader tab, or choose *Advanced* in the installer. It
needs Docker, a source build of a helper backend, and an Apple Music APKM that
you supply yourself. It is slow and not needed for AAC. Docker group access on
Linux is root-equivalent; read each consent prompt.

## Upgrade and uninstall

- **Upgrade:** extract the newer archive and run `Install.sh` again with the
  same prefix. Settings, cookies, music and device files are left alone.
- **Uninstall** (no uninstall command yet), using your prefix if not `~/.local`:

  ```
  [ "$(readlink ~/.local/bin/icu)" = ipod-connection-utility ] && rm ~/.local/bin/icu   # only our alias
  rm -f ~/.local/bin/ipod-connection-utility*
  rm -rf ~/.local/share/ipod-connection-utility
  rm -f ~/.local/share/applications/ipod-connection-utility.desktop   # Linux
  ```

  The downloader environment and saved session live in
  `~/.local/share/ipod-connection-utility/` on Linux and
  `~/Library/Application Support/ipod-connection-utility/` on macOS; deleting
  that directory removes them too (and, on Linux with the default prefix, it is
  the same directory as the second line above). Your music and iPod are never
  touched.

## Troubleshooting

Rerun the installer with the same prefix after fixing whatever the error names.
`ipod-connection-utility-install --dry-run --basic` shows what would change
without changing anything.
