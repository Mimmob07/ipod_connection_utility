#!/usr/bin/env python3
"""Exercise the real app/manager processes with disposable Docker and HTTP fakes.

These checks cover orchestration, not authentication or the Android worker.
Run after `cargo build --locked`: python3 tests/test_lifecycle.py --binary PATH
"""

import argparse
import fcntl
import http.server
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import unittest


FAKE_DOCKER = r'''import json, os, sys, time
from pathlib import Path
args = sys.argv[1:]
action = args[1] if args and args[0] == "compose" else "unexpected"
record = {"action": action, "args": args, "parent": os.getppid(),
          "session": os.getsid(os.getppid()), "cwd": os.getcwd(),
          "env": {key: os.environ.get(key) for key in (
          "TARGET_ARCH", "RUNTIME_PLATFORM", "WRAPPER_WORKER_MODE", "WRAPPER_HOST_PAGE_SIZE",
          "WRAPPER_IMAGE_TAG", "COMPOSE_PROJECT_NAME", "HTTP_PORT", "DECRYPT_PORT", "WRAPPER_DATA_DIR")}}
with open(os.environ["FAKE_DOCKER_LOG"], "a") as output:
    output.write(json.dumps(record) + "\n")
if action == "ps":
    if os.environ.get("FAKE_EXTERNAL") == "1": print("external-container")
elif action == "up":
    time.sleep(float(os.environ.get("FAKE_UP_DELAY", "0")))
elif action == "stop":
    time.sleep(float(os.environ.get("FAKE_STOP_DELAY", "0")))
    if os.environ.get("FAKE_STOP_FAIL") == "1": sys.exit(1)
    Path(os.environ["FAKE_STOPPED"]).touch()
else:
    sys.exit(4)
'''


def wait_for(predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.025)
    raise AssertionError("Timed out waiting for lifecycle condition")


