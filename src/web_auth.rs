//! Wrapper-free local Apple Music web session management.
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(not(test))]
const TIMEOUT: Duration = Duration::from_secs(45);
#[cfg(test)]
const TIMEOUT: Duration = Duration::from_secs(1);

#[cfg(all(test, unix))]
#[path = "web_auth_tests.rs"]
mod tests;

pub fn cookies_path() -> PathBuf {
    crate::wrapper::user_data_dir()
        .join("web-auth")
        .join("cookies.txt")
}

// Ship the exact reviewed helper inside the binary, independent of source/install paths.
const HELPER: &str = include_str!("../tools/apple_web_session.py");

fn python_path() -> PathBuf {
    if let Some(path) = std::env::var_os("IPOD_PYTHON") {
        return path.into();
    }
    let sibling = crate::wrapper::user_data_dir().join("gamdl/bin/python");
    if sibling.is_file() {
        sibling
    } else {
        PathBuf::from("python3")
    }
}

fn run(args: &[&str], cancelled: Option<&AtomicBool>) -> io::Result<String> {
    let is_cancelled = || cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed));
    if is_cancelled() {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let mut command = Command::new(python_path());
    command
        .arg("-c")
        .arg(HELPER)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| io::Error::other("session helper unavailable"))?;
    let result = (|| {
        use std::io::Read;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("session helper unavailable"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // Nonblocking reads keep both output and the wall-clock wait bounded;
            // no reader thread can leak if a descendant retains the pipe.
            let fd = stdout.as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::other("session helper pipe unavailable"));
            }
        }
        #[cfg(not(unix))]
        return Err(io::Error::other("session helper requires a Unix platform"));
        let start = Instant::now();
        let mut output = Vec::new();
        let mut eof = false;
        loop {
            if is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            if start.elapsed() >= TIMEOUT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "session helper timed out",
                ));
            }
            let mut buffer = [0; 257];
            if !eof {
                match stdout.read(&mut buffer) {
                    Ok(0) => eof = true,
                    Ok(n) => {
                        if output.len() + n > 256 {
                            return Err(io::Error::other("invalid session helper response"));
                        }
                        output.extend_from_slice(&buffer[..n]);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(io::Error::other("session helper pipe failed")),
                }
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|_| io::Error::other("session helper wait failed"))?
            {
                if !status.success() {
                    return Err(io::Error::other("session helper failed"));
                }
                if eof {
                    return String::from_utf8(output)
                        .map(|text| text.trim().to_owned())
                        .map_err(|_| io::Error::other("invalid session helper response"));
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    // Always terminate descendants and reap the direct child, even on pipe errors.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    status: String,
    #[serde(default)]
    reason: Option<String>,
}

fn parse(response: &str) -> io::Result<Response> {
    serde_json::from_str::<Response>(response)
        .map_err(|_| io::Error::other("invalid session helper response"))
}

fn status(response: &str) -> io::Result<String> {
    parse(response).map(|response| response.status)
}

/// User-facing explanation for an allowlisted helper import reason. Unknown
/// reasons fall back to a generic message; helper text is never echoed.
pub(crate) fn import_reason_message(reason: Option<&str>) -> &'static str {
    match reason {
        Some("not_found") => {
            "Cookie file not found. Check the path; you can drag the file into the terminal to paste its location."
        }
        Some("too_large") => {
            "That cookie file is over 1 MB. Export only the current site while on music.apple.com."
        }
        Some("not_cookie_file") => {
            "That file is not a Netscape cookies.txt export. Use a cookies.txt exporter while on music.apple.com."
        }
        Some("no_music_token") => {
            "No Apple Music sign-in cookie (media-user-token) in that file. Sign in at music.apple.com in the browser, then export while on music.apple.com."
        }
        Some("expired") => {
            "The Apple Music sign-in cookie in that file has expired. Sign in again in the browser and export a fresh file."
        }
        Some("duplicate_token") => {
            "That file has more than one Apple Music sign-in cookie. Export from a single browser profile/container."
        }
        Some("unsafe_location") => {
            "Could not store the session privately: the app's data folder has unsafe permissions."
        }
        _ => {
            "Could not import Apple Music cookies. Choose a local Netscape cookie export; never paste a password or token."
        }
    }
}

/// Accept paths the way people paste them: surrounding quotes from a file
/// manager's "copy path", a file:// URI, stray whitespace, or a leading `~/`.
pub(crate) fn normalize_cookie_path(source: &str, home: Option<&Path>) -> PathBuf {
    let mut text = source.trim();
    for quote in ['\'', '"'] {
        if text.len() >= 2 && text.starts_with(quote) && text.ends_with(quote) {
            text = &text[1..text.len() - 1];
            break;
        }
    }
    let decoded;
    if let Some(rest) = text.strip_prefix("file://") {
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        decoded = percent_decode(rest);
        text = &decoded;
    }
    match (text.strip_prefix("~/"), home) {
        (Some(tail), Some(home)) => home.join(tail),
        _ => PathBuf::from(text),
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(value) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn import_cookies(source: &str) -> io::Result<()> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let input = normalize_cookie_path(source, home.as_deref());
    if !input.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            import_reason_message(Some("not_found")),
        ));
    }
    let target = cookies_path();
    let result = run(
        &[
            "import",
            input.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid cookie path")
            })?,
            target
                .to_str()
                .ok_or_else(|| io::Error::other("invalid session path"))?,
        ],
        None,
    )?;
    let response = parse(&result)?;
    if response.status == "imported" {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            import_reason_message(response.reason.as_deref()),
        ))
    }
}

/// Whether a locally valid private cookie file exists; not proof of login.
pub fn has_session() -> bool {
    let path = cookies_path();
    path.to_str()
        .and_then(|path| run(&["present", path], None).ok())
        .and_then(|response| status(&response).ok())
        .is_some_and(|status| status == "present")
}

pub fn forget() -> io::Result<()> {
    let path = cookies_path();
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::other("invalid session path"))?;
    if status(&run(&["forget", path], None)?)? == "forgotten" {
        Ok(())
    } else {
        Err(io::Error::other("could not forget private session"))
    }
}

pub fn open_login() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    let mut child = command
        .arg("https://music.apple.com")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| io::Error::other("could not open system browser"))?;
    // System openers may remain alive while the browser runs. Never block the
    // TUI, kill a user's browser, or drop an exited Child without reaping it.
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let path = cookies_path();
    let path = path.to_str().ok_or_else(|| "error".to_string())?;
    let response = run(&["check", path], Some(cancelled)).map_err(|error| {
        if error.kind() == io::ErrorKind::Interrupted {
            if cancelled.load(Ordering::Relaxed) {
                "cancelled"
            } else {
                "error"
            }
            .to_string()
        } else {
            "error".to_string()
        }
    })?;
    match status(&response).as_deref() {
        Ok("authenticated") => Ok(()),
        Ok("sign_in_required") => Err("sign_in_required".into()),
        Ok("subscription_required") => Err("subscription_required".into()),
        _ => Err("error".into()),
    }
}
