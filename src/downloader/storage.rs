use super::{DownloadError, STAGING_ID};
use crate::sync::publish_new;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

struct IncompleteFile(Option<PathBuf>);

impl Drop for IncompleteFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

fn write_error(path: &Path, error: io::Error) -> DownloadError {
    let hint = if error.kind() == io::ErrorKind::ReadOnlyFilesystem {
        "; the library filesystem is mounted read-only"
    } else {
        ""
    };
    DownloadError::LibraryNotWritable(format!("cannot write {}: {error}{hint}", path.display()))
}

pub(super) fn check_writable(library: &Path) -> Result<(), DownloadError> {
    let serial = STAGING_ID.fetch_add(1, Ordering::Relaxed);
    let path = library.join(format!(".ipod-write-check-{}-{serial}", std::process::id()));
    let mut file = File::create_new(&path).map_err(|error| write_error(library, error))?;
    let cleanup = IncompleteFile(Some(path));
    file.write_all(b"write check")
        .and_then(|()| file.sync_all())
        .map_err(|error| write_error(library, error))?;
    drop(file);
    fs::remove_file(cleanup.0.as_ref().unwrap()).map_err(|error| write_error(library, error))?;
    Ok(())
}

/// Copy and sync a hidden sibling before an atomic, exclusive publication.
/// No hard links or overwriting rename fallback: unsupported filesystems fail closed.
/// SIGKILL may leave hidden staging files; never reclaim another writer's staging.
/// File sync precedes publication; directory sync is best-effort only where the
/// filesystem explicitly does not support it (not a power-loss guarantee on exFAT).
pub(super) fn save_m4a(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<bool, DownloadError> {
    let mut source = File::open(source)
        .map_err(|error| DownloadError::Backend(format!("cannot read staged audio: {error}")))?;
    let parent = destination
        .parent()
        .ok_or_else(|| DownloadError::Backend("missing destination folder".into()))?;
    let (temporary, mut target) = loop {
        let serial = STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".ipod-save-{}-{serial}.part", std::process::id()));
        match File::create_new(&path) {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(write_error(destination, error)),
        }
    };
    let cleanup = IncompleteFile(Some(temporary.clone()));
    let copied = (|| -> io::Result<()> {
        let mut buffer = [0; 64 * 1024];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::other("download cancelled"));
            }
            let count = match source.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                break;
            }
            target.write_all(&buffer[..count])?;
        }
        target.sync_all()
    })();
    drop(target);
    copied.map_err(|error| {
        if cancelled.load(Ordering::Relaxed) {
            DownloadError::Backend("download cancelled".into())
        } else {
            write_error(destination, error)
        }
    })?;
    if cancelled.load(Ordering::Relaxed) {
        return Err(DownloadError::Backend("download cancelled".into()));
    }
    match publish_new(&temporary, destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(write_error(destination, error)),
    }
    drop(cleanup);
    // A post-publication sync failure must not delete the complete published song.
    sync_directory(parent).map_err(|error| write_error(destination, error))?;
    Ok(true)
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    match File::open(directory).and_then(|file| file.sync_all()) {
        // Some exFAT/macOS drivers reject directory fsync. The file itself has
        // already been synced; do not confuse this with a fully durable rename.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::tests::Fixture;

    // Run as a subprocess so SIGKILL bypasses all Rust cleanup guards.
    #[test]
    fn interrupted_save_worker() {
        let Some(root) = std::env::var_os("IPOD_SAVE_CRASH_TEST") else {
            return;
        };
        let root = PathBuf::from(root);
        save_m4a(
            &root.join("pipe"),
            &root.join("final.m4a"),
            &AtomicBool::new(false),
        )
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn killed_copy_never_exposes_a_partial_final_song() {
        use std::ffi::CString;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let fixture = Fixture::new("download_killed_copy");
        let root = PathBuf::from(fixture.lib());
        let pipe = root.join("pipe");
        let name = CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "downloader::storage::tests::interrupted_save_worker",
                "--nocapture",
            ])
            .env("IPOD_SAVE_CRASH_TEST", &root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut writer = fs::OpenOptions::new().write(true).open(&pipe).unwrap();
        writer.write_all(&vec![42; 128 * 1024]).unwrap();
        let started = Instant::now();
        // Wait until the bytes reach either the old visible target or new hidden staging file.
        while !fs::read_dir(&root).unwrap().flatten().any(|entry| {
            entry
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.len() >= 128 * 1024)
        }) && started.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        drop(writer);
        assert!(
            !root.join("final.m4a").exists(),
            "SIGKILL left a truncated visible song"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "copy never started"
        );
        // A retry must not mistake the abandoned hidden file for a saved song.
        let source = root.join("a.mp3");
        assert!(save_m4a(&source, &root.join("final.m4a"), &AtomicBool::new(false)).unwrap());
        assert_eq!(
            fs::read(root.join("final.m4a")).unwrap(),
            fs::read(source).unwrap()
        );
    }

    #[test]
    fn saves_by_copy_and_never_overwrites_existing_music() {
        let fixture = Fixture::new("download_copy");
        let source = Path::new(&fixture.lib()).join("a.mp3");
        let destination = fixture.ipod_file("new.m4a");
        let cancelled = AtomicBool::new(false);
        assert!(save_m4a(&source, &destination, &cancelled).unwrap());
        assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
        fixture.write("a.mp3", "changed source");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "a.mp3");
        assert!(!save_m4a(&source, &destination, &cancelled).unwrap());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "a.mp3");
    }

    #[test]
    fn cancelled_copy_cleans_up_its_partial_file_and_probe_cleans_up() {
        let fixture = Fixture::new("download_copy_cancel");
        check_writable(Path::new(&fixture.lib())).unwrap();
        assert!(
            !fs::read_dir(fixture.lib())
                .unwrap()
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".ipod-write-check"))
        );
        let destination = fixture.ipod_file("cancelled.m4a");
        assert!(
            save_m4a(
                &Path::new(&fixture.lib()).join("a.mp3"),
                &destination,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        assert!(!destination.exists());
    }

    #[test]
    fn cancellation_is_not_a_library_write_failure() {
        let fixture = Fixture::new("download_cancel_classification");
        let result = save_m4a(
            &Path::new(&fixture.lib()).join("a.mp3"),
            &fixture.ipod_file("cancel.m4a"),
            &AtomicBool::new(true),
        );
        assert!(
            matches!(result, Err(DownloadError::Backend(ref message)) if message.contains("cancelled")),
            "{result:?}"
        );
        assert!(
            !fs::read_dir(fixture.ipod_file(""))
                .unwrap()
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".ipod-save-"))
        );
    }

    #[test]
    fn racing_saves_publish_exactly_one_complete_song() {
        use std::sync::{Arc, Barrier};
        let fixture = Fixture::new("download_racing_saves");
        let root = PathBuf::from(fixture.lib());
        let destination = fixture.ipod_file("race.m4a");
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let source = root.join(format!("{index}.m4a"));
                fs::write(&source, vec![index as u8; 256 * 1024]).unwrap();
                let destination = destination.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    save_m4a(&source, &destination, &AtomicBool::new(false)).unwrap()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .map(|handle| usize::from(handle.join().unwrap()))
                .sum::<usize>(),
            1
        );
        let bytes = fs::read(destination).unwrap();
        assert_eq!(bytes.len(), 256 * 1024);
        assert!(bytes.iter().all(|byte| *byte == bytes[0]));
        assert!(
            !fs::read_dir(fixture.ipod_file(""))
                .unwrap()
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".ipod-save-"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_destination_symlink_is_preserved() {
        let fixture = Fixture::new("download_dangling_link");
        let destination = fixture.ipod_file("song.m4a");
        let absent = fixture.ipod_file("must-not-create.m4a");
        std::os::unix::fs::symlink(&absent, &destination).unwrap();
        assert!(
            !save_m4a(
                &Path::new(&fixture.lib()).join("a.mp3"),
                &destination,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(fs::read_link(destination).unwrap(), absent);
        assert!(!absent.exists());
    }

    #[test]
    fn read_only_errors_explain_the_mount_problem() {
        let error = write_error(
            Path::new("/music/library"),
            io::Error::from_raw_os_error(30),
        );
        assert!(error.to_string().contains("/music/library"));
        assert!(error.to_string().contains("mounted read-only"));
    }
}
