use super::*;
use crate::downloader::DownloadBackend;
use crate::tui::app::{Confirm, Input, Modal};

#[test]
fn basic_sign_in_asks_for_cookie_file_not_apple_password() {
    let fixture = Fixture::new("tui_cookie_prompt");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    press(&mut app, KeyCode::Char('c'));
    let rendered = screen(&mut app, 130, 36);
    assert!(rendered.contains("cookie file"), "{rendered}");
    assert!(
        !rendered.contains("Apple ID"),
        "basic login must not ask for credentials"
    );
    assert!(!rendered.contains("Password"));
}

#[test]
fn advanced_setup_requires_confirmation_and_preserves_queue() {
    let fixture = Fixture::new("tui_setup_confirmation");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    app.downloader
        .enqueue(
            "https://music.apple.com/us/song/example/456",
            &fixture.lib(),
        )
        .unwrap();
    press(&mut app, KeyCode::Char('u'));
    assert!(matches!(
        app.modal,
        Some(Modal::Confirm {
            action: Confirm::AdvancedSetup,
            ..
        })
    ));
    assert!(app.pending_setup.is_none());
    assert!(screen(&mut app, 130, 36).contains("advanced"));
    press(&mut app, KeyCode::Char('n'));
    assert!(app.modal.is_none());
    assert_eq!(app.downloader.queue().len(), 1);
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(app.pending_setup, Some(DownloadBackend::Wrapper));
    assert!(app.modal.is_none());
    assert_eq!(app.downloader.queue().len(), 1);
    assert!(!app.should_quit);
}

#[test]
fn queued_advanced_track_waits_even_after_an_old_basic_entry() {
    let fixture = Fixture::new("mixed_backend_gate");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    app.wrapper_ready = false;
    app.downloader
        .enqueue(
            "https://music.apple.com/us/song/example/456",
            &fixture.lib(),
        )
        .unwrap();
    let first = app.downloader.next_queued().unwrap();
    app.downloader.finish(
        first.index,
        Err(crate::downloader::DownloadError::GamdlFailed),
    );
    app.downloader.cycle_quality();
    app.downloader.cycle_quality();
    app.downloader
        .enqueue(
            "https://music.apple.com/us/song/example/789",
            &fixture.lib(),
        )
        .unwrap();
    app.poll_job();
    assert!(
        app.job.is_none(),
        "the next queued ALAC track must wait for wrapper readiness, regardless of the first historical entry"
    );
}

#[test]
fn advanced_login_keeps_masked_wrapper_prompt() {
    let fixture = Fixture::new("tui_advanced_prompt");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    app.downloader.cycle_quality();
    app.downloader.cycle_quality();
    app.wrapper_ready = true;
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(
        app.modal,
        Some(Modal::Input {
            target: Input::LoginUsername,
            ..
        })
    ));
}
