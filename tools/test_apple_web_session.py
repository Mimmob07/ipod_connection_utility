import importlib.util
import json
import subprocess
import sys
import pytest

COOKIE = ".music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsynthetic\n"

@pytest.mark.parametrize("payload", [
    COOKIE.replace("TRUE", "MAYBE", 1),
    COOKIE.replace("\t/\t", "\tnot-a-path\t"),
    COOKIE.replace("synthetic", "   "),
    COOKIE.replace("synthetic", "bad\x00value"),
    COOKIE.replace(".music.apple.com", "..music.apple.com"),
    COOKIE.replace(".music.apple.com", ".apple.com"),
    COOKIE + COOKIE,
])
def test_parser_rejects_ambiguous_or_malformed_token(payload):
    assert helper.validate_cookie_text(payload) is None


def test_host_only_cookie_is_canonical_for_gamdl(tmp_path, monkeypatch):
    from http.cookiejar import MozillaCookieJar
    source = tmp_path / "source"
    source.write_text(COOKIE.replace(".music.apple.com\tTRUE", "music.apple.com\tFALSE"))
    target = tmp_path / "private" / "cookies.txt"
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    assert helper.import_cookie_file(source)
    jar = MozillaCookieJar(str(target))
    jar.load(ignore_discard=True, ignore_expires=True)
    assert [c.domain for c in jar] == [".music.apple.com"]
from pathlib import Path
SCRIPT = Path(__file__).with_name("apple_web_session.py")
spec = importlib.util.spec_from_file_location("apple_web_session", SCRIPT)
assert spec and spec.loader
helper = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = helper
spec.loader.exec_module(helper)

def test_cookie_validation_requires_apple_domain_token_and_expiry():
    valid = "# Netscape HTTP Cookie File\n.music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsecret\n"
    assert helper.validate_cookie_text(valid, now=100) == "secret"
    assert helper.validate_cookie_text(valid.replace(".music.apple.com", ".evil.example"), now=100) is None
    assert helper.validate_cookie_text(valid.replace("\t0\t", "\t99\t"), now=100) is None
    assert helper.validate_cookie_text(valid.replace("secret", ""), now=100) is None

def test_httponly_cookie_import_round_trips_through_mozilla(tmp_path, monkeypatch):
    from http.cookiejar import MozillaCookieJar
    source = tmp_path / "export"
    source.write_text("# Netscape HTTP Cookie File\n#HttpOnly_.music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsynthetic\n")
    target = tmp_path / "private" / "cookies.txt"
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    assert helper.validate_cookie_text(source.read_text()) == "synthetic"
    assert helper.import_cookie_file(source)
    jar = MozillaCookieJar(str(target))
    jar.load(ignore_discard=True, ignore_expires=True)
    assert [(c.domain, c.name, c.value) for c in jar] == [(".music.apple.com", "media-user-token", "synthetic")]


def test_import_rejects_bad_source_without_changing_previous_file(tmp_path, monkeypatch):
    target = tmp_path / "private" / "cookies.txt"
    target.parent.mkdir(mode=0o700)
    target.write_text("previous", encoding="utf-8")
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    source = tmp_path / "bad.txt"
    source.write_text("not cookies", encoding="utf-8")
    assert helper.import_cookie_file(str(source)) is False
    assert target.read_text(encoding="utf-8") == "previous"

def test_import_writes_private_atomic_valid_cookie(tmp_path, monkeypatch):
    target = tmp_path / "private" / "cookies.txt"
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    source = tmp_path / "valid.txt"
    source.write_text("# Netscape HTTP Cookie File\n.music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsecret\n", encoding="utf-8")
    assert helper.import_cookie_file(str(source)) is True
    assert target.read_text(encoding="utf-8").endswith("\tsecret\n")
    assert target.stat().st_mode & 0o777 == 0o600
    assert target.parent.stat().st_mode & 0o777 == 0o700

