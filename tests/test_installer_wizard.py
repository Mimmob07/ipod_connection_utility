"""Isolated installer acceptance tests: fake payload, never package/network/root calls."""
import os
from pathlib import Path
import pty
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WizardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="installer space '")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.pkg = self.base / "extracted package"
        self.bin = self.pkg / "bin"
        self.bin.mkdir(parents=True)
        self.log = self.base / "calls"
        self.prefix = self.base / "chosen prefix's"
        self.env = dict(os.environ, HOME=str(self.base), CALLS=str(self.log))
        # Never probe a real local/remote Docker daemon from installer branching.
        tools = self.base / "safe tools"
        tools.mkdir()
        docker = tools / "docker"
        docker.write_text("#!/usr/bin/env bash\nexit 0\n")
        docker.chmod(0o755)
        self.env["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
        self.env.pop("IPOD_INSTALLER_HOLD", None)
        for source, name in [("install.sh", "ipod-connection-utility-install"),
                             ("install-wizard.sh", "ipod-connection-utility-install-wizard")]:
            if (ROOT / "scripts" / source).exists():
                shutil.copy2(ROOT / "scripts" / source, self.bin / name)
        self.script("ipod-connection-utility", 'printf "app:%s\\n" "$*" >> "$CALLS"\n')
        for name in ["setup-backend", "setup-downloader", "install-dependencies"]:
            self.script("ipod-connection-utility-" + name,
                        'printf "%s\\n" "$0" "$@" >> "$CALLS"\nexit "${MOCK_STATUS:-0}"\n')
        share = self.pkg / "share/ipod-connection-utility"
        share.mkdir(parents=True)
        for name in ["wrapper-portable.patch", "README.md", "DOWNLOADER_SETUP.md"]:
            (share / name).write_text("fixture\n")

    def script(self, name, body):
        path = self.bin / name
        path.write_text("#!/usr/bin/env bash\nset -eu\n" + body)
        path.chmod(0o755)

    def run_wizard(self, answers=None):
        cmd = ["bash", str(self.bin / "ipod-connection-utility-install"), "--interactive"]
        if answers is None:
            return subprocess.run(cmd, input="", text=True, capture_output=True, env=self.env)
        master, slave = pty.openpty()
        try:
            process = subprocess.Popen(cmd, stdin=slave, stdout=subprocess.PIPE,
                                       stderr=subprocess.STDOUT, text=True, env=self.env)
            os.close(slave)
            slave = None
            os.write(master, answers.encode())
            output, _ = process.communicate(timeout=15)
            return subprocess.CompletedProcess(cmd, process.returncode, output, "")
        finally:
            os.close(master)
            if slave is not None:
                os.close(slave)

    def test_sync_only_installs_to_prefix_without_backend(self):
        result = self.run_wizard(f"2\n{self.prefix}\nn\ny\nn\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertTrue((self.prefix / "bin/ipod-connection-utility").is_file())
        self.assertNotIn("setup-backend", self.log.read_text())
        self.assertIn("Launch", result.stdout)
        installed = self.prefix / "bin/ipod-connection-utility-install-wizard"
        self.assertTrue(installed.is_file(), "installed wizard must support subsequent --interactive runs")

    def test_default_is_basic_and_expands_home_prefix(self):
        result = self.run_wizard("\n~/chosen prefix's\nn\ny\nn\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertTrue((self.prefix / "bin/ipod-connection-utility").exists())
        self.assertIn("setup-downloader", self.log.read_text())
        self.assertNotIn("setup-backend", self.log.read_text())
        self.assertNotIn("Docker group", result.stdout)

    def test_basic_stages_helper_and_supports_same_prefix_reinstall(self):
        cmd = ["bash", str(self.bin / "ipod-connection-utility-install"),
               "--basic", "--prefix", str(self.prefix)]
        result = subprocess.run(cmd, capture_output=True, text=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        helper = self.prefix / "bin/ipod-connection-utility-setup-downloader"
        self.assertTrue(helper.is_file())
        cmd[1] = str(self.prefix / "bin/ipod-connection-utility-install")
        result = subprocess.run(cmd, capture_output=True, text=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_source_install_stages_downloader_helper(self):
        # Source-layout payload with a fake cargo: no compiler/package/network work.
        scripts = self.pkg / "scripts"
        scripts.mkdir()
        (self.pkg / "Cargo.toml").touch()
        for name in ["install.sh", "install-wizard.sh", "setup-downloader.sh"]:
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        shutil.copy2(self.bin / "ipod-connection-utility-setup-backend", scripts / "setup-backend.sh")
        shutil.copy2(self.bin / "ipod-connection-utility-install-dependencies", scripts / "install-dependencies.sh")
        patchdir = self.pkg / "backend/patches"
        patchdir.mkdir(parents=True)
        (patchdir / "wrapper-portable.patch").touch()
        for name in ["README.md", "DOWNLOADER_SETUP.md"]:
            (self.pkg / name).touch()
        cargo = self.base / "safe tools/cargo"
        cargo.write_text('#!/usr/bin/env bash\nexit 0\n')
        cargo.chmod(0o755)
        result = subprocess.run(["bash", str(scripts / "install.sh"), "--sync-only",
                                 "--prefix", str(self.prefix)], capture_output=True,
                                text=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.prefix / "bin/ipod-connection-utility-setup-downloader").read_bytes(),
                         (ROOT / "scripts/setup-downloader.sh").read_bytes())

    def test_basic_conflicts_rejected_before_writes(self):
        apkm = self.base / "bundle.apkm"
        apkm.touch()
        for extra in [["--sync-only"], ["--apkm", str(apkm)]]:
            result = subprocess.run(["bash", str(self.bin / "ipod-connection-utility-install"),
                                     "--basic", "--prefix", str(self.prefix), *extra],
                                    capture_output=True, text=True, env=self.env)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(self.prefix.exists())
            self.assertFalse(self.log.exists())

    def test_basic_dry_run_routes_to_downloader_only(self):
        result = subprocess.run(["bash", str(self.bin / "ipod-connection-utility-install"),
                                 "--basic", "--yes", "--dry-run", "--prefix", str(self.prefix)],
                                capture_output=True, text=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.log.read_text()
        self.assertIn("setup-downloader", calls)
        self.assertIn("--dry-run", calls)
        self.assertNotIn("setup-backend", calls)
        self.assertNotIn("install-dependencies", calls)
        self.assertFalse(self.prefix.exists())

    def test_explicit_wizard_modes_and_no_nested_launch(self):
        self.env["IPOD_INSTALLER_NO_LAUNCH"] = "1"
        wizard = self.bin / "ipod-connection-utility-install-wizard"
        for flag in ["--basic", "--advanced"]:
            apkm = self.base / "input.apkm"
            apkm.touch()
            answers = ("~/input.apkm\n" if flag == "--advanced" else "")
            answers += f"{self.prefix}\nn\ny\n"
            master, slave = pty.openpty()
            try:
                proc = subprocess.Popen(["bash", str(wizard), flag], stdin=slave,
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                        text=True, env=self.env)
                os.write(master, answers.encode())
                output, _ = proc.communicate(timeout=10)
                self.assertEqual(proc.returncode, 0, output)
                self.assertNotIn("Choose setup", output)
                self.assertNotIn("Launch app now", output)
            finally:
                os.close(master)
                os.close(slave)

    def test_basic_installs_wrapper_free_downloader_without_backend(self):
        result = subprocess.run(["bash", str(self.bin / "ipod-connection-utility-install"),
                                 "--basic", "--prefix", str(self.prefix)], input="", text=True,
                                capture_output=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.log.read_text()
        self.assertIn("setup-downloader", calls)
        self.assertNotIn("setup-backend", calls)
        self.assertNotIn("install-dependencies", calls)

    def test_advanced_installer_option_enters_wizard(self):
        result = subprocess.run(["bash", str(self.bin / "ipod-connection-utility-install-wizard"),
                                 "--advanced"], input="", text=True, capture_output=True, env=self.env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("terminal", result.stdout + result.stderr)

    def test_full_setup_supplies_apkm_and_explicit_consent(self):
        apkm = self.base / "Apple Music's input.apkm"
        apkm.write_text("mock private input")
        result = self.run_wizard(f"3\n{apkm}\n{self.prefix}\ny\ny\nn\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        calls = self.log.read_text()
        self.assertIn("--apkm\n" + str(apkm), calls)
        self.assertIn("--yes", calls)
        self.assertIn("not redistributed", result.stdout)
        self.assertIn("2FA", result.stdout)
        self.assertIn("Asahi", result.stdout)
        self.assertIn("privileged", result.stdout)
        self.assertIn("Docker group", result.stdout)

    def test_failure_reports_recovery_and_never_offers_launch(self):
        self.env["MOCK_STATUS"] = "42"
        apkm = self.base / "input.apkm"
        apkm.touch()
        result = self.run_wizard(f"3\n{apkm}\n{self.prefix}\nn\ny\n")
        self.assertEqual(result.returncode, 42, result.stdout)
        self.assertIn("retry", result.stdout.lower())
        self.assertNotIn("Launch app now", result.stdout)

    def test_extracted_launcher_uses_terminal_with_literal_spaced_path(self):
        source = ROOT / "scripts/open-installer.sh"
        self.assertTrue(source.exists(), "terminal launcher is missing")
        launcher = self.pkg / "Install.sh"
        shutil.copy2(source, launcher)
        tools = self.base / "terminal tools"
        tools.mkdir()
        bash = shutil.which("bash")
        assert bash is not None
        (tools / "bash").symlink_to(bash)
        terminal = tools / "konsole"
        terminal.write_text('#!/usr/bin/env bash\nprintf "%s\\n" "$@" > "$CALLS"\n')
        terminal.chmod(0o755)
        env = dict(self.env, PATH=str(tools))
        result = subprocess.run([bash, str(launcher)], input="", text=True,
                                capture_output=True, env=env, cwd=self.base)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text().splitlines(),
                         ["-e", "bash", str(self.bin / "ipod-connection-utility-install-wizard")])

    def test_launcher_requests_visible_completion_when_opening_terminal(self):
        source = ROOT / "scripts/open-installer.sh"
        launcher = self.pkg / "Install.sh"
        shutil.copy2(source, launcher)
        tools = self.base / "hold tools"
        tools.mkdir()
        bash = shutil.which("bash")
        assert bash is not None
        (tools / "bash").symlink_to(bash)
        terminal = tools / "konsole"
        terminal.write_text('#!/usr/bin/env bash\nprintf "%s" "${IPOD_INSTALLER_HOLD:-missing}" > "$CALLS"\n')
        terminal.chmod(0o755)
        result = subprocess.run([bash, str(launcher)], input="", text=True, capture_output=True,
                                env=dict(self.env, PATH=str(tools)))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text(), "1")

    def test_desktop_wizard_holds_error_visible(self):
        self.env["IPOD_INSTALLER_HOLD"] = "1"
        result = self.run_wizard("bad\n\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Press Enter to close", result.stdout)

    def test_wizard_help_is_noninteractive_and_side_effect_free(self):
        result = subprocess.run(["bash", str(self.bin / "ipod-connection-utility-install-wizard"), "--help"],
                                input="", text=True, capture_output=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("interactive", result.stdout)
        self.assertFalse(self.log.exists())

    def test_confirmed_sync_install_can_launch_app(self):
        result = self.run_wizard(f"2\n{self.prefix}\nn\ny\ny\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(self.log.read_text().splitlines()[-1], "app:")

    def test_cancel_before_install_has_no_side_effects(self):
        result = self.run_wizard(f"2\n{self.prefix}\ny\nn\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertFalse(self.prefix.exists())
        self.assertFalse(self.log.exists())

    def test_missing_apkm_fails_before_any_changes(self):
        result = self.run_wizard("3\n/no/such/bundle.apkm\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not readable", result.stdout)
        self.assertFalse(self.log.exists())

    def test_no_mode_flag_remains_scriptable_app_only(self):
        result = subprocess.run(["bash", str(self.bin / "ipod-connection-utility-install"),
                                 "--prefix", str(self.prefix)], input="", text=True,
                                capture_output=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.prefix / "bin/ipod-connection-utility").exists())
        self.assertNotIn("setup-backend", self.log.read_text())
        self.assertNotIn("install-dependencies", self.log.read_text())

    def test_installer_help_discovers_wizard(self):
        result = subprocess.run(["bash", str(self.bin / "ipod-connection-utility-install"), "--help"],
                                text=True, capture_output=True, env=self.env)
        self.assertEqual(result.returncode, 0)
        self.assertIn("--interactive", result.stdout)

    def test_asahi_consent_forwards_emulation_permission(self):
        uname = self.base / "safe tools/uname"
        uname.write_text('#!/usr/bin/env bash\nif [[ "$1" == -s ]]; then echo Linux; else echo aarch64; fi\n')
        uname.chmod(0o755)
        apkm = self.base / "bundle.apkm"
        apkm.touch()
        result = self.run_wizard(f"3\n{apkm}\n{self.prefix}\ny\ny\nn\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("--configure-emulation", self.log.read_text())

    def test_declining_dependency_consent_does_not_start_docker(self):
        apkm = self.base / "bundle.apkm"
        apkm.touch()
        result = self.run_wizard(f"3\n{apkm}\n{self.prefix}\nn\ny\nn\n")
        self.assertEqual(result.returncode, 0, result.stdout)
        calls = self.log.read_text()
        self.assertNotIn("--start-docker", calls)
        self.assertNotIn("--install\n", calls)
        self.assertNotIn("--configure-emulation", calls)

    def test_root_is_refused_before_any_changes(self):
        # EUID is readonly in bash, so the guard calls `id -u`; stub it to look like root.
        fake = self.base / "root tools"
        fake.mkdir()
        (fake / "id").write_text("#!/usr/bin/env bash\necho 0\n")
        (fake / "id").chmod(0o755)
        env = dict(self.env, PATH=str(fake) + os.pathsep + self.env["PATH"])
        for name, args in [("install", ["--sync-only", "--dry-run"]), ("install-wizard", [])]:
            result = subprocess.run(["bash", str(self.bin / ("ipod-connection-utility-" + name)), *args],
                                    input="", text=True, capture_output=True, env=env)
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn("not with sudo", result.stderr)
        self.assertFalse(self.log.exists())
        self.assertFalse((self.base / ".local").exists())

    def test_noninteractive_refuses_without_changes(self):
        result = self.run_wizard()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("terminal", result.stdout + result.stderr)
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()
