use super::*;
use crate::sync::tests::Fixture;

fn tagged_audio(path: &Path) -> bool {
    if !Command::new(python_executable())
        .args(["-c", "import mutagen"])
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("metadata regression needs mutagen (set IPOD_PYTHON to a test venv)");
        return false;
    }
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=2",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
                "-y"
            ])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
    assert!(Command::new(python_executable()).args(["-c", "import sys; from mutagen.mp4 import MP4; a=MP4(sys.argv[1]); a['cnID']=[456]; a.save()"])
        .arg(path).status().unwrap().success());
    true
}

#[test]
fn matching_catalog_metadata_does_not_make_truncated_audio_already_saved() {
    let fixture = Fixture::new("download_truncated_catalog");
    let library = PathBuf::from(fixture.lib());
    let source = library.join("staged.m4a");
    if !tagged_audio(&source) {
        return;
    }
    let destination = library.join("song.m4a");
    let bytes = fs::read(&source).unwrap();
    fs::write(&destination, &bytes[..bytes.len() * 3 / 4]).unwrap();
    assert_eq!(
        prepare_download(&destination, None, false, &AtomicBool::new(false))
            .unwrap()
            .track_id,
        "456"
    );
    let prepared = PreparedDownload {
        relative_path: "song.m4a".into(),
        track_id: "456".into(),
    };
    let result = save_prepared(
        &source,
        &library,
        &prepared,
        "456",
        Quality::Aac256,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(result, "matching metadata hid a truncated existing song");
    assert_eq!(
        fs::read(&destination).unwrap(),
        bytes[..bytes.len() * 3 / 4]
    );
    assert_eq!(fs::read(library.join("song [456].m4a")).unwrap(), bytes);
}

#[test]
fn matching_playable_audio_is_already_saved_in_base_or_catalog_path() {
    let fixture = Fixture::new("download_existing_valid");
    let library = PathBuf::from(fixture.lib());
    let source = library.join("staged.m4a");
    if !tagged_audio(&source) {
        return;
    }
    let prepared = PreparedDownload {
        relative_path: "song.m4a".into(),
        track_id: "456".into(),
    };
    fs::copy(&source, library.join("song.m4a")).unwrap();
    assert!(
        !save_prepared(
            &source,
            &library,
            &prepared,
            "456",
            Quality::Aac256,
            &AtomicBool::new(false)
        )
        .unwrap()
    );
    fs::write(library.join("song.m4a"), "unrelated original").unwrap();
    fs::copy(&source, library.join("song [456].m4a")).unwrap();
    assert!(
        !save_prepared(
            &source,
            &library,
            &prepared,
            "456",
            Quality::Aac256,
            &AtomicBool::new(false)
        )
        .unwrap()
    );
    assert_eq!(
        fs::read_to_string(library.join("song.m4a")).unwrap(),
        "unrelated original"
    );
    assert_eq!(
        fs::read(library.join("song [456].m4a")).unwrap(),
        fs::read(&source).unwrap()
    );
}

#[test]
fn cancelled_existing_audio_check_never_reports_already_saved() {
    let fixture = Fixture::new("download_existing_cancelled");
    let library = PathBuf::from(fixture.lib());
    fs::write(library.join("song.m4a"), "existing music").unwrap();
    let prepared = PreparedDownload {
        relative_path: "song.m4a".into(),
        track_id: "456".into(),
    };
    let result = save_prepared(
        &library.join("a.mp3"),
        &library,
        &prepared,
        "456",
        Quality::Aac256,
        &AtomicBool::new(true),
    );
    assert!(
        matches!(result, Err(DownloadError::Backend(message)) if message.contains("cancelled"))
    );
    assert!(!library.join("song [456].m4a").exists());
}

#[test]
fn corrupt_catalog_collision_is_not_already_saved() {
    let fixture = Fixture::new("download_corrupt_collision");
    let library = PathBuf::from(fixture.lib());
    let source = library.join("a.mp3");
    let prepared = PreparedDownload {
        relative_path: "song.m4a".into(),
        track_id: "456".into(),
    };
    fs::write(library.join("song.m4a"), "existing unrelated music").unwrap();
    fs::write(library.join("song [456].m4a"), "interrupted old download").unwrap();
    let result = save_prepared(
        &source,
        &library,
        &prepared,
        "456",
        Quality::Aac256,
        &AtomicBool::new(false),
    );
    assert!(
        result.is_err(),
        "existence alone incorrectly counted a corrupt collision as saved: {result:?}"
    );
    assert_eq!(
        fs::read_to_string(library.join("song.m4a")).unwrap(),
        "existing unrelated music"
    );
    assert_eq!(
        fs::read_to_string(library.join("song [456].m4a")).unwrap(),
        "interrupted old download"
    );
}
