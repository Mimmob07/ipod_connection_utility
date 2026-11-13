use super::*;
use crate::sync::tests::Fixture;

#[cfg(unix)]
#[test]
fn web_aac_invocation_uses_cookie_file_without_wrapper_flags() {
    const CHILD: &str = "IPOD_TEST_WEB_ARGS_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = std::env::temp_dir().join(format!("ipod-web-args-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let fake = root.join("gamdl");
        fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$IPOD_TEST_GAMDL_ARGV\"\nexit 17\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "downloader::web_tests::web_aac_invocation_uses_cookie_file_without_wrapper_flags",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("XDG_DATA_HOME", root.join("data"))
            .env("IPOD_GAMDL", fake)
            .env("IPOD_TEST_GAMDL_ARGV", root.join("argv"))
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(root);
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let cookies = crate::web_auth::cookies_path();
    fs::create_dir_all(cookies.parent().unwrap()).unwrap();
    fs::write(&cookies, "# Netscape HTTP Cookie File\n.music.apple.com\tTRUE\t/\tTRUE\t0\tmedia-user-token\tsynthetic-session-for-test\n").unwrap();
    let fixture = Fixture::new("web_args_download");
    let mut downloader = Downloader::default();
    downloader
        .enqueue(
            "https://music.apple.com/us/song/example/456",
            &fixture.lib(),
        )
        .unwrap();
    let task = downloader.next_queued().unwrap();
    let result = download(&task, &AtomicBool::new(false), &mut |_| {});
    assert!(
        matches!(result, Err(DownloadError::GamdlFailed)),
        "fake downloader should be invoked: {result:?}"
    );
    let text = fs::read_to_string(std::env::var_os("IPOD_TEST_GAMDL_ARGV").unwrap()).unwrap();
    let args: Vec<_> = text.lines().collect();
    assert!(
        !args.iter().any(|a| a.contains("wrapper")),
        "basic args include wrapper: {args:?}"
    );
    let index = args
        .iter()
        .position(|a| *a == "--cookies-path")
        .expect("basic mode must pass its private cookie file");
    assert_eq!(Path::new(args[index + 1]), cookies);
    assert!(args.contains(&"aac-web"));
    assert!(
        !text.contains("synthetic-session-for-test"),
        "cookie value leaked to argv"
    );
}

#[test]
fn queued_quality_keeps_its_backend_when_current_selection_changes() {
    let fixture = Fixture::new("captured_backend");
    let mut downloader = Downloader::default();
    downloader
        .enqueue(
            "https://music.apple.com/us/song/example/456",
            &fixture.lib(),
        )
        .unwrap();
    downloader.cycle_quality();
    downloader.cycle_quality();
    assert_eq!(downloader.backend(), DownloadBackend::Wrapper);
    let basic = downloader.next_queued().unwrap();
    assert_eq!(basic.options.backend(), DownloadBackend::Web);
    downloader
        .enqueue(
            "https://music.apple.com/us/song/example/789",
            &fixture.lib(),
        )
        .unwrap();
    downloader.cycle_quality();
    assert_eq!(downloader.backend(), DownloadBackend::Web);
    assert_eq!(
        downloader.next_queued().unwrap().options.backend(),
        DownloadBackend::Wrapper
    );
}
