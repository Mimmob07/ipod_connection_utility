use super::app::{App, Database, Level, Tab};
use super::ui;
use crate::sync::tests::Fixture;
use crate::sync::{data_json_path, read_data_json};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::thread;
use std::time::{Duration, Instant};

/// Draw the app the way the terminal would and return the screen as text
fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();

    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>() + "\n")
        .collect();

    // The interface sticks to plain text and box drawing lines, no emojis or other symbols
    for c in text.chars() {
        assert!(
            c.is_ascii() || ('\u{2500}'..='\u{257f}').contains(&c),
            "unexpected {c:?}"
        );
    }
    text
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(app: &mut App, text: &str) {
    text.chars().for_each(|c| press(app, KeyCode::Char(c)));
}

fn wait_for_job(app: &mut App) {
    let started = Instant::now();
    while app.job.is_some() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "job never finished"
        );
        thread::sleep(Duration::from_millis(5));
        app.poll_job();
    }
}

fn last_log(app: &App) -> (Level, &str) {
    let (level, message) = app.log.last().unwrap();
    (*level, message.as_str())
}

#[test]
fn library_folders_are_relative_to_a_non_canonical_library_path() {
    let fixture = Fixture::new("tui_relative_folder");
    // Like a symlink or macOS /var, this spelling differs from the canonical manifest paths.
    let mut app = App::new(format!("{}/album/..", fixture.lib()), fixture.ipod());
    press(&mut app, KeyCode::Char('i'));
    press(&mut app, KeyCode::Char('y'));
    wait_for_job(&mut app);
    press(&mut app, KeyCode::Char('2'));
    let library = screen(&mut app, 100, 30);
    let rows: Vec<_> = library.lines().filter(|row| row.contains(".mp3")).collect();
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|row| !row.contains('/')));
}

#[test]
fn initialize_scan_and_sync_from_the_keyboard() {
    let fixture = Fixture::new("tui_flow");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    assert_eq!(app.database, Database::Missing);
    assert!(screen(&mut app, 100, 30).contains("press i to initialize"));

    // Scanning and syncing need a database first
    press(&mut app, KeyCode::Char('s'));
    assert!(app.job.is_none());
    assert_eq!(last_log(&app).0, Level::Error);

    press(&mut app, KeyCode::Char('i'));
    assert!(screen(&mut app, 100, 30).contains("Initialize database"));
    press(&mut app, KeyCode::Char('y'));
    wait_for_job(&mut app);
    assert_eq!(app.database, Database::Loaded);
    assert_eq!(app.manifest.len(), 5);

    press(&mut app, KeyCode::Char('2'));
    let library = screen(&mut app, 100, 30);
    assert!(library.contains("Library, 5 files"));
    assert!(library.contains("a.mp3") && library.contains("album"));

    fixture.remove("a.mp3");
    fixture.remove("c.mp3");
    fixture.write("d.mp3", "a longer, changed d");
    fixture.write("new.mp3", "new");

    press(&mut app, KeyCode::Char('s'));
    wait_for_job(&mut app);
    press(&mut app, KeyCode::Char('3'));
    let changes = screen(&mut app, 100, 30);
    assert!(changes.contains("1 to add, 1 to update, 2 to delete"));
    assert!(changes.contains("Add") && changes.contains("Update") && changes.contains("Delete"));

    // Nothing touches the iPod until the sync is confirmed
    press(&mut app, KeyCode::Char('a'));
    assert!(screen(&mut app, 100, 30).contains("Sync to iPod"));
    press(&mut app, KeyCode::Char('n'));
    assert!(app.job.is_none() && fixture.ipod_file("a.mp3").exists());

    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Char('y'));
    wait_for_job(&mut app);
    assert_eq!(
        last_log(&app),
        (Level::Success, "Sync finished, 4 changes applied")
    );
    assert!(!fixture.ipod_file("a.mp3").exists());
    assert!(!fixture.ipod_file("c.mp3").exists());
    assert!(fixture.ipod_file("new.mp3").exists());
    assert!(screen(&mut app, 100, 30).contains("Up to date as of the last scan"));

    let mut saved: Vec<String> = read_data_json(&data_json_path(&fixture.lib()))
        .unwrap()
        .iter()
        .map(|x| {
            x.file_path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        })
        .collect();
    saved.sort();
    assert_eq!(saved, ["b.mp3", "d.mp3", "e.mp3", "new.mp3"]);
}

