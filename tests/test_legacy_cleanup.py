"""Upgrade cleanup: legacy files, old ALAC backend, and installer manifest.

Hermetic: isolated HOME/XDG/CARGO_HOME, a logging fake `docker`; never touches
the host Docker daemon, real installs, or user data.
"""
import os
from pathlib import Path
import pty
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CLEANUP = ROOT / 'scripts/cleanup-legacy.sh'

FAKE_DOCKER = r'''#!/usr/bin/env bash
printf 'docker %s | cwd=%s\n' "$*" "$PWD" >> "$CALLS"
[[ "${DOCKER_DOWN:-0}" == 1 ]] && exit 1
case "$1 ${2:-}" in
  "info "*) exit 0 ;;
  "image ls")
    if [[ "$*" == *'{{.Size}}'* ]]; then echo 1.2GB; else printf '%s\n' ${IMAGES:-}; fi ;;
  "image rm") exit 0 ;;
  "compose down") exit 0 ;;
  *) echo "unexpected docker call: $*" >&2; exit 97 ;;
esac
'''


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="icu cleanup '")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.home = self.base / 'home dir'
        self.data = self.base / 'xdg data' / 'ipod-connection-utility'
        self.cargo = self.base / 'cargo home'
        self.prefix = self.home / '.local'
        self.tools = self.base / 'tools'
        for d in (self.home, self.data, self.tools, self.prefix / 'bin'):
            d.mkdir(parents=True, exist_ok=True)
        self.calls = self.base / 'calls'
        self.calls.touch()
        docker = self.tools / 'docker'
        docker.write_text(FAKE_DOCKER)
        docker.chmod(0o755)
        # Never run the host's real cargo against fixtures.
        cargo = self.tools / 'cargo'
        cargo.write_text('#!/usr/bin/env bash\nprintf "cargo %s\\n" "$*" >> "$CALLS"\nexit 1\n')
        cargo.chmod(0o755)
        self.env = dict(os.environ, HOME=str(self.home), XDG_DATA_HOME=str(self.base / 'xdg data'),
                        CARGO_HOME=str(self.cargo), CALLS=str(self.calls),
                        PATH=f"{self.tools}{os.pathsep}/usr/bin{os.pathsep}/bin")
        self.env.pop('IPOD_INSTALLER_HOLD', None)
        # User data that must survive every cleanup.
        self.keep = {
            self.data / 'web-auth/cookies.txt': 'synthetic web session',
            self.data / 'wrapper-data/asahi-16k/session.bin': 'synthetic wrapper session',
            self.data / 'settings.json': '{}',
            self.data / 'gamdl/bin/gamdl': 'current private venv',
        }
        for path, text in self.keep.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)

    def assert_user_data_kept(self):
        for path, text in self.keep.items():
            self.assertEqual(path.read_text(), text, path)

    def cleanup(self, *args, **env):
        return subprocess.run(['bash', str(CLEANUP), '--prefix', str(self.prefix), *args],
                              env={**self.env, **env}, capture_output=True, text=True, timeout=30)

    def make_legacy(self):
        venv = self.home / '.local/share/ipod-gamdl'
        (venv / 'lib/python3.12/site-packages/gamdl').mkdir(parents=True)
        (venv / 'pyvenv.cfg').write_text('home = /usr/bin\n')
        (self.cargo / 'bin').mkdir(parents=True)
        old_bin = self.cargo / 'bin/ipod_connection_utility'
        old_bin.write_text('old binary')
        (self.cargo / '.crates.toml').write_text(
            '[v1]\n"ipod_connection_utility 0.1.0 (path+file:///src)" = ["ipod_connection_utility"]\n')
        (self.cargo / 'bin/other-tool').write_text('unrelated cargo tool')
        return venv, old_bin

    def make_backend(self, profile='asahi-16k', with_data=False):
        checkout = self.data / 'backends' / profile
        checkout.mkdir(parents=True)
        (checkout / 'compose.yaml').write_text('services: {}\n')
        if with_data:
            (checkout / 'data').mkdir()
            (checkout / 'data/session').write_text('old in-checkout session')
        (self.data / 'backend-profile').write_text(profile)
        return checkout

    def docker_calls(self):
        return [l for l in self.calls.read_text().splitlines() if l.startswith('docker ')]


