"""Hermetic basic downloader acceptance: no network/package/service commands."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
BASH = shutil.which('bash')
assert BASH is not None


class BasicSetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="basic install '")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.tools = self.base / 'tools'
        self.tools.mkdir()
        self.log = self.base / 'calls'
        self.env = dict(os.environ, HOME=str(self.base / 'home'),
                        PATH=str(self.tools), CALLS=str(self.log),
                        XDG_DATA_HOME=str(self.base / 'data space'))
        for name in ['mkdir', 'chmod', 'bash']:
            tool = shutil.which(name)
            assert tool is not None
            (self.tools / name).symlink_to(tool)
        self.script('uname', 'case "$1" in -s) echo "${TEST_OS:-Linux}" ;; -m) echo aarch64 ;; esac\n')
        for name in ['docker', 'git', 'jq', 'unzip', 'sudo', 'systemctl']:
            self.script(name, 'echo "FORBIDDEN:$0" >> "$CALLS"; exit 91\n')
        for name in ['ffmpeg', 'ffprobe']:
            self.script(name, 'exit 0\n')
        self.script('python3', '''printf 'python3:%s\\n' "$*" >> "$CALLS"
if [[ "$1 $2" == '-m venv' && "${3:-}" != --help ]]; then
    mkdir -p "$3/bin"
    printf '#!%s\\nprintf "venv:%%s\\\\n" "$*" >> "$CALLS"\\n' "$BASH" > "$3/bin/python"
    printf '#!%s\\nexit 0\\n' "$BASH" > "$3/bin/gamdl"
    chmod +x "$3/bin/python" "$3/bin/gamdl"
fi
''')

    def script(self, name, body):
        p = self.tools / name
        p.write_text('#!' + BASH + '\nset -eu\n' + body)
        p.chmod(0o755)

    def run_setup(self, *args):
        return subprocess.run([BASH, str(ROOT / 'scripts/setup-downloader.sh'), *args],
                              env=self.env, capture_output=True, text=True, input='')

    def test_private_pinned_environment_without_wrapper(self):
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.log.read_text()
        self.assertIn(str(self.base / 'data space/ipod-connection-utility/gamdl'), calls)
        self.assertIn('pip install gamdl==3.9.1 mutagen pillow', calls)
        self.assertNotIn('FORBIDDEN', calls)

    def test_dry_run_and_dependency_consent_never_call_wrapper_tools(self):
        shutil.copy2(ROOT / 'scripts/install-dependencies.sh',
                     self.tools / 'install-dependencies.sh')
        shutil.copy2(ROOT / 'scripts/setup-downloader.sh', self.tools / 'setup-downloader.sh')
        self.script('pacman', 'echo FORBIDDEN >> "$CALLS"; exit 91\n')
        os_release = self.base / 'os-release'
        os_release.write_text('ID=alarm\n')
        self.env.update(IPOD_OS_RELEASE_FILE=str(os_release))
        cmd = [BASH, str(self.tools / 'setup-downloader.sh'), '--yes']
        result = subprocess.run(cmd + ['--dry-run'], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.base / 'data space').exists())
        self.assertNotIn('venv:', self.log.read_text())
        result = subprocess.run(cmd, env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn('FORBIDDEN', self.log.read_text())

    def test_data_home_fallback_matches_backend_on_macos_and_linux(self):
        self.env['XDG_DATA_HOME'] = 'relative-is-ignored'
        for system, suffix in [('Darwin', 'Library/Application Support'), ('Linux', '.local/share')]:
            self.env['TEST_OS'] = system
            result = self.run_setup()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((Path(self.env['HOME']) / suffix / 'ipod-connection-utility/gamdl/bin/gamdl').exists())

    def test_debian_python_without_ensurepip_plans_python3_venv(self):
        self.script('apt-get', 'echo FORBIDDEN >> "$CALLS"; exit 91\n')
        # Debian without python3-venv: `-m venv --help` works, only ensurepip is missing.
        self.script('python3', '[[ "$*" != *ensurepip* ]]\n')
        os_release = self.base / 'os-release'
        os_release.write_text('ID=debian\nVERSION_ID="13"\n')
        self.env.update(IPOD_OS_RELEASE_FILE=str(os_release), IPOD_DEPENDENCY_PATH=str(self.tools))
        result = subprocess.run([BASH, str(ROOT / 'scripts/install-dependencies.sh'),
                                 '--basic', '--yes', '--dry-run'],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('apt-get install -y --no-upgrade python3-venv\n', result.stdout)

    def test_basic_dependency_plan_excludes_wrapper_tools(self):
        self.script('pacman', 'echo FORBIDDEN >> "$CALLS"; exit 91\n')
        os_release = self.base / 'os-release'
        os_release.write_text('ID=alarm\n')
        self.env.update(IPOD_OS_RELEASE_FILE=str(os_release), IPOD_DEPENDENCY_PATH=str(self.tools))
        # Missing Python/media commands make the package plan observable.
        for name in ['python3', 'ffmpeg', 'ffprobe']:
            (self.tools / name).unlink()
        result = subprocess.run([BASH, str(ROOT / 'scripts/install-dependencies.sh'),
                                 '--basic', '--yes', '--dry-run'], env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('pacman -S --needed python ffmpeg', result.stdout)
        for token in ['docker', ' git', ' jq', ' unzip']:
            self.assertNotIn(token, result.stdout.lower())
        self.assertFalse(self.log.exists())


if __name__ == '__main__':
    unittest.main()