def test_status_output_is_bounded_and_never_contains_exception_secrets(monkeypatch, tmp_path):
    monkeypatch.setattr(helper, "SESSION_PATH", tmp_path / "cookies")
    helper.SESSION_PATH.write_text("# Netscape HTTP Cookie File\n.music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsecret\n", encoding="utf-8")
    helper.SESSION_PATH.chmod(0o600)
    monkeypatch.setattr(helper, "query_subscription", lambda _p: (_ for _ in ()).throw(RuntimeError("cookie=secret")))
    result = helper.run_check()
    assert result == {"status": "error"}
    assert "secret" not in json.dumps(result)
    assert len(json.dumps(result)) < 256

@pytest.mark.parametrize("where", ["parent", "ancestor", "target", "source"])
def test_import_refuses_symlinks_without_touching_previous(tmp_path, monkeypatch, where):
    real = tmp_path / "real"
    real.mkdir(mode=0o700)
    target = real / "cookies.txt"
    target.write_text("previous")
    source = tmp_path / "source"
    source.write_text(COOKIE)
    if where == "parent":
        (tmp_path / "link").symlink_to(real, target_is_directory=True)
        target = tmp_path / "link" / "cookies.txt"
    elif where == "ancestor":
        (tmp_path / "link").symlink_to(tmp_path, target_is_directory=True)
        target = tmp_path / "link" / "real" / "cookies.txt"
    elif where == "target":
        target.unlink()
        target.symlink_to(source)
    else:
        (tmp_path / "link").symlink_to(source)
        source = tmp_path / "link"
    before = target.read_bytes()
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    assert not helper.import_cookie_file(source)
    assert target.read_bytes() == before


def test_import_rejects_writable_ancestor(tmp_path, monkeypatch):
    unsafe = tmp_path / "unsafe"
    unsafe.mkdir(mode=0o777)
    unsafe.chmod(0o777)
    monkeypatch.setattr(helper, "SESSION_PATH", unsafe / "private" / "cookies.txt")
    source = tmp_path / "source"
    source.write_text(COOKIE)
    assert not helper.import_cookie_file(source)
    assert not helper.SESSION_PATH.exists()


@pytest.mark.parametrize("mode", ["public", "symlink", "expired", "empty", "oversized"])
def test_check_rejects_unsafe_saved_file_before_network(tmp_path, monkeypatch, mode):
    target = tmp_path / "cookies.txt"
    target.write_text(COOKIE)
    target.chmod(0o600)
    if mode == "public":
        target.chmod(0o644)
    elif mode == "symlink":
        (tmp_path / "real").write_text(COOKIE)
        target.unlink()
        target.symlink_to(tmp_path / "real")
    elif mode == "expired":
        target.write_text(COOKIE.replace("\t0\t", "\t1\t"))
    elif mode == "empty":
        target.write_text(COOKIE.replace("synthetic", ""))
    else:
        target.write_bytes(b"#" * (helper.MAX_COOKIE_BYTES + 1))
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    monkeypatch.setattr(helper, "query_subscription", lambda _: pytest.fail("network called"))
    assert helper.run_check() == {"status": "sign_in_required"}


def test_import_reports_local_import_not_authenticated(tmp_path, monkeypatch):
    source = tmp_path / "source"
    source.write_text(COOKIE)
    monkeypatch.setattr(helper, "SESSION_PATH", tmp_path / "private" / "cookies.txt")
    assert helper.main(["helper", "import", str(source)]) == {"status": "imported"}


@pytest.mark.parametrize("status", [401, 403])
def test_revoked_network_token_requires_sign_in(tmp_path, monkeypatch, status):
    source = tmp_path / "source"
    source.write_text(COOKIE)
    monkeypatch.setattr(helper, "SESSION_PATH", tmp_path / "private" / "cookies.txt")
    assert helper.import_cookie_file(source)
    class ApiError(Exception):
        status_code = status
    monkeypatch.setattr(helper, "query_subscription", lambda _: (_ for _ in ()).throw(ApiError("synthetic")))
    assert helper.run_check() == {"status": "sign_in_required"}