class Backend(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        data = {"version": "test", "runtime": {"initialized": True, "playback_ready": True},
                "auth": {"state": "authenticated"}}
        body = json.dumps(data).encode()
        self.send_response(200 if self.server.healthy else 503)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


class Lifecycle(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ipod-lifecycle-")
        self.root = Path(self.temp.name)
        self.checkout = self.root / "backend with spaces"
        self.checkout.mkdir()
        (self.checkout / "compose.yaml").write_text("services: {}\n")
        self.bin_dir = self.root / "bin"
        self.bin_dir.mkdir()
        docker = self.bin_dir / "docker"
        # /usr/bin/env finds Python in this test's private executable directory.
        os.symlink(sys.executable, self.bin_dir / "ipod-test-python")
        docker.write_text("#!/usr/bin/env ipod-test-python\n" + FAKE_DOCKER)
        docker.chmod(0o755)
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Backend)
        self.server.daemon_threads = True
        self.server.healthy = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        # Clear inherited app/Compose overrides so the test cannot touch real state.
        self.env = {k: v for k, v in os.environ.items()
                    if not k.startswith(("IPOD_", "WRAPPER_", "COMPOSE_", "FAKE_"))
                    and k not in ("TARGET_ARCH", "RUNTIME_PLATFORM", "HTTP_PORT",
                                  "DECRYPT_PORT", "XDG_DATA_HOME")}
        self.env.update({"PATH": str(self.bin_dir) + os.pathsep + os.environ.get("PATH", ""),
                         "XDG_DATA_HOME": str(self.root / "data with spaces"),
                         "IPOD_WRAPPER_DIR": str(self.checkout),
                         "IPOD_WRAPPER_PROFILE": "native-amd64",
                         "IPOD_WRAPPER_HTTP_PORT": str(self.server.server_port),
                         "IPOD_WRAPPER_DECRYPT_PORT": "12345",
                         "FAKE_DOCKER_LOG": str(self.root / "docker.jsonl"),
                         "FAKE_STOPPED": str(self.root / "stopped"), "TERM": "xterm-256color"})
        self.children = []

    def tearDown(self):
        for child in self.children:
            if child.stdin and not child.stdin.closed:
                child.stdin.close()
        for child in self.children:
            try:
                child.wait(timeout=8)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
            if child.stdout:
                child.stdout.close()
            if child.stderr:
                child.stderr.close()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.temp.cleanup()

    def records(self):
        path = Path(self.env["FAKE_DOCKER_LOG"])
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text().splitlines()]

    def manager(self, **extra):
        child = subprocess.Popen([str(BINARY), "--internal-wrapper-manager"],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, env={**self.env, **extra})
        self.children.append(child)
        return child

    def event(self, child):
        readable, _, _ = select.select([child.stdout], [], [], 8)
        self.assertTrue(readable, "Manager did not report readiness/failure")
        line = child.stdout.readline()
        self.assertTrue(line, "Manager exited without an event")
        return json.loads(line)

    def close_manager(self, child):
        child.stdin.close()
        self.assertEqual(child.wait(timeout=8), 0)

    def test_start_stop_restart_reuses_atomic_profile_and_external_state(self):
        for _ in range(2):
            manager = self.manager(TARGET_ARCH="wrong", RUNTIME_PLATFORM="wrong",
                                   WRAPPER_WORKER_MODE="wrong", WRAPPER_HOST_PAGE_SIZE="65536")
            self.assertTrue(self.event(manager)["Ready"]["authenticated"])
            self.close_manager(manager)
        records = self.records()
        self.assertEqual([r["action"] for r in records], ["ps", "up", "stop"] * 2)
        expected = records[0]["env"]
        self.assertTrue(all(r["env"] == expected for r in records))
        self.assertEqual(expected["TARGET_ARCH"], "x86_64")
        self.assertEqual(expected["RUNTIME_PLATFORM"], "linux/amd64")
        self.assertEqual(expected["WRAPPER_WORKER_MODE"], "native")
        self.assertEqual(expected["WRAPPER_HOST_PAGE_SIZE"], "0")
        self.assertEqual(expected["COMPOSE_PROJECT_NAME"], "ipod-wrapper-native-amd64")
        self.assertEqual(expected["HTTP_PORT"], str(self.server.server_port))
        self.assertEqual(expected["DECRYPT_PORT"], "12345")
        self.assertTrue(Path(expected["WRAPPER_DATA_DIR"]).is_dir())
        self.assertNotIn(str(self.checkout), expected["WRAPPER_DATA_DIR"])

    def test_backend_that_stops_responding_reports_failure_and_stops(self):
        manager = self.manager()
        self.assertTrue(self.event(manager)["Ready"]["authenticated"])
        self.server.healthy = False
        # Three failed checks, two seconds apart.
        readable, _, _ = select.select([manager.stdout], [], [], 12)
        self.assertTrue(readable, "Manager never reported the failed health checks")
        self.assertIn("stopped responding", self.event(manager)["Failed"])
        self.assertEqual(manager.wait(timeout=8), 0)
        self.assertEqual([r["action"] for r in self.records()], ["ps", "up", "stop"])

    def test_saved_profile_backend_directory_is_used_without_environment_override(self):
        saved_checkout = self.root / "saved backend checkout with spaces"
        saved_checkout.mkdir()
        (saved_checkout / "compose.yaml").write_text("services: {}\n")
        state_directory = Path(self.env["XDG_DATA_HOME"]) / "ipod-connection-utility"
        state_directory.mkdir(parents=True)
        (state_directory / "backend-dir-native-amd64").write_text(
            str(saved_checkout.resolve()) + "\n"
        )
        env = dict(self.env)
        env.pop("IPOD_WRAPPER_DIR")
        manager = subprocess.Popen([str(BINARY), "--internal-wrapper-manager"],
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, env=env)
        self.children.append(manager)
        self.assertTrue(self.event(manager)["Ready"]["authenticated"])
        self.close_manager(manager)
        self.assertEqual([record["cwd"] for record in self.records()],
                         [str(saved_checkout.resolve())] * 3)

    def test_empty_xdg_uses_fixture_home_for_start_and_stop_state(self):
        fixture_home = self.root / "fixture home"
        env = dict(self.env)
        env.update({"XDG_DATA_HOME": "", "HOME": str(fixture_home)})
        manager = subprocess.Popen([str(BINARY), "--internal-wrapper-manager"],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, env=env)
        self.children.append(manager)
        self.assertTrue(self.event(manager)["Ready"]["authenticated"])
        self.close_manager(manager)

        if sys.platform == "darwin":
            state_dir = fixture_home / "Library" / "Application Support" / "ipod-connection-utility"
        else:
            state_dir = fixture_home / ".local" / "share" / "ipod-connection-utility"
        self.assertTrue((state_dir / "wrapper-manager.lock").is_file())
        records = self.records()
        self.assertEqual([record["action"] for record in records], ["ps", "up", "stop"])
        self.assertEqual(
            records[0]["env"]["WRAPPER_DATA_DIR"],
            str(state_dir / "wrapper-data" / "native-amd64"),
        )
        self.assertTrue(Path(records[0]["env"]["WRAPPER_DATA_DIR"]).is_dir())

    def test_refuses_externally_owned_container_without_stopping_it(self):
        manager = self.manager(FAKE_EXTERNAL="1")
        self.assertIn("already running outside", self.event(manager)["Failed"])
        self.close_manager(manager)
        self.assertEqual([r["action"] for r in self.records()], ["ps"])

    def test_exit_during_startup_settles_up_then_stops(self):
        manager = self.manager(FAKE_UP_DELAY="0.5")
        wait_for(lambda: any(r["action"] == "up" for r in self.records()))
        self.close_manager(manager)
        self.assertEqual([r["action"] for r in self.records()], ["ps", "up", "stop"])
        self.assertTrue(Path(self.env["FAKE_STOPPED"]).exists())

    def test_contending_manager_waits_until_previous_owner_stops(self):
        first = self.manager()
        self.assertIn("Ready", self.event(first))
        second = self.manager()
        time.sleep(0.25)
        self.assertEqual([r["action"] for r in self.records()], ["ps", "up"])
        self.close_manager(first)
        self.assertIn("Ready", self.event(second))
        self.close_manager(second)
        self.assertEqual([r["action"] for r in self.records()], ["ps", "up", "stop"] * 2)

    def test_stop_error_is_saved_and_cleared_after_success(self):
        manager = self.manager(FAKE_STOP_FAIL="1")
        self.assertIn("Ready", self.event(manager))
        self.close_manager(manager)
        error_path = (self.root / "data with spaces/ipod-connection-utility/"
                      "native-amd64-lifecycle-error.log")
        self.assertIn("Docker command failed", error_path.read_text())
        manager = self.manager()
        self.assertIn("Ready", self.event(manager))
        self.close_manager(manager)
        self.assertFalse(error_path.exists())

    def test_basic_downloader_does_not_start_wrapper(self):
        child, master, output = self.tui()
        wait_for(lambda: b"Overview" in output)
        os.write(master, b"4")
        time.sleep(0.4)
        os.write(master, b"q")
        self.assertEqual(child.wait(timeout=3), 0)
        self.assertEqual(self.records(), [], "basic AAC tab must not invoke Docker")

    def test_tui_lazy_start_detaches_manager_and_quits_before_cleanup(self):
        self.assert_tui_cleanup(b"q")

    def test_tui_ctrl_c_returns_before_background_cleanup(self):
        self.assert_tui_cleanup(b"\x03")

    def tui(self, **extra):
        """Run the TUI on a pty drained by a thread: the app blocks once a pty buffer
        fills, and macOS pty buffers hold less than one full frame."""
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
        child = subprocess.Popen([str(BINARY), "-l", str(self.root)], stdin=slave,
                                 stdout=slave, stderr=slave, env={**self.env, **extra})
        self.children.append(child)
        os.close(slave)
        output = bytearray()
        draining = threading.Event()

        def drain():
            while not draining.is_set():
                try:
                    readable, _, _ = select.select([master], [], [], 0.1)
                    if readable:
                        output.extend(os.read(master, 65536))
                except OSError:
                    return

        reader = threading.Thread(target=drain, daemon=True)
        reader.start()

        def close():
            draining.set()
            reader.join(timeout=2)
            os.close(master)

        self.addCleanup(close)
        return child, master, output

    def assert_tui_cleanup(self, quit_key):
        child, master, output = self.tui(FAKE_STOP_DELAY="2")
        wait_for(lambda: b"Overview" in output)
        self.assertEqual(self.records(), [], "Core TUI must not invoke Docker")
        os.write(master, b"4vv")  # Select ALAC on Downloader before wrapper startup.
        wait_for(lambda: any(r["action"] == "up" for r in self.records()))
        record = next(r for r in self.records() if r["action"] == "up")
        self.assertEqual(record["parent"], record["session"], "Manager must own a POSIX session")
        self.assertNotEqual(record["parent"], child.pid)
        started = time.monotonic()
        os.write(master, quit_key)
        self.assertEqual(child.wait(timeout=1.5), 0)
        self.assertLess(time.monotonic() - started, 1.5)
        wait_for(lambda: Path(self.env["FAKE_STOPPED"]).exists())
        self.assertEqual([r["action"] for r in self.records()], ["ps", "up", "stop"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/ipod-connection-utility"))
    options, remaining = parser.parse_known_args()
    BINARY = options.binary.resolve()
    if not BINARY.is_file():
        parser.error(f"Build the app first; binary does not exist: {BINARY}")
    unittest.main(argv=[sys.argv[0], *remaining])
