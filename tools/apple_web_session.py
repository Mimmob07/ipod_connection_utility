#!/usr/bin/env python3
"""Private wrapper-free Apple Music session checker (gamdl 3.9.1 API)."""
import json
import os
import pathlib
import stat
import sys
import secrets
from contextlib import contextmanager
import time

MAX_COOKIE_BYTES = 1024 * 1024
COOKIE_NAME = "media-user-token"
APPLE_DOMAINS = {".music.apple.com", "music.apple.com"}

def _data_dir():
    xdg = os.environ.get("XDG_DATA_HOME")
    if xdg and os.path.isabs(xdg):
        return pathlib.Path(xdg) / "ipod-connection-utility" / "web-auth"
    home = pathlib.Path.home()
    if sys.platform == "darwin":
        return home / "Library/Application Support/ipod-connection-utility/web-auth"
    return home / ".local/share/ipod-connection-utility/web-auth"

SESSION_PATH = _data_dir() / "cookies.txt"

IMPORT_REASONS = ("not_found", "too_large", "not_cookie_file", "no_music_token",
                  "expired", "duplicate_token", "unsafe_location")


class ImportProblem(ValueError):
    """Safe, allowlisted import failure reason; never carries cookie data."""

    def __init__(self, reason):
        assert reason in IMPORT_REASONS
        super().__init__(reason)
        self.reason = reason


def _parse_cookie_text_strict(text, now=None):
    """Return (token, expiry) or raise ImportProblem.

    Browser exports (for example Firefox's cookies.txt add-on) include cookies
    for other apple.com hosts; those lines are structurally validated and then
    ignored -- only the music.apple.com media-user-token is kept. Canonical
    domain is required by gamdl 3.9.1's exact domain lookup. This local format
    validation is NOT an authenticated network login.
    """
    now = int(time.time() if now is None else now)
    if len(text.encode("utf-8")) > MAX_COOKIE_BYTES:
        raise ImportProblem("too_large")
    found = None
    expired = False
    records = 0
    for line in text.splitlines():
        line = line.removeprefix("#HttpOnly_")
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) != 7:
            raise ImportProblem("not_cookie_file")
        domain, subdomains, path, secure, expiry, name, value = cols
        if (subdomains not in {"TRUE", "FALSE"}
                or secure not in {"TRUE", "FALSE"}
                or not path.startswith("/")
                or not expiry.isascii() or not expiry.removeprefix("-").isdigit()
                or any(ord(c) < 32 or ord(c) == 127 for c in line if c != "\t")):
            raise ImportProblem("not_cookie_file")
        records += 1
        if domain.lower() not in APPLE_DOMAINS or name != COOKIE_NAME:
            continue  # unrelated cookie: never stored
        if not value or any(c.isspace() for c in value):
            raise ImportProblem("not_cookie_file")
        expires = int(expiry)
        if expires and expires <= now:
            expired = True
            continue
        if found is not None:
            raise ImportProblem("duplicate_token")
        found = (value, expiry)
    if found is None:
        if expired:
            raise ImportProblem("expired")
        raise ImportProblem("no_music_token" if records else "not_cookie_file")
    return found


def validate_cookie_text(text, now=None):
    try:
        return _parse_cookie_text_strict(text, now)[0]
    except ImportProblem:
        return None

def _is_private_group(gid):
    """True for a user-private group: the user's primary group, named after
    the user, with no other members (as OpenSSH StrictModes accepts)."""
    try:
        import grp
        import pwd
        me = pwd.getpwuid(os.getuid())
        group = grp.getgrgid(gid)
    except (ImportError, KeyError, OSError):
        return False
    return (gid == me.pw_gid and group.gr_name == me.pw_name
            and all(member == me.pw_name for member in group.gr_mem))


def _ancestor_is_safe(info, uid=None):
    """Whether no other user can rename/replace entries below this directory."""
    uid = os.getuid() if uid is None else uid
    if info.st_uid not in (0, uid):
        return False
    if info.st_mode & stat.S_IWOTH:
        # Sticky root-owned directories such as /tmp: other users cannot
        # rename entries they do not own, and the next component must still
        # be owned by root or this user.
        return info.st_uid == 0 and bool(info.st_mode & stat.S_ISVTX)
    if info.st_mode & stat.S_IWGRP:
        return info.st_uid == uid and _is_private_group(info.st_gid)
    return True


@contextmanager
def _private_dir(path, create=False):
    """Walk without symlinks, then anchor every mutation to the open directory."""
    path = pathlib.Path(os.path.abspath(path))
    fd = os.open(path.anchor, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for component in path.parts[1:]:
            if create:
                try:
                    os.mkdir(component, 0o700, dir_fd=fd)
                except FileExistsError:
                    pass
            new_fd = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = new_fd
            # Ancestors writable by another user would let them replace the
            # session tree; see _ancestor_is_safe for the accepted exceptions.
            if not _ancestor_is_safe(os.fstat(fd)):
                raise OSError("unsafe session directory")
        info = os.fstat(fd)
        if info.st_uid != os.getuid():
            raise OSError("unsafe session directory")
        if create:
            os.fchmod(fd, 0o700)
        elif stat.S_IMODE(info.st_mode) != 0o700:
            raise OSError("unsafe session directory")
        yield fd
    finally:
        os.close(fd)


def _read_cookie_file(path, dir_fd=None, private=False):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=dir_fd)
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise ValueError("invalid cookie file")
        if info.st_size > MAX_COOKIE_BYTES:
            raise ImportProblem("too_large")
        if private and (info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o600):
            raise ValueError("unsafe cookie file")
        payload = source.read(MAX_COOKIE_BYTES + 1)
        if len(payload) > MAX_COOKIE_BYTES:
            raise ImportProblem("too_large")
    return payload.decode("utf-8")