#[test]
fn sync_is_refused_when_the_ipod_is_missing() {
    let fixture = Fixture::new("tui_no_ipod");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    press(&mut app, KeyCode::Char('i'));
    press(&mut app, KeyCode::Char('y'));
    wait_for_job(&mut app);
    fixture.write("new.mp3", "new");
    press(&mut app, KeyCode::Char('s'));
    wait_for_job(&mut app);

    press(&mut app, KeyCode::Char('p'));
    app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_text(&mut app, "/nowhere/ipod");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.ipod_path, "/nowhere/ipod");

    press(&mut app, KeyCode::Char('a'));
    assert!(app.modal.is_none() && app.job.is_none());
    assert!(app.plan.is_none());
    assert!(last_log(&app).1.contains("press s to scan"));
    press(&mut app, KeyCode::Char('s'));
    wait_for_job(&mut app);
    assert!(app.plan.is_none());
    assert!(last_log(&app).1.contains("Scan failed"));
    assert!(screen(&mut app, 100, 30).contains("not found"));
}

#[test]
fn downloader_tab_queues_song_and_playlist_links_and_masks_login() {
    let fixture = Fixture::new("tui_downloader");
    let mut app = App::new(fixture.lib(), fixture.ipod());

    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::BackTab);
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.tab, Tab::Downloader);
    let tab = screen(&mut app, 100, 30);
    assert!(tab.contains("cookie file") && tab.contains("The queue is empty"));

    press(&mut app, KeyCode::Char('n'));
    type_text(&mut app, "https://example.com/song");
    assert!(screen(&mut app, 100, 30).contains("https://example.com/song"));
    press(&mut app, KeyCode::Enter);
    assert!(
        last_log(&app)
            .1
            .contains("enter an Apple Music song or playlist URL")
    );
    assert!(app.downloader.queue().is_empty());

    press(&mut app, KeyCode::Char('n'));
    type_text(
        &mut app,
        "https://music.apple.com/us/album/example/123?i=456",
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.downloader.queue().len(), 1);
    assert!(screen(&mut app, 100, 30).contains("Queued"));

    press(&mut app, KeyCode::Char('v'));
    assert!(screen(&mut app, 100, 30).contains("HE-AAC 64 kbps"));
    assert!(app.downloader.queue()[0].rockbox_art);
    press(&mut app, KeyCode::Char('b'));
    assert!(screen(&mut app, 100, 30).contains("Original"));
    press(&mut app, KeyCode::Char('n'));
    type_text(
        &mut app,
        "https://music.apple.com/us/playlist/example/pl.abc123",
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.downloader.queue().len(), 2);
    assert_eq!(app.downloader.queue()[1].quality.label(), "HE-AAC 64 kbps");
    assert!(!app.downloader.queue()[1].rockbox_art);

    press(&mut app, KeyCode::Char('v')); // ALAC retains the explicit advanced sign-in flow.
    press(&mut app, KeyCode::Char('c'));
    type_text(&mut app, "name@example.com");
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "secret-password");
    let masked = screen(&mut app, 100, 30);
    assert!(!masked.contains("secret-password"));
    assert!(masked.contains("***************"));
    press(&mut app, KeyCode::Esc);
}

