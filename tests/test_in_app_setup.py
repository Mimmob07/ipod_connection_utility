#!/usr/bin/env python3
"""Exercise suspending and resuming the real TUI around a fake local installer."""
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time
import unittest

BINARY = Path('target/debug/ipod-connection-utility').resolve()

def rendered_text(data):
    # Ratatui sends differential cursor-addressed updates, not complete strings.
    cells = {}
    row = col = 0
    for part in re.findall(r'\x1b\[[0-?]*[ -/]*[@-~]|[^\x1b]+', data.decode('utf-8', 'replace')):
        if part.startswith('\x1b['):
            command = part[-1]
            args = part[2:-1]
            numbers = [int(x) if x.isdigit() else 0 for x in args.split(';')]
            if command in 'Hf':
                row = (numbers[0] or 1)-1
                col = ((numbers[1] if len(numbers)>1 else 1) or 1)-1
            elif command == 'J' and numbers[0] in (2,3): cells.clear()
            elif command == 'K':
                cells = {(r,c):v for (r,c),v in cells.items() if r != row or (numbers[0] == 0 and c < col) or (numbers[0] == 1 and c > col)}
            elif command == 'h' and args == '?1049': cells.clear()
            continue
        for char in part:
            if char == '\r': col = 0
            elif char == '\n': row += 1
            elif char == '\b': col = max(0,col-1)
            elif ord(char) >= 32:
                cells[row,col] = char
                col += 1
    return '\n'.join(''.join(cells.get((r,c),' ') for c in range(350)) for r in range(60))

class InAppSetup(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ipod-inapp-", dir=os.environ.get('TMPDIR'))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        binary_dir = self.root / "install prefix's/bin"
        binary_dir.mkdir(parents=True)
        binary = binary_dir / 'ipod-connection-utility'
        shutil.copy2(BINARY, binary)
        wizard = binary_dir / 'ipod-connection-utility-install-wizard'
        wizard.write_text('#!/bin/sh\nprintf "%s\\n" "$@" "no-launch=$IPOD_INSTALLER_NO_LAUNCH" > "$IPOD_SETUP_TEST_LOG"\nexit 7\n')
        wizard.chmod(0o755)
        self.log = self.root / 'setup-argv'
        self.library = self.root / 'library'
        self.library.mkdir()
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 35, 150, 0, 0))
        self.child = subprocess.Popen([str(binary), '-l', str(self.library)], stdin=slave, stdout=slave, stderr=slave,
            env={**os.environ, 'HOME': str(self.root), 'XDG_DATA_HOME': str(self.root/'data'), 'TERM': 'xterm-256color', 'IPOD_SETUP_TEST_LOG': str(self.log)})
        self.addCleanup(self.close_child)
        os.close(slave)
        self.output = b''
        self.wait_text('Overview')
        os.write(self.master, b'4')
        self.wait_text('Song downloader')

    def close_child(self):
        if self.child.poll() is None:
            self.child.kill()
            self.child.wait()
        os.close(self.master)

    def wait_text(self, text):
        until = time.monotonic()+4
        while time.monotonic()<until:
            ready,_,_=select.select([self.master],[],[],.05)
            if ready:
                try: self.output += os.read(self.master, 65536)
                except OSError: break
            if text in rendered_text(self.output): return
        self.fail(f'missing {text!r}: {self.output[-1000:]!r}')

    def test_advanced_setup_runs_only_after_confirmation_and_resumes_on_failure(self):
        os.write(self.master,b'u')
        self.wait_text('Install advanced')
        self.assertFalse(self.log.exists())
        os.write(self.master,b'y')
        until=time.monotonic()+4
        while not self.log.exists() and time.monotonic()<until: time.sleep(.02)
        self.assertTrue(self.log.exists(), 'confirmed setup never invoked installer')
        self.assertEqual(self.log.read_text().splitlines(),['--advanced','no-launch=1'])
        self.wait_text('Setup exited')
        self.assertIsNone(self.child.poll(), 'installer failure must not quit app')
        self.assertEqual(list(self.library.iterdir()), [])
        os.write(self.master,b'q')
        self.assertEqual(self.child.wait(timeout=4),0)

    def test_basic_setup_uses_basic_wizard_mode(self):
        os.write(self.master,b'd')
        self.wait_text('Set up basic')
        os.write(self.master,b'y')
        until=time.monotonic()+4
        while not self.log.exists() and time.monotonic()<until: time.sleep(.02)
        self.assertTrue(self.log.exists())
        self.assertEqual(self.log.read_text().splitlines(),['--basic','no-launch=1'])
        self.wait_text('Setup exited')
        os.write(self.master,b'q')
        self.assertEqual(self.child.wait(timeout=4),0)

if __name__ == '__main__': unittest.main()