def import_cookie_file_detailed(source):
    """Import a browser export; return None on success or a safe reason code."""
    try:
        path = pathlib.Path(source).expanduser()
        try:
            raw = _read_cookie_file(path)
        except FileNotFoundError:
            return "not_found"
        except ImportProblem as problem:
            return problem.reason
        except (OSError, UnicodeError, ValueError):
            return "not_cookie_file"
        try:
            token, expiry = _parse_cookie_text_strict(raw)
        except ImportProblem as problem:
            return problem.reason
        payload = ("# Netscape HTTP Cookie File\n"
                   f".music.apple.com\tTRUE\t/\tTRUE\t{expiry}\t{COOKIE_NAME}\t{token}\n")
        target = pathlib.Path(SESSION_PATH)
        with _private_dir(target.parent, create=True) as directory:
            try:
                info = os.stat(target.name, dir_fd=directory, follow_symlinks=False)
                if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
                    return "unsafe_location"
            except FileNotFoundError:
                pass
            tmp = ".cookies-" + secrets.token_hex(16)
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=directory)
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as out:
                    os.fchmod(out.fileno(), 0o600)
                    out.write(payload)
                    out.flush()
                    os.fsync(out.fileno())
                os.replace(tmp, target.name, src_dir_fd=directory, dst_dir_fd=directory)
                os.fsync(directory)
            finally:
                try:
                    os.unlink(tmp, dir_fd=directory)
                except FileNotFoundError:
                    pass
        return None
    except (OSError, UnicodeError, ValueError):
        return "unsafe_location"

def query_subscription(cookie_path):
    """Use gamdl 3.9.1's documented AppleMusicApi cookie constructor/property."""
    import asyncio
    from gamdl.api import AppleMusicApi
    async def check():
        api = await AppleMusicApi.create_from_netscape_cookies(str(cookie_path))
        try:
            return bool(api.active_subscription)
        finally:
            await api.client.aclose()
    return asyncio.run(check())

def session_present():
    path = pathlib.Path(SESSION_PATH)
    try:
        with _private_dir(path.parent) as directory:
            return bool(validate_cookie_text(_read_cookie_file(path.name, directory, private=True)))
    except (OSError, UnicodeError, ValueError):
        return False


def forget_session():
    path = pathlib.Path(SESSION_PATH)
    try:
        with _private_dir(path.parent) as directory:
            os.unlink(path.name, dir_fd=directory)
            os.fsync(directory)
        return True
    except FileNotFoundError:
        return True
    except OSError:
        return False


def run_check():
    path = pathlib.Path(SESSION_PATH)
    if not session_present():
        return {"status": "sign_in_required"}
    try:
        if query_subscription(path):
            return {"status": "authenticated"}
        return {"status": "subscription_required"}
    except Exception as error:
        # gamdl.api.exceptions.GamdlApiResponseError exposes status_code.
        # Never stringify it: upstream errors/logs may contain account details.
        if getattr(error, "status_code", None) in {401, 403}:
            return {"status": "sign_in_required"}
        return {"status": "error"}

def main(argv):
    global SESSION_PATH
    if len(argv) in {3, 4} and argv[1] == "import":
        if len(argv) == 4:
            SESSION_PATH = pathlib.Path(argv[3])
        reason = import_cookie_file_detailed(argv[2])
        if reason is None:
            return {"status": "imported"}
        return {"status": "error", "reason": reason if reason in IMPORT_REASONS else "not_cookie_file"}
    if len(argv) == 3 and argv[1] in {"check", "present", "forget"}:
        SESSION_PATH = pathlib.Path(argv[2])
        if argv[1] == "present":
            return {"status": "present" if session_present() else "sign_in_required"}
        if argv[1] == "forget":
            return {"status": "forgotten" if forget_session() else "error"}
        return run_check()
    return {"status": "error"}

def _cli():
    # Suppress even raw-FD/native dependency diagnostics, without accumulating
    # unbounded logs in memory. Only the allowlisted JSON result leaves stdout.
    output = os.dup(1)
    try:
        with open(os.devnull, "wb") as sink:
            os.dup2(sink.fileno(), 1)
            os.dup2(sink.fileno(), 2)
        try:
            result = main(sys.argv)
        except Exception:
            result = {"status": "error"}
        os.write(output, (json.dumps(result, separators=(",", ":")) + "\n").encode("ascii"))
    finally:
        os.close(output)


if __name__ == "__main__":
    _cli()
