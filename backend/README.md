# Backend source and patch layers

The app uses `glomatico/wrapper-v2` upstream commit
`100e0a864e883e03a3ac450a780dd9563fff5271` (the pinned tree contains an
Unlicense/public-domain dedication).
The large working checkouts, Android/Apple binaries, APKM, account state, logs,
and generated executables are intentionally not part of this repository.
Only a source patch is distributed; no binaries or account data.

## Portable profiles

`patches/wrapper-portable.patch` contains:

- Loopback-only HTTP/decrypt publishing, defaulting to 8080/10020.
- App-owned restart/stop policy and build-context exclusions.
- Seven-string/eight-string Apple persistent-key ABI compatibility and optional
  loader mapping diagnostics.
- A supervisor build stage selected from the Docker runtime CPU, independent of
  the Android worker ABI.
- Native amd64 and ARM64 final stages plus a conditional packaged-QEMU x86_64
  worker path for large-page ARM Linux.
- The large-host-page `madvise` guard, disabled by default and enabled with the
  actual Docker Linux page size only for the compatibility profile.
- Profile-specific image tags, explicit runtime platforms, and external session
  data paths.
- SIGTERM/SIGINT supervisor shutdown with worker termination/reaping, and build
  context exclusions for account state and local environment files.

A **fresh** checkout can be
reconstructed from the outer repository root with:

```sh
git clone https://github.com/glomatico/wrapper-v2.git .wrapper-v2
git -C .wrapper-v2 checkout --detach 100e0a864e883e03a3ac450a780dd9563fff5271
git -C .wrapper-v2 apply ../backend/patches/wrapper-portable.patch
```

The recommended path automates clone, profile isolation,
patching, staging, verification, image build, and Python setup:

```sh
./scripts/setup-backend.sh --apkm /path/to/apple-music.apkm
```

Do not reapply the manual commands over an existing checkout containing
uncommitted work. The setup tool uses a different checkout and image tag for
each profile, so changing ABI cannot leave stale files in the selected rootfs.
If an older portable patch or local edits are present, setup stops instead of
overwriting them. Use a fresh `--backend-dir`; setup saves that path for the app
and preserves the existing profile's external session directory.
Consult [DOWNLOADER_SETUP.md](../DOWNLOADER_SETUP.md#backend) for the distinction between Android ABI,
Docker runtime CPU, and Docker Linux page size.

In the app, `src/wrapper.rs` resolves the
saved/detected profile as one atomic tuple and applies the same image, worker,
runtime, page-size, port, project, and state settings to Compose start and stop.
Installed backends use XDG data when set; the fallback is `~/.local/share` on
Linux or Application Support on macOS. A source-tree `.wrapper-v2` remains a development fallback. Native
runtime paths have limited live login/decrypt/audio testing.

`validate-portable-patch.sh` checks fresh reconstruction against a patched
reference, compiles/runs the page-guard test, and checks build-context exclusions
when Docker Buildx is available. `validate-portable-images.sh` inspects all
three already-built images and checks worker IPC health plus container exit 0
after SIGTERM. The image validator requires a local Linux Docker Engine and
`readelf`, `curl`, and `jq`; it uses an internal network without outbound access.
Set `IPOD_VALIDATION_PENDING_LOGIN=1` for the dummy pending-login stop case and
`IPOD_VALIDATION_ASAHI_PAGE_SIZE=16384` to explicitly simulate that guard setting
on a 4096-byte kernel. Simulation does not validate an actual large-page host.
Image names can be overridden with `IPOD_VALIDATION_AMD64_IMAGE`,
`IPOD_VALIDATION_ARM64_IMAGE`, and `IPOD_VALIDATION_ASAHI_IMAGE`.

## The `asahi-16k` profile

The profile uses an ARM64 supervisor with an x86_64 Android worker run through
packaged QEMU, and enables the `madvise` page-size guard. A native-ARM worker is
not a known-good runtime on these hosts. Large-page hardware testing of this
profile is still pending.
