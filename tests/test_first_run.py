#!/usr/bin/env python3
"""First-run and remembered-folder checks against the real terminal app."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest

BINARY = Path('target/debug/ipod-connection-utility').resolve()

class Paths(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ipod-paths-', dir=os.environ.get('TMPDIR'))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / 'home'
        self.home.mkdir()
        self.library = self.root / 'chosen music'
        self.library.mkdir()
        self.device = self.root / 'device'
        self.device.mkdir()
        self.settings = self.root / 'data/ipod-connection-utility/settings.json'
        self.env = {**os.environ, 'HOME': str(self.home), 'XDG_DATA_HOME': str(self.root / 'data'), 'TERM': 'xterm-256color'}
        self.child = None

    def tearDown(self):
        if self.child and self.child.poll() is None:
            self.child.kill()
            self.child.wait()
        if hasattr(self, 'master'):
            os.close(self.master)

    def start(self, *args):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 35, 320, 0, 0))
        self.child = subprocess.Popen([str(BINARY), *args], stdin=slave, stdout=slave, stderr=slave, cwd=self.root, env=self.env)
        os.close(slave)
        self.output = b''

    def pump(self, done):
        """Keep reading until done(): the app blocks once the pty buffer fills, and
        macOS pty buffers hold less than one full frame."""
        deadline = time.monotonic() + 4
        while not done() and time.monotonic() < deadline:
            readable, _, _ = select.select([self.master], [], [], .05)
            if readable:
                try:
                    self.output += os.read(self.master, 65536)
                except OSError:
                    break
                if b'\x1b[6n' in self.output:
                    os.write(self.master, b'\x1b[1;1R')
        return done()

    def wait_text(self, text):
        def found():
            clean = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', self.output)
            return b''.join(text.encode().split()) in b''.join(clean.split())
        if not self.pump(found):
            self.fail(f'{text!r} not in terminal output: {self.output[-1500:]!r}')

    def quit(self, key=b'q'):
        os.write(self.master, key)
        assert self.child is not None
        self.pump(lambda: self.child.poll() is not None)
        self.assertEqual(self.child.wait(timeout=1), 0)

    def seed(self, value):
        self.settings.parent.mkdir(parents=True)
        self.settings.write_text(json.dumps(value))

    def test_first_launch_asks_for_library_then_remembers_selection(self):
        self.start()
        self.wait_text('Local library path')
        os.write(self.master, str(self.library).encode() + b'\r')
        self.assertTrue(self.pump(self.settings.exists), 'selected path was not saved')
        # Saved folders are canonical; macOS TMPDIR is under the /var symlink.
        self.assertEqual(json.loads(self.settings.read_text())['local_path'], str(self.library.resolve()))
        self.assertFalse((self.library / 'data.json').exists(), 'first launch must not initialize music')
        self.quit()

    def test_saved_paths_are_shown_on_launch(self):
        self.seed({'local_path': str(self.library), 'ipod_path': str(self.device)})
        self.start()
        self.wait_text(str(self.library))
        self.wait_text(str(self.device))
        self.quit()

    def test_explicit_interactive_paths_override_saved_values(self):
        self.seed({'local_path': '/must-not-use', 'ipod_path': '/must-not-use-either'})
        self.start('-l', str(self.library), '-p', str(self.device))
        self.wait_text(str(self.library))
        self.wait_text(str(self.device))
        self.quit()

    def test_saved_device_never_authorizes_noninteractive_sync(self):
        self.seed({'local_path': str(self.library), 'ipod_path': str(self.device)})
        before = self.settings.read_bytes()
        for mode in ([], ['--dry-run'], ['--no-tui']):
            result = subprocess.run([str(BINARY), *mode], env=self.env, cwd=self.root, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('--path is required', result.stderr)
        self.assertEqual(self.settings.read_bytes(), before)
        self.assertEqual(list(self.device.iterdir()), [])

    def test_malformed_config_returns_to_first_run(self):
        self.settings.parent.mkdir(parents=True)
        self.settings.write_text('broken json')
        self.start()
        self.wait_text('Local library path')
        self.quit(b'\x03')

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, default=BINARY)
    options, args = parser.parse_known_args()
    BINARY = options.binary.resolve()
    unittest.main(argv=[sys.argv[0], *args])
