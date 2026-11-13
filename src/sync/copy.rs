//! Publish a complete, synced byte copy without truncating the old destination.
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);

impl Drop for Stage {
    fn drop(&mut self) {
        // Best effort: unplugged/read-only devices can prevent cleanup. Hidden
        // leftovers are deliberately outside both local and device inventories.
        let _ = fs::remove_file(&self.0);
    }
}

pub(super) fn publish(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    // Byte copying intentionally avoids fs::copy's permission propagation and
    // hard links: neither is a portable assumption on exFAT destinations.
    publish_with(destination, true, |output| {
        io::copy(&mut input, output).map(drop)
    })
}

/// Stage `write`'s bytes beside `destination`, sync them, then publish. Without
/// `replace`, fail with AlreadyExists if any entry (even a dangling symlink) exists.
pub(super) fn publish_with(
    destination: &Path,
    replace: bool,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Copy destination has no parent",
        )
    })?;
    let (stage, mut output) = loop {
        let id = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".ipod-sync-{}-{id}.tmp", std::process::id()));
        // create_new never follows a planted symlink or reuses another writer's stage.
        match File::create_new(&path) {
            Ok(file) => break (Stage(path), file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    write(&mut output)?;
    output.sync_all()?;
    drop(output);
    // Same-directory rename is the publication point. Never remove the old
    // destination first, including when rename fails on an unsupported platform.
    if replace {
        fs::rename(&stage.0, destination)?;
    } else {
        publish_new(&stage.0, destination)?;
    }
    // Persist the directory entry where supported. A failure here is reported
    // AFTER publication: the complete new file may already be visible. Atomic
    // rename/fsync cannot promise power-loss safety on faulty devices or FSes.
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) fn publish_new(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let source = CString::new(source.as_os_str().as_bytes())?;
        let destination = CString::new(destination.as_os_str().as_bytes())?;
        // Both OS APIs provide atomic no-replace on supporting filesystems,
        // including exFAT drivers that implement exclusive rename. Never fall
        // back to exists()+rename(): that would clobber a racing writer.
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "exclusive publication is unsupported on this OS",
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn staging_skips_planted_symlinks_at_upcoming_stage_names() {
        let fixture = crate::sync::tests::Fixture::new("stage_symlinks");
        let lib = PathBuf::from(fixture.lib());
        let sentinel = lib.join("album/e.mp3");
        let before = fs::read(&sentinel).unwrap();
        let next = NEXT_STAGE.load(Ordering::Relaxed);
        for id in next..next + 64 {
            let stage = lib.join(format!(".ipod-sync-{}-{id}.tmp", std::process::id()));
            std::os::unix::fs::symlink(&sentinel, stage).unwrap();
        }
        let destination = lib.join("new.json");
        publish_with(&destination, false, |file| {
            io::Write::write_all(file, b"new")
        })
        .unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), before);
        assert_eq!(fs::read(&destination).unwrap(), b"new");
    }
}
