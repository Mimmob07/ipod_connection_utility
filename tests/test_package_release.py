#!/usr/bin/env python3
"""Release payload regression checks, without running a foreign executable."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from test_check_elf_pages import synthetic_elf, EM_AARCH64


class PackageReleaseTests(unittest.TestCase):
    def test_package_contains_current_helpers_and_valid_checksum(self):
        with tempfile.TemporaryDirectory(prefix="ipod-package-", dir=os.environ.get("TMPDIR")) as tmp:
            tmp = Path(tmp)
            binary = tmp / "binary with spaces"
            binary.write_bytes(synthetic_elf(EM_AARCH64, [(0, 0, 65536)]))
            binary.chmod(0o755)
            out = tmp / "output with spaces"
            subprocess.run(["bash", str(ROOT / "scripts/package-release.sh"),
                            "--binary", str(binary), "--target", "aarch64-unknown-linux-gnu",
                            "--output", str(out)], check=True, capture_output=True, text=True)
            package = "ipod-connection-utility-aarch64-unknown-linux-gnu"
            archive = out / (package + ".tar.gz")
            checksum = archive.with_name(archive.name + ".sha256").read_text().split()
            self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), checksum[0])
            self.assertEqual(archive.name, checksum[1])
            with tarfile.open(archive) as tar:
                expected = {
                    "bin/ipod-connection-utility": binary,
                    "bin/ipod-connection-utility-install-wizard": ROOT / "scripts/install-wizard.sh",
                    "Install.sh": ROOT / "scripts/open-installer.sh",
                    "LICENSE": ROOT / "LICENSE",
                    "START_HERE.md": ROOT / "QUICKSTART.md",
                    "share/ipod-connection-utility/QUICKSTART.md": ROOT / "QUICKSTART.md",
                    "bin/ipod-connection-utility-install": ROOT / "scripts/install.sh",
                    "bin/ipod-connection-utility-install-dependencies": ROOT / "scripts/install-dependencies.sh",
                    "bin/ipod-connection-utility-setup-backend": ROOT / "scripts/setup-backend.sh",
                    "bin/ipod-connection-utility-setup-downloader": ROOT / "scripts/setup-downloader.sh",
                    "bin/ipod-connection-utility-cleanup": ROOT / "scripts/cleanup-legacy.sh",
                    "share/ipod-connection-utility/check_elf_pages.py": ROOT / "tools/check_elf_pages.py",
                    "share/ipod-connection-utility/wrapper-portable.patch": ROOT / "backend/patches/wrapper-portable.patch",
                }
                for relative, source in expected.items():
                    member = tar.getmember(f"{package}/{relative}")
                    stream = tar.extractfile(member)
                    self.assertIsNotNone(stream)
                    assert stream is not None
                    self.assertEqual(stream.read(), source.read_bytes())
                    if relative.startswith("bin/"):
                        self.assertTrue(member.mode & 0o111)
                for member in tar.getmembers():
                    self.assertFalse(member.issym() or member.islnk())
                    self.assertNotIn("..", Path(member.name).parts)
                    self.assertNotIn(".wrapper-v2", Path(member.name).parts)

    def test_arm_linux_rejects_incompatible_elf(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as tmp:
            binary = Path(tmp) / "bad-arm"
            binary.write_bytes(synthetic_elf(EM_AARCH64, [(0, 0, 4096)]))
            binary.chmod(0o755)
            result = subprocess.run(["bash", str(ROOT / "scripts/package-release.sh"),
                                     "--binary", str(binary), "--target", "aarch64-unknown-linux-gnu",
                                     "--output", tmp], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("below page size", result.stderr)
            self.assertEqual(list(Path(tmp).glob("*.tar.gz")), [])

    def test_existing_artifact_is_not_overwritten(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as tmp:
            binary = Path(tmp) / "arm"
            binary.write_bytes(synthetic_elf(EM_AARCH64, [(0, 0, 65536)]))
            binary.chmod(0o755)
            archive = Path(tmp) / "ipod-connection-utility-aarch64-unknown-linux-gnu.tar.gz"
            archive.write_bytes(b"keep previous artifact")
            result = subprocess.run(["bash", str(ROOT / "scripts/package-release.sh"),
                                     "--binary", str(binary), "--target", "aarch64-unknown-linux-gnu",
                                     "--output", tmp], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(archive.read_bytes(), b"keep previous artifact")

    def test_missing_binary_is_rejected_without_archive(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR")) as tmp:
            result = subprocess.run(["bash", str(ROOT / "scripts/package-release.sh"),
                                     "--binary", str(Path(tmp) / "absent"),
                                     "--target", "aarch64-unknown-linux-gnu", "--output", tmp],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("binary", result.stderr.lower())
            self.assertEqual(list(Path(tmp).glob("*.tar.gz")), [])


if __name__ == "__main__":
    unittest.main()
