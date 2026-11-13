use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const COOKIE: &str = "# Netscape HTTP Cookie File\n#HttpOnly_.music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsynthetic-only\n";

fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "web-auth-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn probe(root: &Path, scenario: &str, python: Option<&Path>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "web_auth::tests::child_probe", "--nocapture"])
        .env("WEB_AUTH_PROBE", scenario)
        .env("WEB_AUTH_ROOT", root)
        .env("HOME", root)
        .env("XDG_DATA_HOME", root.join("data"))
        .env(
            "IPOD_PYTHON",
            python.unwrap_or(Path::new("/usr/bin/python3")),
        )
        .env("PYTHONPATH", root.join("modules"));
    if scenario == "opener" {
        command.env("PATH", root);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn child_probe() {
    let Ok(scenario) = std::env::var("WEB_AUTH_PROBE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("WEB_AUTH_ROOT").unwrap());
    match scenario.as_str() {
        "timeout" | "cancel_running" => {
            let start = Instant::now();
            let cancelled = AtomicBool::new(false);
            thread::scope(|scope| {
                if scenario == "cancel_running" {
                    scope.spawn(|| {
                        thread::sleep(Duration::from_millis(100));
                        cancelled.store(true, Ordering::Relaxed);
                    });
                }
                assert_eq!(
                    check(&cancelled),
                    Err(if scenario == "timeout" {
                        "error"
                    } else {
                        "cancelled"
                    }
                    .into())
                );
            });
            assert!(start.elapsed() < Duration::from_secs(2));
            let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
            assert_eq!(pid, -1, "helper was not reaped");
        }
        "subscription" | "revoked" | "error" => {
            import_cookies(root.join("source").to_str().unwrap()).unwrap();
            let expected = match scenario.as_str() {
                "subscription" => "subscription_required",
                "revoked" => "sign_in_required",
                _ => "error",
            };
            assert_eq!(check(&AtomicBool::new(false)), Err(expected.into()));
        }
        "opener" => {
            open_login().unwrap();
            thread::sleep(Duration::from_millis(200));
            assert_eq!(
                fs::read_to_string(root.join("opened")).unwrap(),
                "1:https://music.apple.com"
            );
            let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
            assert_eq!(pid, -1, "opener left an unreaped child");
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        }
        "symlink_store" => {
            assert!(!has_session(), "symlink store must not be accepted");
            assert!(forget().is_err());
            assert!(root.join("outside/cookies.txt").is_file());
        }
        "cancel_absent" => assert_eq!(check(&AtomicBool::new(true)), Err("cancelled".into())),
        "transport" => {
            let start = Instant::now();
            let result = run(&[], None);
            assert!(result.is_err());
            assert!(start.elapsed() < Duration::from_millis(700));
        }
        "embedded" => assert_eq!(run(&[], None).unwrap(), "{\"status\":\"error\"}"),
        "invalid_json" => assert_eq!(check(&AtomicBool::new(false)), Err("error".into())),
        "round_trip" => {
            assert!(!has_session());
            import_cookies(root.join("source").to_str().unwrap()).unwrap();
            assert!(has_session());
            assert_eq!(
                fs::metadata(cookies_path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let previous = fs::read(cookies_path()).unwrap();
            fs::write(root.join("source"), "invalid").unwrap();
            assert!(import_cookies(root.join("source").to_str().unwrap()).is_err());
            // A real browser export with no music token explains why, safely.
            fs::write(
                root.join("source"),
                ".apple.com\tTRUE\t/\tTRUE\t0\tgeo\tsynthetic-geo\n",
            )
            .unwrap();
            let error = import_cookies(root.join("source").to_str().unwrap()).unwrap_err();
            assert!(error.to_string().contains("media-user-token"), "{error}");
            assert!(!error.to_string().contains("synthetic"));
            assert_eq!(fs::read(cookies_path()).unwrap(), previous);
            assert_eq!(check(&AtomicBool::new(false)), Ok(()));
            assert!(root.join("closed").is_file());
            assert_eq!(check(&AtomicBool::new(true)), Err("cancelled".into()));
            forget().unwrap();
            forget().unwrap();
            assert!(!has_session());
        }
        other => panic!("unknown probe {other}"),
    }
}

#[test]
fn hung_helper_is_bounded_cancelled_and_reaped() {
    let root = scratch();
    let python = root.join("fake-python");
    fs::write(&python, "#!/usr/bin/python3\nimport time\ntime.sleep(10)\n").unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    probe(&root, "timeout", Some(&python));
    probe(&root, "cancel_running", Some(&python));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn strict_json_rejects_duplicate_unknown_and_trailing_data() {
    for response in [
        "{\"status\":\"authenticated\",\"status\":\"error\"}",
        "{\"status\":\"authenticated\",\"extra\":true}",
        "{\"status\":\"authenticated\"} trailing",
        "[]",
        "{\"status\":123}",
    ] {
        assert!(status(response).is_err());
    }
}

#[test]
fn browser_opener_uses_fixed_url_and_is_reaped() {
    let root = scratch();
    let opener = root.join(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    fs::write(
        &opener,
        "#!/bin/sh\nprintf '%s:%s' \"$#\" \"$1\" > \"$HOME/opened\"\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&opener, fs::Permissions::from_mode(0o700)).unwrap();
    probe(&root, "opener", None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn symlink_store_is_never_read_or_forgotten() {
    use std::os::unix::fs::symlink;
    let root = scratch();
    fs::create_dir(root.join("outside")).unwrap();
    fs::write(root.join("outside/cookies.txt"), COOKIE).unwrap();
    fs::create_dir_all(root.join("data/ipod-connection-utility")).unwrap();
    symlink(
        root.join("outside"),
        root.join("data/ipod-connection-utility/web-auth"),
    )
    .unwrap();
    probe(&root, "symlink_store", None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellation_wins_even_without_saved_session() {
    let root = scratch();
    probe(&root, "cancel_absent", None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn helper_is_embedded_in_executable() {
    let root = scratch();
    let python = root.join("fake-python");
    fs::write(
        &python,
        "#!/bin/sh\n[ \"$1\" = '-c' ] || exit 1\nprintf '{\"status\":\"error\"}'\n",
    )
    .unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    probe(&root, "embedded", Some(&python));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stdout_flood_fails_before_timeout_without_pipe_deadlock() {
    let root = scratch();
    let python = root.join("fake-python");
    fs::write(
        &python,
        "#!/usr/bin/python3\nimport os, time\nos.write(1, b'x' * 1000000)\ntime.sleep(5)\n",
    )
    .unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    probe(&root, "transport", Some(&python));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_json_substring_spoofing() {
    let root = scratch();
    let session = root.join("data/ipod-connection-utility/web-auth");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("cookies.txt"), COOKIE).unwrap();
    let python = root.join("fake-python");
    fs::write(
        &python,
        "#!/bin/sh\nprintf 'not-json {\"status\":\"authenticated\"}'\n",
    )
    .unwrap();
    fs::set_permissions(&python, fs::Permissions::from_mode(0o700)).unwrap();
    probe(&root, "invalid_json", Some(&python));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_python_import_and_fake_gamdl_check_round_trip() {
    let root = scratch();
    fs::write(root.join("source"), COOKIE).unwrap();
    let module = root.join("modules/gamdl");
    fs::create_dir_all(&module).unwrap();
    fs::write(module.join("__init__.py"), "").unwrap();
    fs::write(
        module.join("api.py"),
        r#"
import os
from pathlib import Path
from http.cookiejar import MozillaCookieJar
class Client:
    async def aclose(self):
        Path(os.environ['HOME'], 'closed').write_text('closed')
class AppleMusicApi:
    @classmethod
    async def create_from_netscape_cookies(cls, path):
        jar = MozillaCookieJar(path)
        jar.load(ignore_discard=True, ignore_expires=True)
        assert any(c.domain == '.music.apple.com' and c.name == 'media-user-token' for c in jar)
        mode = os.environ.get('WEB_AUTH_PROBE')
        if mode == 'revoked':
            class Revoked(Exception):
                status_code = 401
            raise Revoked('synthetic-secret')
        if mode == 'error':
            raise RuntimeError('synthetic-secret')
        api = cls()
        api.client = Client()
        api.active_subscription = mode != 'subscription'
        return api
"#,
    )
    .unwrap();
    probe(&root, "round_trip", None);
    for scenario in ["subscription", "revoked", "error"] {
        fs::write(root.join("source"), COOKIE).unwrap();
        probe(&root, scenario, None);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pasted_cookie_paths_are_normalized() {
    let home = Path::new("/home/me");
    for (input, expected) in [
        ("/tmp/cookies.txt", "/tmp/cookies.txt"),
        ("  /tmp/cookies.txt \n", "/tmp/cookies.txt"),
        (
            "'/home/me/Downloads/my cookies.txt'",
            "/home/me/Downloads/my cookies.txt",
        ),
        (
            "\"/home/me/Downloads/it's.txt\"",
            "/home/me/Downloads/it's.txt",
        ),
        (
            "file:///home/me/Downloads/my%20cookies.txt",
            "/home/me/Downloads/my cookies.txt",
        ),
        ("file://localhost/tmp/a.txt", "/tmp/a.txt"),
        ("~/Downloads/cookies.txt", "/home/me/Downloads/cookies.txt"),
        ("/tmp/100%.txt", "/tmp/100%.txt"),
        ("'unbalanced.txt", "'unbalanced.txt"),
    ] {
        assert_eq!(
            normalize_cookie_path(input, Some(home)),
            PathBuf::from(expected),
            "{input:?}"
        );
    }
}

#[test]
fn import_reasons_map_to_safe_messages() {
    assert!(import_reason_message(Some("no_music_token")).contains("media-user-token"));
    assert!(import_reason_message(Some("expired")).contains("expired"));
    let generic = import_reason_message(None);
    assert_eq!(import_reason_message(Some("attacker <token>")), generic);
    assert_eq!(
        status("{\"status\":\"error\",\"reason\":\"expired\"}").unwrap(),
        "error"
    );
}
