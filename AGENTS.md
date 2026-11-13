# Contributor guide

This Rust terminal application previews and syncs a local music library to an iPod, and downloads Apple Music songs through host-side `gamdl`; an optional Docker backend supplies Apple Music services.

## Repository layout

- `src/`: Rust application; `src/tui/`, `src/downloader/`, and `src/sync/` hold the UI, downloader, and sync code.
- `tools/`: Python helpers and their tests.
- `scripts/`: installer, setup, and packaging scripts.
- `tests/`: script-style integration and installer tests; `test_cli.py` and `test_lifecycle.py` are run with `--binary`, not under pytest.
- `backend/`: pinned upstream wrapper patch and patch/image validators.

## Build and verification

CI runs these commands:

```sh
cargo test --locked
cargo build --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
.venv/bin/python -m unittest discover -s tools -p 'test_*.py'
.venv/bin/python -m pytest tools/test_apple_web_session.py
python3 tests/test_package_release.py
python3 tests/test_installer_wizard.py
python3 tests/test_desktop_install.py
python3 tests/test_first_run.py
python3 tests/test_in_app_setup.py
python3 tests/test_basic_installer.py
python3 tests/test_upgrade_alias.py
python3 tests/test_legacy_cleanup.py
for script in scripts/*.sh tools/test_scripts.sh tools/test_setup_emulation.sh; do bash -n "$script"; done
bash tools/test_scripts.sh
bash tools/test_setup_emulation.sh
scripts/setup-backend.sh --help
scripts/install.sh --help
python3 tests/test_lifecycle.py --binary target/debug/ipod-connection-utility
python3 tests/test_cli.py --binary target/debug/ipod-connection-utility
```

Before `cargo test`, set `IPOD_PYTHON` to a virtualenv Python with `mutagen` and `pillow` installed. CI also validates the backend patch and tests its pinned upstream checkout on Linux. Installer tests must remain hermetic: use `IPOD_OS_RELEASE_FILE` and `IPOD_DEPENDENCY_PATH` fixtures, never real package managers.

## Invariants

- Never commit Apple APKM or `.so` files, `.wrapper-v2/` backend checkouts, session/account data, cookies, or built binaries.
- Never log tokens or the backend `/me` response.
- Do not sync automatically at startup; noninteractive sync requires explicit `--path`.
- Device and library writes use staged sibling files and atomic publication.
- Backend ports bind to loopback only.
- Keep backend runtime CPU, Android ABI, and host page size as separate profile choices.
- Reproduce the wrapper patch with `backend/validate-portable-patch.sh` against pinned upstream commit `100e0a864e883e03a3ac450a780dd9563fff5271`.
