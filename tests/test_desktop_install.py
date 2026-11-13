"""Linux desktop launcher installation acceptance tests using isolated payloads."""
import ctypes
import ctypes.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux desktop integration")
class DesktopInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="desktop install '")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.home = self.base / "home"
        self.home.mkdir()
        self.prefix = self.base / "prefix with 'quote $ and %"
        self.pkg = self.base / "package"
        (self.pkg / "bin").mkdir(parents=True)
        (self.pkg / "scripts").mkdir()
        shutil.copy2(ROOT / "scripts/install.sh", self.pkg / "scripts/install.sh")
        self.calls = self.base / "app-calls"
        app = self.pkg / "bin/ipod-connection-utility"
        app.write_text('#!/usr/bin/env bash\nprintf "%s\\n" "$*" >> "$APP_CALLS"\n')
        app.chmod(0o755)
        self.env = dict(os.environ, HOME=str(self.home), APP_CALLS=str(self.calls),
                        PATH=os.environ["PATH"], XDG_DATA_HOME="")
        self.share = self.pkg / "share/ipod-connection-utility"
        self.share.mkdir(parents=True)
        for name in ("wrapper-portable.patch", "README.md", "DOWNLOADER_SETUP.md"):
            (self.share / name).write_text("fixture\n")
        setup = self.pkg / "scripts/ipod-connection-utility-setup-backend"
        setup.write_text("#!/bin/sh\nexit 0\n")
        setup.chmod(0o755)

    def install(self, *args):
        return subprocess.run(["bash", str(self.pkg / "scripts/install.sh"),
                               "--prefix", str(self.prefix), "--sync-only", *args],
                              text=True, capture_output=True, env=self.env)

    def test_sync_install_creates_terminal_desktop_entry_with_safe_exec(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        launcher = self.home / ".local/share/applications/ipod-connection-utility.desktop"
        self.assertTrue(launcher.is_file(), "Linux sync install should add menu launcher")
        text = launcher.read_text()
        self.assertIn("Type=Application", text)
        self.assertIn("Terminal=true", text)
        self.assertIn("Exec=", text)
        self.assertEqual((self.prefix / "bin/ipod-connection-utility").stat().st_mode & 0o111, 0o111)
        validator = shutil.which("desktop-file-validate")
        if validator:
            subprocess.run([validator, str(launcher)], check=True, capture_output=True, text=True)

    def test_xdg_data_home_is_used_and_exec_round_trips_special_path(self):
        xdg = self.base / "xdg data % $ '"
        xdg.mkdir()
        self.env["XDG_DATA_HOME"] = str(xdg)
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        launcher = xdg / "applications/ipod-connection-utility.desktop"
        self.assertTrue(launcher.is_file())
        text = launcher.read_text()
        exec_value = next(line[5:] for line in text.splitlines() if line.startswith("Exec="))
        self.assertIn('"', exec_value, "path argument containing whitespace must be quoted")
        self.assertIn("%%", exec_value, "literal percent signs must be escaped as %%")
        self.assertIn("\\$", exec_value, "dollars must be escaped against shell-like expansion")
        self.assertIn("'", exec_value, "apostrophes remain literal inside desktop quoting")
        self.assertNotIn("%F", exec_value, "a path percent must not become a field code")
        self.assertTrue(exec_value.startswith('/usr/bin/env -- "') and exec_value.endswith('"'))

    def test_desktop_exec_really_launches_literal_special_path(self):
        library = ctypes.util.find_library("gio-2.0")
        if not library:
            self.skipTest("GIO is unavailable")
        self.prefix = self.base / 'prefix "quote" \\slash $cash `tick` %F'
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        original = self.home / '.local/share/applications/ipod-connection-utility.desktop'
        launcher = self.base / 'launch-test.desktop'
        # Test the generated Exec through the real desktop parser/spawner without
        # requiring a GUI terminal in CI; production must retain Terminal=true.
        launcher.write_text(original.read_text().replace('Terminal=true', 'Terminal=false'))
        before = len(self.calls.read_text().splitlines())
        code = '''import ctypes,sys
lib=ctypes.CDLL(sys.argv[1])
lib.g_desktop_app_info_new_from_filename.argtypes=[ctypes.c_char_p]
lib.g_desktop_app_info_new_from_filename.restype=ctypes.c_void_p
lib.g_app_info_launch.argtypes=[ctypes.c_void_p,ctypes.c_void_p,ctypes.c_void_p,ctypes.c_void_p]
lib.g_app_info_launch.restype=ctypes.c_int
app=lib.g_desktop_app_info_new_from_filename(sys.argv[2].encode())
assert app, "desktop parser rejected Exec"
assert lib.g_app_info_launch(app,None,None,None), "desktop Exec failed to launch"
'''
        launched = subprocess.run([sys.executable, '-c', code, library, str(launcher)],
                                  env=self.env, capture_output=True, text=True)
        self.assertEqual(launched.returncode, 0, launched.stderr)
        deadline = time.monotonic() + 4
        while len(self.calls.read_text().splitlines()) == before and time.monotonic() < deadline:
            time.sleep(.02)
        self.assertGreater(len(self.calls.read_text().splitlines()), before)

    def test_dry_run_writes_neither_prefix_nor_desktop_file(self):
        result = self.install("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.prefix.exists())
        self.assertFalse((self.home / ".local/share/applications/ipod-connection-utility.desktop").exists())

    def test_linux_sync_install_does_not_call_package_or_docker_tools(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls.read_text()
        self.assertNotIn("setup-backend", calls)
        self.assertNotIn("install-dependencies", calls)
        self.assertTrue((self.home / ".local/share/applications/ipod-connection-utility.desktop").exists())


if __name__ == "__main__":
    unittest.main()