def test_cli_suppresses_dependency_stdout_stderr_and_raw_fd_logs(tmp_path):
    import os
    source = tmp_path / "source"
    source.write_text(COOKIE)
    env = dict(os.environ, XDG_DATA_HOME=str(tmp_path), HOME=str(tmp_path))
    imported = subprocess.run([sys.executable, str(SCRIPT), "import", str(source)], env=env, capture_output=True, text=True, timeout=5)
    assert imported.returncode == 0
    module = tmp_path / "modules" / "gamdl"
    module.mkdir(parents=True)
    (module / "__init__.py").write_text("")
    (module / "api.py").write_text('''
import os, sys
print("synthetic-SECRET")
print("synthetic-SECRET", file=sys.stderr)
os.write(1, b"synthetic-SECRET")
os.write(2, b"synthetic-SECRET")
raise RuntimeError("synthetic-SECRET")
''')
    env["PYTHONPATH"] = str(module.parent)
    result = subprocess.run([sys.executable, str(SCRIPT), "check", str(tmp_path / "ipod-connection-utility/web-auth/cookies.txt")], env=env, capture_output=True, text=True, timeout=5)
    assert json.loads(result.stdout) == {"status": "error"}
    assert result.stderr == ""
    assert len(result.stdout) < 256


def test_import_accepts_explicit_rust_session_destination(tmp_path, monkeypatch):
    source = tmp_path / "source"
    source.write_text(COOKIE)
    unwanted = tmp_path / "unwanted" / "cookies.txt"
    target = tmp_path / "chosen" / "cookies.txt"
    monkeypatch.setattr(helper, "SESSION_PATH", unwanted)
    assert helper.main(["helper", "import", str(source), str(target)]) == {"status": "imported"}
    assert target.exists()
    assert not unwanted.exists()


@pytest.mark.parametrize("active", [True, False, "property_raises"])
def test_gamdl_client_is_closed_after_success_or_property_failure(monkeypatch, active):
    import types
    closed = []
    class Client:
        async def aclose(self):
            closed.append(True)
    class Api:
        client = Client()
        @property
        def active_subscription(self):
            if active == "property_raises":
                raise RuntimeError("synthetic")
            return active
        @classmethod
        async def create_from_netscape_cookies(cls, path):
            assert path == "synthetic-path"
            return cls()
    monkeypatch.setitem(sys.modules, "gamdl", types.ModuleType("gamdl"))
    module = types.ModuleType("gamdl.api")
    module.AppleMusicApi = Api
    monkeypatch.setitem(sys.modules, "gamdl.api", module)
    if active == "property_raises":
        with pytest.raises(RuntimeError):
            helper.query_subscription("synthetic-path")
    else:
        assert helper.query_subscription("synthetic-path") is active
    assert closed == [True]


def test_failed_atomic_replace_preserves_previous_and_cleans_staging(tmp_path, monkeypatch):
    source = tmp_path / "source"
    source.write_text(COOKIE)
    target = tmp_path / "private" / "cookies.txt"
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    assert helper.import_cookie_file(source)
    previous = target.read_bytes()
    def fail(*args, **kwargs):
        raise OSError("synthetic-failure")
    monkeypatch.setattr(helper.os, "replace", fail)
    assert not helper.import_cookie_file(source)
    assert target.read_bytes() == previous
    assert list(target.parent.iterdir()) == [target]


def test_file_size_limit_is_inclusive_and_rejected_import_preserves_previous(tmp_path, monkeypatch):
    source = tmp_path / "source"
    # Pad a comment so the total encoded source is exactly 1 MiB.
    payload = COOKIE + "#" * (helper.MAX_COOKIE_BYTES - len(COOKIE))
    source.write_text(payload)
    target = tmp_path / "private" / "cookies.txt"
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    assert helper.import_cookie_file(source)
    previous = target.read_bytes()
    source.write_text(payload + "#")
    assert not helper.import_cookie_file(source)
    assert target.read_bytes() == previous