class LegacyCleanupTests(Fixture):
    def test_clean_machine_scan_finds_nothing_and_never_calls_docker(self):
        result = self.cleanup('--scan')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('No leftovers', result.stdout)
        self.assertEqual(self.docker_calls(), [])

    def test_legacy_files_detected_and_removed_but_unrelated_files_kept(self):
        venv, old_bin = self.make_legacy()
        unrelated_venv = self.home / '.local/share/other-venv'
        unrelated_venv.mkdir(parents=True)
        (unrelated_venv / 'pyvenv.cfg').write_text('x')
        scan = self.cleanup('--scan')
        self.assertIn(f'LEGACY\t{venv}\t', scan.stdout)
        self.assertIn(f'LEGACY\t{old_bin}\t', scan.stdout)
        self.assertNotIn('other-venv', scan.stdout)
        self.assertTrue(venv.exists() and old_bin.exists(), 'scan must not delete')
        result = self.cleanup('--remove-legacy-files')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(venv.exists())
        self.assertFalse(old_bin.exists())
        self.assertTrue((self.cargo / 'bin/other-tool').exists())
        self.assertTrue(unrelated_venv.exists())
        self.assert_user_data_kept()
        self.assertEqual(self.docker_calls(), [])

    def test_cargo_binary_without_cargo_ownership_record_is_not_touched(self):
        (self.cargo / 'bin').mkdir(parents=True)
        stranger = self.cargo / 'bin/ipod_connection_utility'
        stranger.write_text('not recorded by cargo')
        self.assertNotIn('LEGACY', self.cleanup('--scan').stdout)
        self.cleanup('--remove-legacy-files')
        self.assertTrue(stranger.exists())

    def test_old_backend_removed_with_containers_and_images_but_sessions_kept(self):
        checkout = self.make_backend()
        images = 'wrapper-v2:asahi-16k wrapper-v2:native-arm64'
        scan = self.cleanup('--scan', IMAGES=images)
        self.assertIn(f'BACKEND\t{checkout}\t', scan.stdout)
        self.assertIn('Docker image wrapper-v2:asahi-16k', scan.stdout)
        self.assertTrue(checkout.exists())
        result = self.cleanup('--remove-old-backend', IMAGES=images)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.docker_calls()
        self.assertIn(f'docker compose down --remove-orphans | cwd={checkout}', calls)
        self.assertIn('docker image rm wrapper-v2:asahi-16k | cwd=' + os.getcwd(), calls)
        self.assertIn('docker image rm wrapper-v2:native-arm64 | cwd=' + os.getcwd(), calls)
        joined = '\n'.join(calls)
        for forbidden in (' -v', '--volumes', '--force', ' -f ', 'prune', 'system'):
            self.assertNotIn(forbidden, joined)
        self.assertFalse(checkout.exists())
        self.assertFalse((self.data / 'backend-profile').exists())
        self.assert_user_data_kept()

    def test_backend_checkout_holding_session_data_is_kept(self):
        checkout = self.make_backend(with_data=True)
        result = self.cleanup('--remove-old-backend')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((checkout / 'data/session').read_text(), 'old in-checkout session')
        self.assertIn('Kept', result.stderr)

    def test_unreachable_docker_deletes_nothing(self):
        checkout = self.make_backend()
        result = self.cleanup('--remove-old-backend', DOCKER_DOWN='1')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Nothing was deleted', result.stderr)
        self.assertTrue((checkout / 'compose.yaml').exists())
        self.assertTrue((self.data / 'backend-profile').exists())

    def test_requires_an_action(self):
        self.assertEqual(self.cleanup().returncode, 2)


