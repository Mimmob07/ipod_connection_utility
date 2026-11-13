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
    let mut command = Command::new("bash");
    command
        .arg(script)
        .arg(if backend == DownloadBackend::Wrapper {
            "--advanced"
        } else {
            "--basic"
        })
        .env("IPOD_INSTALLER_NO_LAUNCH", "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // Like system(): Ctrl+C stops the wizard, while the app and its queue survive.
    #[cfg(unix)]
    let previous = unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
        [
            libc::signal(libc::SIGINT, libc::SIG_IGN),
            libc::signal(libc::SIGQUIT, libc::SIG_IGN),
        ]
    };
    let status = command.status();
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, previous[0]);
        libc::signal(libc::SIGQUIT, previous[1]);
    }
    status
}