def test_fifo_source_is_rejected_without_blocking(tmp_path):
    import os
    source = tmp_path / "pipe"
    os.mkfifo(source)
    result = subprocess.run([sys.executable, str(SCRIPT), "import", str(source), str(tmp_path / "private/cookies.txt")], capture_output=True, text=True, timeout=2)
    assert json.loads(result.stdout) == {"status": "error", "reason": "not_cookie_file"}
    assert not (tmp_path / "private").exists()


def test_cli_emits_one_generic_json_status(tmp_path):
    result = subprocess.run([sys.executable, str(SCRIPT), "check", str(tmp_path / "absent")], capture_output=True, text=True, timeout=5)
    assert json.loads(result.stdout) == {"status": "sign_in_required"}
    assert result.stderr == ""


# Ancestor policy (OpenSSH StrictModes-style). Unit-tested with synthetic stat
# results so the assertions do not depend on the host's /tmp or groups.
from types import SimpleNamespace as _St
import stat as _stat


def _dir(mode, uid=1000, gid=1000):
    return _St(st_mode=_stat.S_IFDIR | mode, st_uid=uid, st_gid=gid)


@pytest.mark.parametrize("info, private_group, safe", [
    (_dir(0o755, uid=0), False, True),                 # /, /home
    (_dir(0o755), False, True),                        # owned by user
    (_dir(0o755, uid=1001), False, False),             # another user's dir
    (_dir(0o1777, uid=0), False, True),                # /tmp: sticky, root-owned
    (_dir(0o1777, uid=1001), False, False),            # sticky but other user's
    (_dir(0o777), False, False),                       # world-writable, no sticky
    (_dir(0o1777), False, False),                      # user-owned world-writable
    (_dir(0o775), True, True),                         # user-private group (umask 002)
    (_dir(0o775), False, False),                       # shared group writable
    (_dir(0o775, uid=0, gid=100), False, False),       # root dir, shared group
])
def test_ancestor_policy(info, private_group, safe, monkeypatch):
    monkeypatch.setattr(helper, "_is_private_group", lambda gid: private_group)
    assert helper._ancestor_is_safe(info, uid=1000) is safe


def test_user_private_group_detection(monkeypatch):
    import grp
    import pwd
    me = _St(pw_name="alice", pw_gid=1000)
    monkeypatch.setattr(pwd, "getpwuid", lambda uid: me)
    groups = {
        1000: _St(gr_name="alice", gr_mem=[]),
        1001: _St(gr_name="alice", gr_mem=["alice"]),
        1002: _St(gr_name="users", gr_mem=[]),
        1003: _St(gr_name="alice", gr_mem=["bob"]),
    }
    monkeypatch.setattr(grp, "getgrgid", lambda gid: groups[gid])
    assert helper._is_private_group(1000)
    assert not helper._is_private_group(1001)  # not the user's primary gid
    assert not helper._is_private_group(1002)
    monkeypatch.setattr(pwd, "getpwuid", lambda uid: _St(pw_name="alice", pw_gid=1003))
    assert not helper._is_private_group(1003)  # bob is also a member


def test_session_under_sticky_tmp_imports(monkeypatch):
    """Regression: imports failed whenever any ancestor was sticky /tmp."""
    import os
    import tempfile
    info = os.stat("/tmp")
    if not (info.st_uid == 0 and info.st_mode & _stat.S_ISVTX):
        pytest.skip("/tmp is not root-owned sticky on this host")
    with tempfile.TemporaryDirectory(dir="/tmp") as base:
        base = pathlib_Path(base)
        source = base / "source"
        source.write_text(COOKIE)
        monkeypatch.setattr(helper, "SESSION_PATH", base / "data" / "web-auth" / "cookies.txt")
        assert helper.import_cookie_file(source)
        assert helper.session_present()


from pathlib import Path as pathlib_Path  # noqa: E402


