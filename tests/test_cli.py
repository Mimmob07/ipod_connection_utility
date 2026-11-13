#!/usr/bin/env python3
"""Test installed CLI behavior on scratch libraries, optionally through QEMU."""
import argparse
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest


class CommandLine(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ipod-cli-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.library = self.root / "library with spaces"
        self.music = self.root / "device/Music"
        self.library.mkdir()
        self.music.mkdir(parents=True)
        (self.library / "song.m4a").write_bytes(b"scratch audio content")
        (self.music / "orphan.mp3").write_bytes(b"scratch orphan")
        (self.music.parent / "firmware.bin").write_bytes(b"preserve firmware")
        (self.music / "playlist.m3u").write_text("song.m4a\n")
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        fake_docker = fake_bin / "docker"
        fake_docker.write_text("#!/bin/sh\nexit 93\n")
        fake_docker.chmod(0o755)
        self.env = {**os.environ, "PATH": str(fake_bin), "XDG_DATA_HOME": str(self.root / "data")}

    def run_cli(self, *args, succeeds=True):
        result = subprocess.run([*RUNNER, str(BINARY), *args], env=self.env,
                                capture_output=True, text=True, timeout=30)
        if succeeds:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def snapshot(self):
        return {str(path.relative_to(self.root)): path.read_bytes()
                for path in self.root.rglob("*") if path.is_file()}

    def test_explicit_target_is_required_for_piped_dry_run_and_sync(self):
        before = self.snapshot()
        for mode in ([], ["--dry-run"], ["--no-tui"]):
            result = self.run_cli(*mode, "-l", str(self.library), succeeds=False)
            self.assertIn("--path is required", result.stderr)
        self.assertEqual(before, self.snapshot())

    def test_uninitialized_library_explains_initialize(self):
        before = self.snapshot()
        for mode in ("--dry-run", "--no-tui"):
            result = self.run_cli(mode, "-l", str(self.library), "-p", str(self.music.parent), succeeds=False)
            self.assertIn("run with --initialize first", result.stderr)
            self.assertNotIn("Os {", result.stderr)
        self.assertEqual(before, self.snapshot())

    def test_initialize_preview_and_sync_without_docker(self):
        self.run_cli("--initialize", "-l", str(self.library))
        self.assertTrue((self.library / "data.json").is_file())
        before = self.snapshot()
        preview = self.run_cli("--dry-run", "-l", str(self.library), "-p", str(self.music.parent))
        self.assertIn("1 to add, 0 to update, 1 to delete", preview.stdout)
        self.assertEqual(before, self.snapshot(), "Dry-run must leave all files unchanged")
        self.run_cli("--no-tui", "-l", str(self.library), "-p", str(self.music.parent))
        self.assertEqual((self.music / "song.m4a").read_bytes(),
                         (self.library / "song.m4a").read_bytes())
        self.assertFalse((self.music / "orphan.mp3").exists())
        self.assertEqual((self.music.parent / "firmware.bin").read_bytes(), b"preserve firmware")
        self.assertTrue((self.music / "playlist.m3u").is_file())
        preview = self.run_cli("--dry-run", "-l", str(self.library), "-p", str(self.music))
        self.assertIn("0 to add, 0 to update, 0 to delete", preview.stdout)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/ipod-connection-utility"))
    parser.add_argument("--runner", default="", help="Optional emulator command, e.g. qemu-aarch64-static -L /usr/aarch64-linux-gnu")
    options, remaining = parser.parse_known_args()
    BINARY = options.binary.resolve()
    RUNNER = shlex.split(options.runner)
    if not BINARY.is_file():
        parser.error(f"Build the app first; binary does not exist: {BINARY}")
    unittest.main(argv=[sys.argv[0], *remaining])
