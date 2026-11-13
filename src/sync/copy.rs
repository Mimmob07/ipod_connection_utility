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
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Copy destination has no parent",
        )
    })?;
    let mut input = File::open(source)?;
    let (stage, mut output) = loop {
        let id = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".ipod-sync-{}-{id}.tmp", std::process::id()));
        match File::create_new(&path) {
            Ok(file) => break (Stage(path), file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    // Byte copying intentionally avoids fs::copy's permission propagation and
    // hard links: neither is a portable assumption on exFAT destinations.
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    // Same-directory rename is the publication point. Never remove the old
    // destination first, including when rename fails on an unsupported platform.
    fs::rename(&stage.0, destination)?;
    // Persist the directory entry where supported. A failure here is reported
    // AFTER publication: the complete new file may already be visible. Atomic
    // rename/fsync cannot promise power-loss safety on faulty devices or FSes.
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