# Real browser exports (e.g. Firefox "cookies.txt" add-on) contain cookies for
# other apple.com hosts alongside the music token. Synthetic values only.
FIREFOX_EXPORT = (
    "# Netscape HTTP Cookie File\n"
    "# https://curl.haxx.se/rfc/cookie_spec.html\n"
    "# This is a generated file! Do not edit.\n"
    "\n"
    ".apple.com\tTRUE\t/\tTRUE\t4102444800\tgeo\tUS\n"
    ".apple.com\tTRUE\t/\tFALSE\t4102444800\tdslang\tUS-EN\n"
    "#HttpOnly_.apple.com\tTRUE\t/\tTRUE\t4102444800\tmyacinfo\tsynthetic-account-cookie\n"
    "music.apple.com\tFALSE\t/\tTRUE\t0\tpldfltcid\tsynthetic-other\n"
    ".music.apple.com\tTRUE\t/\tTRUE\t4102444800\tmedia-user-token\tsynthetic-token\n"
    ".music.apple.com\tTRUE\t/\tTRUE\t4102444800\titspod\t31\n"
)


def test_real_browser_export_with_other_apple_hosts_imports_only_the_token(tmp_path, monkeypatch):
    source = tmp_path / "cookies.txt"
    source.write_text(FIREFOX_EXPORT.replace("\n", "\r\n"))  # also tolerate CRLF
    target = tmp_path / "private" / "cookies.txt"
    monkeypatch.setattr(helper, "SESSION_PATH", target)
    assert helper.import_cookie_file_detailed(source) is None
    stored = target.read_text()
    assert "synthetic-token" in stored
    for unrelated in ("synthetic-account-cookie", "synthetic-other", "geo", "dslang"):
        assert unrelated not in stored, "only the music token may be stored"


@pytest.mark.parametrize("payload, reason", [
    ("hello world\n", "not_cookie_file"),
    (FIREFOX_EXPORT.replace(".apple.com\tTRUE\t/\tTRUE\t4102444800\tgeo\tUS\n", "garbage line\n"), "not_cookie_file"),
    (FIREFOX_EXPORT.replace("media-user-token", "other-name"), "no_music_token"),
    (FIREFOX_EXPORT.replace(".music.apple.com\tTRUE", ".apple.com\tTRUE"), "no_music_token"),
    (FIREFOX_EXPORT.replace("4102444800\tmedia-user-token", "100\tmedia-user-token"), "expired"),
    (FIREFOX_EXPORT + ".music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsecond\n", "duplicate_token"),
])
def test_import_reports_specific_safe_reason(tmp_path, monkeypatch, payload, reason):
    source = tmp_path / "cookies.txt"
    source.write_text(payload)
    monkeypatch.setattr(helper, "SESSION_PATH", tmp_path / "private" / "cookies.txt")
    assert helper.import_cookie_file_detailed(source) == reason
    assert not helper.import_cookie_file(source)


def test_import_reports_too_large_and_missing(tmp_path, monkeypatch):
    monkeypatch.setattr(helper, "SESSION_PATH", tmp_path / "private" / "cookies.txt")
    big = tmp_path / "big.txt"
    big.write_bytes(b"#" * (helper.MAX_COOKIE_BYTES + 1))
    assert helper.import_cookie_file_detailed(big) == "too_large"
    assert helper.import_cookie_file_detailed(tmp_path / "absent.txt") == "not_found"


def test_cli_import_error_reason_is_allowlisted_and_secret_free(tmp_path):
    source = tmp_path / "cookies.txt"
    source.write_text(FIREFOX_EXPORT.replace("media-user-token", "other-name"))
    result = subprocess.run([sys.executable, str(SCRIPT), "import", str(source), str(tmp_path / "p" / "c.txt")],
                            capture_output=True, text=True, timeout=10)
    assert json.loads(result.stdout) == {"status": "error", "reason": "no_music_token"}
    assert "synthetic" not in result.stdout + result.stderr