#[test]
fn help_small_terminals_and_quit() {
    let fixture = Fixture::new("tui_misc");
    let mut app = App::new(fixture.lib(), fixture.ipod());

    assert!(screen(&mut app, 40, 10).contains("Terminal too small"));
    // The smallest supported size has to draw every tab and window without panicking
    for key in ['1', '2', '3', '4'] {
        press(&mut app, KeyCode::Char(key));
        screen(&mut app, 60, 18);
        press(&mut app, KeyCode::Char('?'));
        assert!(screen(&mut app, 60, 18).contains("Help"));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('l'));
        screen(&mut app, 60, 18);
        press(&mut app, KeyCode::Esc);
    }

    assert!(!app.should_quit);
    press(&mut app, KeyCode::Char('q'));
    assert!(app.should_quit);
}

#[test]
fn default_web_download_does_not_wait_for_wrapper_startup() {
    let fixture = Fixture::new("tui_web_no_wrapper_wait");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    app.wrapper_ready = false;
    app.downloader
        .enqueue(
            "https://music.apple.com/us/song/example/456",
            &fixture.lib(),
        )
        .unwrap();
    app.poll_job();
    assert!(app.job.is_some());
    assert!(matches!(
        app.downloader.queue()[0].status,
        crate::downloader::DownloadStatus::Downloading
    ));
}

#[test]
fn queued_downloads_wait_for_wrapper_startup() {
    let fixture = Fixture::new("tui_wrapper_startup");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    app.wrapper_ready = false;
    app.wrapper_status = "Starting Apple Music...".into();
    app.downloader.cycle_quality();
    app.downloader.cycle_quality();
    app.downloader
        .enqueue(
            "https://music.apple.com/us/song/example/456",
            &fixture.lib(),
        )
        .unwrap();
    app.poll_job();
    assert!(app.job.is_none());
    assert!(matches!(
        app.downloader.queue()[0].status,
        crate::downloader::DownloadStatus::Queued
    ));
    press(&mut app, KeyCode::Char('c'));
    assert!(app.modal.is_none());
    assert!(last_log(&app).1.contains("Starting"));
    app.wrapper_event(crate::wrapper::Event::Ready {
        seconds: 5.0,
        authenticated: true,
    });
    assert!(screen(&mut app, 100, 30).contains("Signed in"));
    assert!(last_log(&app).1.contains("saved session restored"));
}

#[test]
fn s_scans_the_ipod_for_untracked_songs_before_confirmed_sync() {
    let fixture = Fixture::new("tui_orphan");
    crate::sync::initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
    std::fs::write(fixture.ipod_file("orphan.mp3"), "untracked song").unwrap();
    std::fs::remove_file(fixture.ipod_file("b.mp3")).unwrap();
    let mut app = App::new(fixture.lib(), fixture.ipod());
    press(&mut app, KeyCode::Char('s'));
    wait_for_job(&mut app);
    press(&mut app, KeyCode::Char('3'));
    let changes = screen(&mut app, 100, 30);
    assert!(changes.contains("1 to add, 0 to update, 1 to delete"));
    assert!(changes.contains("orphan.mp3"));
    assert!(fixture.ipod_file("orphan.mp3").exists());
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Char('n'));
    assert!(fixture.ipod_file("orphan.mp3").exists());
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Char('y'));
    wait_for_job(&mut app);
    assert!(!fixture.ipod_file("orphan.mp3").exists());
    assert!(fixture.ipod_file("b.mp3").exists());
    press(&mut app, KeyCode::Char('s'));
    wait_for_job(&mut app);
    assert!(app.plan.as_ref().unwrap().is_empty());
}

#[test]
fn long_scan_errors_show_the_file_and_os_error_in_the_log() {
    let fixture = Fixture::new("tui_scan_error_context");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.log.push((Level::Error,
        "Scan failed: Read audio contents /run/media/user/IPOD/Music/a-song-with-a-long-filename.m4a: Input/output error (os error 5)".into()));
    let rendered = screen(&mut app, 100, 30);
    assert!(rendered.contains("Read audio contents"));
    assert!(rendered.contains("a-song-with-a-long-filename.m4a"));
    assert!(rendered.contains("Input/output error (os error 5)"));
}

