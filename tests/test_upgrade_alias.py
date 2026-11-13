"""Safe app upgrade cleanup and the installed `icu` alias."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class UpgradeAliasTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="icu upgrade '")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.pkg = self.root / 'release with spaces' / 'ipod-connection-utility'
        (self.pkg / 'bin').mkdir(parents=True)
        (self.pkg / 'share/ipod-connection-utility').mkdir(parents=True)
        app = self.pkg / 'bin/ipod-connection-utility'
        app.write_text('#!/bin/sh\nprintf "app:%s\\n" "$*"\n')
        app.chmod(0o755)
        for name in ('install.sh', 'setup-backend.sh'):
            shutil.copy2(ROOT / 'scripts' / name, self.pkg / 'bin' / f'ipod-connection-utility-{name.removesuffix(".sh")}')
        for name in ('wrapper-portable.patch',):
            (self.pkg / 'share/ipod-connection-utility' / name).write_text('fixture')
        (self.pkg / 'share/ipod-connection-utility/README.md').write_text('fixture')
        (self.pkg / 'share/ipod-connection-utility/DOWNLOADER_SETUP.md').write_text('fixture')
        self.prefix = self.root / 'home with spaces' / '.local'
        self.home = self.root / 'home with spaces'
        self.env = dict(os.environ, HOME=str(self.home), XDG_DATA_HOME=str(self.root / 'xdg data'))

    def install(self):
        return subprocess.run(['bash', str(self.pkg / 'bin/ipod-connection-utility-install'),
                               '--prefix', str(self.prefix), '--sync-only'],
                              env=self.env, capture_output=True, text=True)

    def test_install_and_reinstall_creates_launchable_icu_alias(self):
        for _ in range(2):
            result = self.install()
            self.assertEqual(result.returncode, 0, result.stderr)
            alias = self.prefix / 'bin/icu'
            self.assertTrue(alias.is_symlink())
            self.assertEqual(alias.resolve(), (self.prefix / 'bin/ipod-connection-utility').resolve())
            self.assertEqual(subprocess.check_output([str(alias), '--version'], text=True).strip(), 'app:--version')

    def test_unrelated_icu_collision_is_preserved_and_does_not_block_upgrade(self):
        bindir = self.prefix / 'bin'
        bindir.mkdir(parents=True)
        collision = bindir / 'icu'
        collision.write_text('user command')
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(collision.read_text(), 'user command')
        self.assertFalse(collision.is_symlink())
        self.assertIn('icu', result.stdout.lower())

    def test_user_replacement_of_recorded_alias_survives_reinstall(self):
        self.assertEqual(self.install().returncode, 0)
        alias = self.prefix / 'bin/icu'
        alias.unlink()
        alias.write_text('user command')
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(alias.read_text(), 'user command')

    def test_user_data_and_unrelated_bin_files_survive_upgrade(self):
        data = self.root / 'xdg data/ipod-connection-utility/web-auth/cookies.txt'
        data.parent.mkdir(parents=True)
        data.write_text('synthetic-cookie-data')
        bindir = self.prefix / 'bin'
        bindir.mkdir(parents=True)
        (bindir / 'my-tool').write_text('keep')
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(data.read_text(), 'synthetic-cookie-data')
        self.assertEqual((bindir / 'my-tool').read_text(), 'keep')


if __name__ == '__main__':
    unittest.main()
