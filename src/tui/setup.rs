//! Resume the current app after the separate consent-based setup wizard exits.
use crate::downloader::DownloadBackend;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

fn wizard_at(executable: &Path) -> io::Result<PathBuf> {
    let sibling = executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("ipod-connection-utility-install-wizard");
    if sibling.is_file() {
        return Ok(sibling);
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install-wizard.sh");
    if source.is_file() {
        return Ok(source);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Setup wizard missing; extract/install the complete app archive",
    ))
}

pub(super) fn run(backend: DownloadBackend) -> io::Result<ExitStatus> {
    let script = wizard_at(&std::env::current_exe()?)?;
    Command::new("bash")
        .arg(script)
        .arg(if backend == DownloadBackend::Wrapper {
            "--advanced"
        } else {
            "--basic"
        })
        .env("IPOD_INSTALLER_NO_LAUNCH", "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}