#[test]
fn playlist_resolution_displays_song_rows_progress_and_scrolls() {
    use super::worker::{Job, JobResult};
    use crate::downloader::{DownloadOutcome, DownloadProgress, DownloadStatus};
    let fixture = Fixture::new("tui_playlist_rows");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    let source = "https://music.apple.com/us/playlist/test/pl.test";
    app.downloader.enqueue(source, &fixture.lib()).unwrap();
    let mut details = crate::downloader::tests::test_playlist(20);
    details.tracks[2].unavailable = Some("No catalog song".into());
    app.job = Some(Job::test_finished(JobResult::PlaylistResolved {
        index: 0,
        result: Ok(details),
    }));
    app.poll_job();
    assert_eq!(app.downloader.queue().len(), 20);
    assert!(last_log(&app).1.contains("20 songs in the queue"));
    let first = app.downloader.next_queued().unwrap();
    app.downloader.finish(
        first.index,
        Ok(DownloadOutcome {
            saved: 1,
            existing: 0,
            unavailable: 0,
            other_skipped: 0,
            invalid: 0,
            rate_limited: false,
            issues: Vec::new(),
        }),
    );
    let second = app.downloader.next_queued().unwrap();
    app.downloader.update_progress(
        second.index,
        DownloadProgress {
            percent: Some(42),
            phase: "Downloading",
            title: None,
        },
    );
    let rendered = screen(&mut app, 120, 35);
    assert!(rendered.contains("Song 01") && rendered.contains("Test Artist"));
    assert!(rendered.contains("Downloaded") && rendered.contains("Downloading 42%"));
    assert!(rendered.contains("Unavailable") && rendered.contains("Test Mix"));
    assert!(rendered.contains("2/20 processed") && rendered.contains("1 downloaded"));
    assert!(!rendered.contains(source));
    press(&mut app, KeyCode::End);
    assert_eq!(app.download_table.selected(), Some(19));
    assert!(screen(&mut app, 120, 35).contains("Song 20"));
    press(&mut app, KeyCode::Home);
    assert_eq!(app.download_table.selected(), Some(0));
    app.downloader.finish(
        second.index,
        Err(crate::downloader::DownloadError::LibraryNotWritable(
            "read-only mount".into(),
        )),
    );
    assert!(app.downloader.next_queued().is_none());
    press(&mut app, KeyCode::Char('t'));
    assert!(matches!(
        app.downloader.queue()[1].status,
        DownloadStatus::Queued
    ));
    assert!(matches!(
        app.downloader.queue()[0].status,
        DownloadStatus::Finished(_)
    ));
}

#[test]
fn recovered_download_shows_downloaded_and_keeps_rate_limit_backoff() {
    use super::worker::{Job, JobResult};
    use crate::downloader::DownloadOutcome;
    let fixture = Fixture::new("tui_recovered_rate_limit");
    let mut app = App::new(fixture.lib(), fixture.ipod());
    app.tab = Tab::Downloader;
    app.downloader
        .enqueue("https://music.apple.com/us/song/test/123", &fixture.lib())
        .unwrap();
    app.downloader.next_queued().unwrap();
    app.job = Some(Job::test_finished(JobResult::Downloaded {
        index: 0,
        result: Ok(DownloadOutcome {
            saved: 1,
            existing: 0,
            unavailable: 0,
            other_skipped: 0,
            invalid: 0,
            rate_limited: true,
            issues: Vec::new(),
        }),
    }));
    app.poll_job();
    let rendered = screen(&mut app, 120, 35);
    assert!(rendered.contains("Downloaded"));
    assert!(!rendered.contains("Partial"));
    assert!(app.download_cooldown_secs() >= 290);
}

#[path = "web_tests.rs"]
mod web_tests;