class InstallerManifestTests(Fixture):
    def setUp(self):
        super().setUp()
        self.pkg = self.base / 'release dir' / 'pkg'
        (self.pkg / 'bin').mkdir(parents=True)
        share = self.pkg / 'share/ipod-connection-utility'
        share.mkdir(parents=True)
        app = self.pkg / 'bin/ipod-connection-utility'
        app.write_text('#!/bin/sh\nprintf "app:%s\\n" "$*"\n')
        app.chmod(0o755)
        for src, name in [('install.sh', 'install'), ('setup-backend.sh', 'setup-backend'),
                          ('cleanup-legacy.sh', 'cleanup'), ('install-wizard.sh', 'install-wizard')]:
            shutil.copy2(ROOT / 'scripts' / src, self.pkg / 'bin' / f'ipod-connection-utility-{name}')
        for name in ('wrapper-portable.patch', 'README.md', 'DOWNLOADER_SETUP.md'):
            (share / name).write_text('fixture')

    def install(self, *args):
        return subprocess.run(['bash', str(self.pkg / 'bin/ipod-connection-utility-install'),
                               '--prefix', str(self.prefix), '--sync-only', *args],
                              env=self.env, capture_output=True, text=True, timeout=60)

    def test_reinstall_removes_files_dropped_by_new_version_only(self):
        self.assertEqual(self.install().returncode, 0)
        manifest = self.prefix / 'share/ipod-connection-utility/installed-files'
        recorded = manifest.read_text().split()
        self.assertIn('bin/ipod-connection-utility', recorded)
        self.assertIn('bin/icu', recorded)
        self.assertIn('bin/ipod-connection-utility-cleanup', recorded)
        # Simulate an older version that shipped a helper this version dropped.
        dropped = self.prefix / 'bin/ipod-connection-utility-old-helper'
        dropped.write_text('obsolete')
        outside = self.base / 'outside-target'
        outside.write_text('must survive')
        user_file = self.prefix / 'bin/my-script'
        user_file.write_text('user')
        manifest.write_text(manifest.read_text() + 'bin/ipod-connection-utility-old-helper\n'
                            + '../../outside-target\nbin/../../outside-target\nshare/other/x\n')
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(dropped.exists())
        self.assertIn('Removed obsolete file', result.stdout)
        self.assertEqual(outside.read_text(), 'must survive')
        self.assertEqual(user_file.read_text(), 'user')
        self.assertTrue((self.prefix / 'bin/icu').is_symlink())
        self.assertNotIn('old-helper', manifest.read_text())
        self.assert_user_data_kept()

    def test_cleanup_flags_run_after_successful_install(self):
        venv, old_bin = self.make_legacy()
        result = self.install('--cleanup-legacy')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(venv.exists())
        self.assertFalse(old_bin.exists())
        self.assert_user_data_kept()

    def test_plain_install_never_removes_legacy_or_backend(self):
        venv, _ = self.make_legacy()
        checkout = self.make_backend()
        self.assertEqual(self.install().returncode, 0)
        self.assertTrue(venv.exists())
        self.assertTrue(checkout.exists())
        self.assertEqual(self.docker_calls(), [])

    def test_remove_old_backend_conflicts_with_advanced_setup(self):
        apkm = self.base / 'fake.apkm'
        apkm.write_text('x')
        result = self.install('--remove-old-backend', '--apkm', str(apkm))
        self.assertNotEqual(result.returncode, 0)

    def wizard(self, answers):
        cmd = ['bash', str(self.pkg / 'bin/ipod-connection-utility-install'), '--interactive']
        master, slave = pty.openpty()
        try:
            proc = subprocess.Popen(cmd, stdin=slave, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, text=True, env=self.env)
            os.close(slave)
            slave = None
            os.write(master, answers.encode())
            out, _ = proc.communicate(timeout=30)
            return proc.returncode, out
        finally:
            os.close(master)
            if slave is not None:
                os.close(slave)

    def test_wizard_offers_cleanup_with_recommended_defaults(self):
        venv, old_bin = self.make_legacy()
        checkout = self.make_backend()
        # sync-only, prefix, deps=n, legacy=<Enter: default yes>, backend=<Enter: default no>,
        # proceed=y, launch=n
        code, out = self.wizard(f'2\n{self.prefix}\nn\n\n\ny\nn\n')
        self.assertEqual(code, 0, out)
        self.assertIn('earlier install', out)
        self.assertIn('old advanced (ALAC) Docker backend', out)
        self.assertFalse(venv.exists(), 'legacy cleanup defaults to yes')
        self.assertFalse(old_bin.exists())
        self.assertTrue(checkout.exists(), 'backend removal defaults to no')
        self.assert_user_data_kept()

    def test_wizard_removes_backend_when_user_agrees(self):
        checkout = self.make_backend()
        code, out = self.wizard(f'2\n{self.prefix}\nn\ny\ny\nn\n')
        self.assertEqual(code, 0, out)
        self.assertFalse(checkout.exists())
        self.assert_user_data_kept()

    def test_wizard_shows_no_cleanup_prompts_on_clean_machine(self):
        code, out = self.wizard(f'2\n{self.prefix}\nn\ny\nn\n')
        self.assertEqual(code, 0, out)
        self.assertNotIn('earlier install', out)
        self.assertEqual(self.docker_calls(), [])
        self.assertIn('/icu', out, 'launch hint should name the short icu command')


if __name__ == '__main__':
    unittest.main()
