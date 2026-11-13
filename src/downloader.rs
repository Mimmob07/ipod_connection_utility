//! Apple Music song downloads through a local wrapper-v2 session and gamdl.

use crate::wrapper::user_data_dir;
use serde_json::{Value, json};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

static STAGING_ID: AtomicU64 = AtomicU64::new(0);

mod playlist;
mod progress;
mod storage;
pub use playlist::{PlaylistContext, PlaylistDetails, resolve_playlist, resolve_playlist_wrapper};
pub use progress::DownloadProgress;

pub struct Download {
    pub source: String,
    pub source_id: String,
    pub kind: SourceKind,
    pub quality: Quality,
    pub rockbox_art: bool,
    pub library_path: PathBuf,
    pub status: DownloadStatus,
    pub title: String,
    pub artist: String,
    pub playlist: Option<PlaylistContext>,
    pub percent: Option<u8>,
    pub phase: String,
}

pub struct DownloadTask {
    pub index: usize,
    pub source: String,
    pub id: String,
    pub kind: SourceKind,
    pub options: DownloadOptions,
    pub library: PathBuf,
    pub playlist: Option<PlaylistContext>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Song,
    Playlist,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Aac256,
    HeAac64,
    Alac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadBackend {
    Web,
    Wrapper,
}

impl DownloadBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Web => "Basic AAC (no Docker)",
            Self::Wrapper => "Advanced ALAC (wrapper)",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DownloadOptions {
    pub quality: Quality,
    pub rockbox_art: bool,
}

impl DownloadOptions {
    pub fn backend(self) -> DownloadBackend {
        self.quality.backend()
    }
}

impl Quality {
    pub fn backend(self) -> DownloadBackend {
        match self {
            Self::Alac => DownloadBackend::Wrapper,
            Self::Aac256 | Self::HeAac64 => DownloadBackend::Web,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Aac256 => Self::HeAac64,
            Self::HeAac64 => Self::Alac,
            Self::Alac => Self::Aac256,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Aac256 => "AAC 256 kbps",
            Self::HeAac64 => "HE-AAC 64 kbps",
            Self::Alac => "ALAC lossless",
        }
    }

    fn codec(self) -> &'static str {
        match self {
            Self::Aac256 => "aac-web",
            Self::HeAac64 => "aac-he-web",
            Self::Alac => "alac",
        }
    }

    fn audio_codec(self) -> &'static str {
        match self {
            Self::Alac => "alac",
            _ => "aac",
        }
    }
}

#[derive(Debug)]
pub struct DownloadOutcome {
    pub saved: usize,
    pub existing: usize,
    pub unavailable: usize,
    pub other_skipped: usize,
    pub invalid: usize,
    pub rate_limited: bool,
    pub issues: Vec<String>,
}

impl DownloadOutcome {
    pub fn detail(&self) -> String {
        format!(
            "{} saved, {} already present, {} unavailable, {} other skipped, {} invalid{}. Press s to scan",
            self.saved,
            self.existing,
            self.unavailable,
            self.other_skipped,
            self.invalid,
            if self.rate_limited {
                ", rate-limit warning"
            } else {
                ""
            }
        )
    }

    fn partial(&self) -> bool {
        self.unavailable + self.other_skipped + self.invalid > 0
    }
}

pub enum DownloadStatus {
    Queued,
    Resolving,
    Downloading,
    Unavailable(String),
    Blocked(String),
    SignInRequired,
    Finished(DownloadOutcome),
    Failed(String),
}

impl fmt::Display for DownloadStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queued => write!(f, "Queued"),
            Self::Resolving => write!(f, "Loading songs"),
            Self::Downloading => write!(f, "Downloading"),
            Self::Unavailable(_) => write!(f, "Unavailable"),
            Self::Blocked(_) => write!(f, "Library blocked"),
            Self::SignInRequired => write!(f, "Sign in"),
            Self::Finished(outcome) if outcome.partial() => write!(f, "Partial"),
            Self::Finished(outcome) if outcome.saved > 0 => write!(f, "Downloaded"),
            Self::Finished(_) => write!(f, "Already saved"),
            Self::Failed(_) => write!(f, "Failed"),
        }
    }
}

#[derive(Debug)]
pub enum DownloadError {
    InvalidUrl,
    Duplicate,
    InvalidLibrary,
    LibraryNotWritable(String),
    WrapperUnavailable,
    SignInRequired,
    GamdlFailed,
    SubscriptionRequired,
    RateLimited(String),
    Unavailable,
    Backend(String),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "enter an Apple Music song or playlist URL"),
            Self::Duplicate => write!(f, "this URL is already queued"),
            Self::InvalidLibrary => write!(f, "local library folder was not found"),
            Self::LibraryNotWritable(message) => write!(f, "{message}"),
            Self::WrapperUnavailable => write!(
                f,
                "wrapper-v2 is not available at 127.0.0.1:{}",
                wrapper_port()
            ),
            Self::SignInRequired => write!(f, "sign in on the Downloader tab (press c)"),
            Self::GamdlFailed => write!(f, "gamdl failed; check the track and subscription"),
            Self::SubscriptionRequired => {
                write!(f, "an active Apple Music subscription is required")
            }
            Self::RateLimited(reason) => write!(f, "{reason}; wait before retrying"),
            Self::Unavailable => write!(
                f,
                "no streamable tracks found (possibly unavailable in this storefront)"
            ),
            Self::Backend(message) => write!(f, "{message}"),
        }
    }
}

pub struct Downloader {
    queue: Vec<Download>,
    quality: Quality,
    rockbox_art: bool,
}

impl Default for Downloader {
    fn default() -> Self {
        Self {
            queue: Vec::new(),
            quality: Quality::Aac256,
            rockbox_art: true,
        }
    }
}

impl Downloader {
    pub const IMPLEMENTED: bool = true;

    pub fn queue(&self) -> &[Download] {
        &self.queue
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn backend(&self) -> DownloadBackend {
        self.quality.backend()
    }

    pub fn cycle_quality(&mut self) {
        self.quality = self.quality.next();
    }

    pub fn rockbox_art(&self) -> bool {
        self.rockbox_art
    }

    pub fn toggle_rockbox_art(&mut self) {
        self.rockbox_art = !self.rockbox_art;
    }

    pub fn last_detail(&self) -> Option<String> {
        self.queue.iter().rev().find_map(|item| match &item.status {
            DownloadStatus::Finished(outcome) => Some(outcome.detail()),
            DownloadStatus::Failed(error) => Some(format!("Failed: {error}")),
            DownloadStatus::Blocked(error) => {
                Some(format!("Library blocked: {error}. Press t to retry"))
            }
            DownloadStatus::SignInRequired => Some("Sign in required; press c".into()),
            _ => None,
        })
    }

    pub fn enqueue(&mut self, source: &str, library_path: &str) -> Result<(), DownloadError> {
        let (kind, source_id) = parse_source(source).ok_or(DownloadError::InvalidUrl)?;
        if !Path::new(library_path).is_dir() {
            return Err(DownloadError::InvalidLibrary);
        }
        if self.queue.iter().any(|item| {
            item.library_path == Path::new(library_path)
                && ((item.kind == kind && item.source_id == source_id && item.playlist.is_none())
                    || (kind == SourceKind::Playlist
                        && item
                            .playlist
                            .as_ref()
                            .is_some_and(|playlist| playlist.id == source_id)))
                && !matches!(item.status, DownloadStatus::Failed(_))
        }) {
            return Err(DownloadError::Duplicate);
        }
        self.queue.push(Download {
            source: source.to_string(),
            source_id,
            kind,
            quality: self.quality,
            rockbox_art: self.rockbox_art,
            library_path: PathBuf::from(library_path),
            status: DownloadStatus::Queued,
            title: if kind == SourceKind::Playlist {
                "Playlist (waiting to load songs)".into()
            } else {
                source.into()
            },
            artist: String::new(),
            playlist: None,
            percent: None,
            phase: String::new(),
        });
        Ok(())
    }

    pub fn next_backend(&self) -> Option<DownloadBackend> {
        self.queue
            .iter()
            .find(|item| matches!(item.status, DownloadStatus::Queued))
            .map(|item| item.quality.backend())
    }

    pub fn next_queued(&mut self) -> Option<DownloadTask> {
        let (index, item) = self
            .queue
            .iter_mut()
            .enumerate()
            .find(|(_, item)| matches!(item.status, DownloadStatus::Queued))?;
        item.status = if item.kind == SourceKind::Playlist {
            DownloadStatus::Resolving
        } else {
            DownloadStatus::Downloading
        };
        Some(DownloadTask {
            index,
            source: item.source.clone(),
            id: item.source_id.clone(),
            kind: item.kind,
            options: DownloadOptions {
                quality: item.quality,
                rockbox_art: item.rockbox_art,
            },
            library: item.library_path.clone(),
            playlist: item.playlist.clone(),
        })
    }

    pub fn expand_playlist(
        &mut self,
        index: usize,
        details: PlaylistDetails,
    ) -> Result<usize, DownloadError> {
        let item = self.queue.get(index).ok_or(DownloadError::InvalidUrl)?;
        playlist::validate_details(&item.source_id, &details)?;
        let context = PlaylistContext {
            id: details.id,
            title: details.title,
            folder: details.folder,
        };
        let count = details.tracks.len();
        let tracks: Vec<_> = details
            .tracks
            .into_iter()
            .map(|track| Download {
                source: track.source,
                source_id: track.id,
                kind: SourceKind::Song,
                title: track.title,
                artist: track.artist,
                playlist: Some(context.clone()),
                quality: item.quality,
                rockbox_art: item.rockbox_art,
                library_path: item.library_path.clone(),
                status: match track.unavailable {
                    Some(reason) => DownloadStatus::Unavailable(reason),
                    None => DownloadStatus::Queued,
                },
                percent: None,
                phase: String::new(),
            })
            .collect();
        self.queue.splice(index..=index, tracks);
        Ok(count)
    }

    pub fn update_progress(&mut self, index: usize, progress: DownloadProgress) {
        if let Some(item) = self.queue.get_mut(index) {
            item.percent = progress.percent;
            item.phase = progress.phase.into();
            if let Some(title) = progress.title {
                item.title = title;
            }
        }
    }

    pub fn retry_failed(&mut self) {
        for item in &mut self.queue {
            if matches!(
                item.status,
                DownloadStatus::Failed(_) | DownloadStatus::Blocked(_)
            ) {
                item.status = DownloadStatus::Queued;
                item.percent = None;
                item.phase.clear();
            }
        }
    }

    pub fn completed(&self) -> usize {
        self.queue
            .iter()
            .filter(|item| {
                matches!(
                    item.status,
                    DownloadStatus::Finished(_)
                        | DownloadStatus::Failed(_)
                        | DownloadStatus::Unavailable(_)
                )
            })
            .count()
    }

    pub fn finish(&mut self, index: usize, result: Result<DownloadOutcome, DownloadError>) {
        if let Err(DownloadError::LibraryNotWritable(message)) = &result
            && let Some(library) = self.queue.get(index).map(|item| item.library_path.clone())
        {
            for (position, item) in self.queue.iter_mut().enumerate() {
                if item.library_path == library
                    && (position == index || matches!(item.status, DownloadStatus::Queued))
                {
                    item.status = DownloadStatus::Blocked(message.clone());
                }
            }
            return;
        }
        if let Some(item) = self.queue.get_mut(index) {
            item.status = match result {
                Ok(outcome) => DownloadStatus::Finished(outcome),
                Err(DownloadError::SignInRequired) => DownloadStatus::SignInRequired,
                Err(DownloadError::Unavailable) => {
                    DownloadStatus::Unavailable("No streamable audio available".into())
                }
                Err(error) => DownloadStatus::Failed(error.to_string()),
            };
        }
    }

    /// Requeue entries paused for sign-in on `backend` only; the web session
    /// and the advanced wrapper are independent credentials.
    pub fn retry_sign_in_required(&mut self, backend: DownloadBackend) {
        for item in &mut self.queue {
            if matches!(item.status, DownloadStatus::SignInRequired)
                && item.quality.backend() == backend
            {
                item.status = DownloadStatus::Queued;
            }
        }
    }

    /// Pause queued entries that need `backend`'s credential; entries captured
    /// for the other backend keep running.
    pub fn pause_for_sign_in(&mut self, backend: DownloadBackend) {
        for item in &mut self.queue {
            if matches!(item.status, DownloadStatus::Queued) && item.quality.backend() == backend {
                item.status = DownloadStatus::SignInRequired;
            }
        }
    }
}

fn parse_source(source: &str) -> Option<(SourceKind, String)> {
    if source.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let rest = source.strip_prefix("https://music.apple.com/")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut parts = path.split('/');
    let storefront = parts.next()?;
    let kind = parts.next()?;
    if storefront.len() != 2 || !storefront.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let slug = parts.next()?;
    let path_id = parts.next()?;
    if slug.is_empty() || parts.next().is_some() {
        return None;
    }
    match kind {
        "album" | "song" => {
            let id = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("i="))
                .or_else(|| (kind == "song").then_some(path_id))?;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            Some((SourceKind::Song, id.to_string()))
        }
        "playlist" => {
            if path_id.is_empty()
                || !path_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
            {
                return None;
            }
            Some((SourceKind::Playlist, path_id.to_string()))
        }
        _ => None,
    }
}

fn port(primary: &str, compose: &str, default: u16) -> u16 {
    let primary = std::env::var(primary).ok();
    let compose = std::env::var(compose).ok();
    port_values(primary.as_deref(), compose.as_deref(), default)
}

fn port_values(primary: Option<&str>, compose: Option<&str>, default: u16) -> u16 {
    primary
        .or(compose)
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(default)
}

fn wrapper_port() -> u16 {
    port("IPOD_WRAPPER_HTTP_PORT", "HTTP_PORT", 8080)
}
fn decrypt_port() -> u16 {
    port("IPOD_WRAPPER_DECRYPT_PORT", "DECRYPT_PORT", 10020)
}

/// Only speaks to loopback; response bodies can contain account tokens and are never logged.
fn wrapper_request(
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<(u16, Value), DownloadError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], wrapper_port()));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|_| DownloadError::WrapperUnavailable)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(75)))
        .map_err(|_| DownloadError::WrapperUnavailable)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| DownloadError::WrapperUnavailable)?;
    let data = body.map(|value| value.to_string()).unwrap_or_default();
    write!(stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",
        data.len()
    ).map_err(|_| DownloadError::WrapperUnavailable)?;
    let mut response = Vec::new();
    stream
        .take(1024 * 1024)
        .read_to_end(&mut response)
        .map_err(|_| DownloadError::WrapperUnavailable)?;
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(DownloadError::WrapperUnavailable)?;
    let header = String::from_utf8_lossy(&response[..separator]);
    let status = header
        .split_whitespace()
        .nth(1)
        .and_then(|part| part.parse().ok())
        .ok_or(DownloadError::WrapperUnavailable)?;
    let value = serde_json::from_slice(&response[separator + 4..])
        .map_err(|_| DownloadError::WrapperUnavailable)?;
    Ok((status, value))
}

#[derive(Debug)]
pub enum LoginResult {
    Authenticated,
    NeedsCode,
}

pub fn login(username: String, password: String) -> Result<LoginResult, DownloadError> {
    check_wrapper_for_login()?;
    let (status, _) = wrapper_request(
        "POST",
        "/login",
        Some(json!({
            "username": username, "password": password
        })),
    )?;
    match status {
        200 => Ok(LoginResult::Authenticated),
        202 => Ok(LoginResult::NeedsCode),
        401 => Err(DownloadError::Backend(
            "Apple Music rejected the login".into(),
        )),
        _ => Err(DownloadError::Backend(format!(
            "wrapper login failed (HTTP {status})"
        ))),
    }
}

pub fn login_code(code: String) -> Result<LoginResult, DownloadError> {
    check_wrapper_for_login()?;
    let (status, _) = wrapper_request("POST", "/login/2fa", Some(json!({ "code": code })))?;
    match status {
        200 => Ok(LoginResult::Authenticated),
        202 => Ok(LoginResult::NeedsCode),
        401 => Err(DownloadError::Backend(
            "Apple Music rejected the code".into(),
        )),
        _ => Err(DownloadError::Backend(format!(
            "wrapper code failed (HTTP {status})"
        ))),
    }
}

fn check_session() -> Result<(), DownloadError> {
    let (status, value) = wrapper_request("GET", "/me", None)?;
    if status != 200 {
        return Err(DownloadError::WrapperUnavailable);
    }
    if value["runtime"]["playback_ready"] != true {
        return Err(DownloadError::Backend(
            "wrapper playback is unavailable; check its setup".into(),
        ));
    }
    if value["auth"]["state"] != "authenticated" {
        return Err(DownloadError::SignInRequired);
    }
    Ok(())
}

/// Clear only the wrapper's in-memory token snapshot, then ask it to restore
/// fresh tokens from its persistent Apple session. The on-disk session stays in place.
fn refresh_session() -> Result<(), DownloadError> {
    let (status, _) = wrapper_request("DELETE", "/login", None)?;
    if status != 200 {
        return Err(DownloadError::WrapperUnavailable);
    }
    check_session()
}

fn check_wrapper_for_login() -> Result<(), DownloadError> {
    let (status, value) = wrapper_request("GET", "/me", None)?;
    if status != 200
        || !value["version"].is_string()
        || !value["auth"]["state"].is_string()
        || value["runtime"]["playback_ready"] != true
    {
        return Err(DownloadError::Backend(
            "wrapper-v2 is not ready for sign in".into(),
        ));
    }
    Ok(())
}

struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wait_child(
    child: &mut Child,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<ExitStatus, DownloadError> {
    wait_child_reporting(child, cancelled, timeout, &mut || Ok(()))
}

fn wait_child_reporting(
    child: &mut Child,
    cancelled: &AtomicBool,
    timeout: Duration,
    on_tick: &mut dyn FnMut() -> Result<(), DownloadError>,
) -> Result<ExitStatus, DownloadError> {
    let started = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DownloadError::Backend("download cancelled".into()));
        }
        if let Err(error) = on_tick() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                on_tick()?;
                return Ok(status);
            }
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DownloadError::Backend("download timed out".into()));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DownloadError::Backend(format!(
                    "child process failed: {error}"
                )));
            }
        }
    }
}

fn web_cookies_path() -> PathBuf {
    crate::web_auth::cookies_path()
}

pub fn check_web_session(cancelled: &AtomicBool) -> Result<LoginResult, DownloadError> {
    crate::web_auth::check(cancelled).map(|()| LoginResult::Authenticated).map_err(|error| match error.as_str() {
        "sign_in_required" => DownloadError::SignInRequired,
        "subscription_required" => DownloadError::SubscriptionRequired,
        "cancelled" => DownloadError::Backend("session check cancelled".into()),
        _ => DownloadError::Backend("Could not check Apple Music web session; check your connection and basic downloader setup".into()),
    })
}

pub fn download(
    task: &DownloadTask,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<DownloadOutcome, DownloadError> {
    storage::check_writable(&task.library)?;
    match download_once(task, cancelled, on_progress) {
        Err(DownloadError::SignInRequired)
            if task.options.backend() == DownloadBackend::Wrapper
                && !cancelled.load(Ordering::Relaxed) =>
        {
            refresh_session()?;
            download_once(task, cancelled, on_progress)
        }
        result => result,
    }
}

fn download_once(
    task: &DownloadTask,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<DownloadOutcome, DownloadError> {
    let source = &task.source;
    let id = &task.id;
    let kind = task.kind;
    let options = task.options;
    let library = &task.library;
    on_progress(DownloadProgress::phase("Checking session"));
    match options.backend() {
        DownloadBackend::Wrapper => check_session()?,
        DownloadBackend::Web if !web_cookies_path().is_file() => {
            return Err(DownloadError::SignInRequired);
        }
        DownloadBackend::Web => {}
    }
    let serial = STAGING_ID.fetch_add(1, Ordering::Relaxed);
    let staging_path = library.join(format!(
        ".ipod-download-{}-{serial}-{id}",
        std::process::id()
    ));
    create_private_dir(&staging_path)
        .map_err(|error| DownloadError::Backend(format!("cannot stage download: {error}")))?;
    let staging = Staging(staging_path);
    let output = staging.0.join("output");
    let temp = staging.0.join("temp");
    fs::create_dir(&output)
        .and_then(|_| fs::create_dir(&temp))
        .map_err(|error| DownloadError::Backend(format!("cannot stage download: {error}")))?;

    let log_path = staging.0.join("gamdl.log");
    let log_file = fs::File::create(&log_path)
        .map_err(|error| DownloadError::Backend(format!("cannot capture gamdl output: {error}")))?;
    let error_file = log_file
        .try_clone()
        .map_err(|error| DownloadError::Backend(format!("cannot capture gamdl errors: {error}")))?;

    let executable = gamdl_executable();
    let folder_template = match kind {
        SourceKind::Song => "Staged",
        SourceKind::Playlist => "Playlists/{playlist_title} by {playlist_artist}",
    };
    let mut command = Command::new(executable);
    command.env("PYTHONUNBUFFERED", "1").arg("--no-config-file");
    match options.backend() {
        DownloadBackend::Web => {
            command.arg("--cookies-path").arg(web_cookies_path());
        }
        DownloadBackend::Wrapper => {
            command
                .arg("--use-wrapper")
                .arg("--wrapper-url")
                .arg(format!("http://127.0.0.1:{}", wrapper_port()))
                .arg("--wrapper-decrypt-host")
                .arg("127.0.0.1")
                .arg("--wrapper-decrypt-port")
                .arg(decrypt_port().to_string());
        }
    }
    let mut child = command
        .arg("--song-codec-priority")
        .arg(options.quality.codec())
        .arg("--album-folder-template")
        .arg(folder_template)
        .arg("--compilation-folder-template")
        .arg(folder_template)
        .arg("--no-album-folder-template")
        .arg(folder_template)
        .arg("--single-disc-file-template")
        .arg("{title_id}")
        .arg("--multi-disc-file-template")
        .arg("{title_id}")
        .arg("--no-album-file-template")
        .arg("{title_id}")
        .arg("--no-synced-lyrics")
        .arg("--output-path")
        .arg(&output)
        .arg("--temp-path")
        .arg(&temp)
        .arg(source)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(error_file))
        .spawn()
        .map_err(|error| DownloadError::Backend(format!("cannot start gamdl: {error}")))?;
    let timeout = match kind {
        SourceKind::Song => Duration::from_secs(15 * 60),
        SourceKind::Playlist => Duration::from_secs(4 * 60 * 60),
    };
    on_progress(DownloadProgress::phase("Downloading"));
    let status = progress::wait_download(&mut child, cancelled, timeout, &log_path, on_progress)?;
    let log = fs::read_to_string(&log_path).unwrap_or_default();
    let log_summary = summarize_log(&log);
    if log_summary.auth_failed {
        return Err(DownloadError::SignInRequired);
    }
    if log_summary.subscription_required {
        return Err(DownloadError::SubscriptionRequired);
    }
    let files: Vec<PathBuf> = WalkDir::new(&output)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file() && entry.path().extension().is_some_and(|ext| ext == "m4a")
        })
        .map(|entry| entry.into_path())
        .collect();
    let mut outcome = DownloadOutcome {
        saved: 0,
        existing: 0,
        unavailable: log_summary.unavailable,
        other_skipped: log_summary.other_skipped,
        invalid: 0,
        rate_limited: log_summary.rate_limited,
        issues: log_summary.issues,
    };
    if !status.success() {
        outcome.other_skipped += 1;
        outcome
            .issues
            .push("gamdl stopped before completing the URL".into());
    }
    for file in files {
        if cancelled.load(Ordering::Relaxed) {
            return Err(DownloadError::Backend("download cancelled".into()));
        }
        let Some(track_id) = file
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| valid_track_id(name))
        else {
            outcome.invalid += 1;
            continue;
        };
        if kind == SourceKind::Song && track_id != id {
            outcome.invalid += 1;
            continue;
        }
        if fs::metadata(&file).map_or(true, |meta| meta.len() == 0) {
            outcome.invalid += 1;
            continue;
        }
        on_progress(DownloadProgress::phase("Checking audio"));
        if !verify_audio(&file, options.quality, cancelled)? {
            outcome.invalid += 1;
            continue;
        }
        let playlist_folder = if let Some(playlist) = &task.playlist {
            Some(playlist.folder.as_path())
        } else if kind == SourceKind::Playlist {
            file.parent()
                .and_then(|parent| parent.strip_prefix(&output).ok())
        } else {
            None
        };
        on_progress(DownloadProgress::phase("Preparing artwork"));
        let prepared =
            match prepare_download(&file, playlist_folder, options.rockbox_art, cancelled) {
                Ok(prepared) => prepared,
                Err(error) => {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err(error);
                    }
                    outcome.other_skipped += 1;
                    if outcome.issues.len() < 10 {
                        outcome.issues.push(format!("Track {track_id}: {error}"));
                    }
                    continue;
                }
            };
        on_progress(DownloadProgress::phase("Saving"));
        match save_prepared(
            &file,
            library,
            &prepared,
            track_id,
            options.quality,
            cancelled,
        )? {
            true => outcome.saved += 1,
            false => outcome.existing += 1,
        }
    }
    if outcome.saved + outcome.existing == 0 {
        if outcome.rate_limited {
            return Err(DownloadError::RateLimited(
                outcome
                    .issues
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Backend reported rate limiting".into()),
            ));
        }
        if outcome.unavailable > 0 {
            return Err(DownloadError::Unavailable);
        }
        if !status.success() {
            return Err(DownloadError::GamdlFailed);
        }
        if outcome.other_skipped > 0 {
            return Err(DownloadError::Backend(
                outcome.issues.first().cloned().unwrap_or_else(|| {
                    "all tracks were skipped; check selected quality and account access".into()
                }),
            ));
        }
        return Err(DownloadError::Backend(
            "gamdl produced no valid m4a tracks".into(),
        ));
    }
    Ok(outcome)
}

fn save_prepared(
    file: &Path,
    library: &Path,
    prepared: &PreparedDownload,
    track_id: &str,
    quality: Quality,
    cancelled: &AtomicBool,
) -> Result<bool, DownloadError> {
    let mut destination = library.join(&prepared.relative_path);
    if destination.exists() {
        if existing_audio_matches(&destination, &prepared.track_id, quality, cancelled)? {
            return Ok(false);
        }
        destination.set_file_name(format!(
            "{} [{track_id}].m4a",
            prepared
                .relative_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    }
    fs::create_dir_all(
        destination
            .parent()
            .ok_or_else(|| DownloadError::Backend("Missing destination folder".into()))?,
    )
    .map_err(|error| {
        DownloadError::LibraryNotWritable(format!(
            "cannot create music folder {}: {error}",
            destination.display()
        ))
    })?;
    match storage::save_m4a(file, &destination, cancelled)? {
        true => Ok(true),
        false if existing_audio_matches(&destination, &prepared.track_id, quality, cancelled)? => {
            Ok(false)
        }
        false => Err(DownloadError::Backend(format!(
            "existing file is not verified audio for track {track_id}; preserved without overwriting: {}",
            destination.display()
        ))),
    }
}

fn existing_audio_matches(
    destination: &Path,
    track_id: &str,
    quality: Quality,
    cancelled: &AtomicBool,
) -> Result<bool, DownloadError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(DownloadError::Backend("download cancelled".into()));
    }
    if track_id.is_empty() {
        return Ok(false);
    }
    let metadata = match prepare_download(destination, None, false, cancelled) {
        Ok(metadata) => metadata,
        Err(error) if cancelled.load(Ordering::Relaxed) => return Err(error),
        Err(_) => return Ok(false),
    };
    if metadata.track_id != track_id {
        return Ok(false);
    }
    verify_audio(destination, quality, cancelled)
}

fn valid_track_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

fn gamdl_executable() -> PathBuf {
    if let Some(path) = std::env::var_os("IPOD_GAMDL") {
        return path.into();
    }
    let installed = user_data_dir().join("gamdl/bin/gamdl");
    if installed.is_file() {
        return installed;
    }
    // Backward compatibility with the original host-specific setup path.
    if let Some(home) = std::env::var_os("HOME") {
        let installed = PathBuf::from(home).join(".local/share/ipod-gamdl/bin/gamdl");
        if installed.is_file() {
            return installed;
        }
    }
    PathBuf::from("gamdl")
}

fn python_executable() -> PathBuf {
    std::env::var_os("IPOD_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let sibling = gamdl_executable().with_file_name("python");
            if sibling.is_file() {
                sibling
            } else {
                PathBuf::from("python3")
            }
        })
}

struct PreparedDownload {
    relative_path: PathBuf,
    track_id: String,
}

fn prepare_download(
    file: &Path,
    playlist_folder: Option<&Path>,
    rockbox_art: bool,
    cancelled: &AtomicBool,
) -> Result<PreparedDownload, DownloadError> {
    let mut command = Command::new(python_executable());
    command
        .arg("-c")
        .arg(include_str!("../tools/prepare_download.py"))
        .arg(file);
    if let Some(folder) = playlist_folder {
        command.arg("--playlist-folder").arg(folder);
    }
    if rockbox_art {
        command.arg("--rockbox-art");
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            DownloadError::Backend(format!("cannot start song preparation: {error}"))
        })?;
    let status = wait_child(&mut child, cancelled, Duration::from_secs(60))?;
    if !status.success() {
        return Err(DownloadError::Backend(
            "song naming or artwork conversion failed; check Python, mutagen and Pillow".into(),
        ));
    }
    let mut output = String::new();
    child
        .stdout
        .take()
        .ok_or_else(|| DownloadError::Backend("missing song preparation response".into()))?
        .take(16 * 1024)
        .read_to_string(&mut output)
        .map_err(|error| {
            DownloadError::Backend(format!("cannot read song preparation: {error}"))
        })?;
    let metadata: Value = serde_json::from_str(&output)
        .map_err(|_| DownloadError::Backend("invalid song preparation response".into()))?;
    let relative = PathBuf::from(
        metadata["relative_path"]
            .as_str()
            .ok_or_else(|| DownloadError::Backend("missing song filename".into()))?,
    );
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
        || relative
            .extension()
            .is_none_or(|extension| extension != "m4a")
    {
        return Err(DownloadError::Backend("unsafe song filename".into()));
    }
    Ok(PreparedDownload {
        relative_path: relative,
        track_id: metadata["track_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}

#[derive(Default)]
struct LogSummary {
    unavailable: usize,
    other_skipped: usize,
    rate_limited: bool,
    auth_failed: bool,
    subscription_required: bool,
    issues: Vec<String>,
}

fn rate_limit_signal(line: &str) -> Option<&'static str> {
    let lower = line.to_ascii_lowercase();
    // Track names and processing URLs are data, not backend diagnostics. Keep
    // only a trailing error after a quoted title, when one is present.
    let diagnostic = ["downloading \"", "processing \"", "skipping \""]
        .iter()
        .find_map(|marker| {
            lower
                .split_once(marker)
                .map(|(_, rest)| rest.split_once("\":").map_or("", |(_, error)| error))
        })
        .unwrap_or(&lower);
    let words = format!(
        " {} ",
        diagnostic
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    );
    if [
        " http error 429 ",
        " http 1 1 429 ",
        " http 1 0 429 ",
        " http 2 429 ",
        " http 2 0 429 ",
        " http 3 429 ",
        " status code 429 ",
        " status 429 ",
        " 429 too many requests ",
    ]
    .iter()
    .any(|pattern| words.contains(pattern))
    {
        return Some("Rate-limit signal: backend reported HTTP 429");
    }
    let warning_or_error = ["[warning]", "[error]", "[critical]"]
        .iter()
        .any(|level| lower.contains(level));
    if words.contains(" too many requests ")
        || words.contains(" rate limit exceeded ")
        || words.contains(" rate limit reached ")
        || words.contains(" rate limited ")
        || words.contains(" ratelimiterror ")
        || (warning_or_error && words.contains(" rate limit "))
    {
        return Some("Rate-limit signal: backend explicitly reported rate limiting");
    }
    None
}

fn summarize_log(log: &str) -> LogSummary {
    let mut result = LogSummary::default();
    for line in log.lines() {
        let lower = line.to_ascii_lowercase();
        if !result.rate_limited
            && let Some(reason) = rate_limit_signal(line)
        {
            result.rate_limited = true;
            result.issues.insert(0, reason.into());
        }
        if lower.contains("unauthorized")
            || lower.contains("not authenticated")
            || lower.contains("session expired")
        {
            result.auth_failed = true;
        }
        if lower.contains("no active apple music subscription") {
            result.subscription_required = true;
        }
        if lower.contains("skipping ") {
            let unavailable = lower.contains("not streamable")
                || lower.contains("not available in your country")
                || lower.contains("not available in your region");
            if unavailable {
                result.unavailable += 1;
            } else {
                result.other_skipped += 1;
            }
            if result.issues.len() < 10
                && let Some(title) = line
                    .split("Skipping \"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
            {
                let label = if unavailable {
                    "Unavailable/stream not offered"
                } else {
                    "Skipped"
                };
                result.issues.push(format!(
                    "{label}: {}",
                    title.chars().take(80).collect::<String>()
                ));
            }
        } else if lower.contains("error downloading \"") {
            result.other_skipped += 1;
            if result.issues.len() < 10
                && let Some(title) = line
                    .split("Error downloading \"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
            {
                result.issues.push(format!(
                    "Download failed: {}",
                    title.chars().take(80).collect::<String>()
                ));
            }
        } else if lower.contains("error processing \"") {
            result.other_skipped += 1;
        }
    }
    result
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

fn verify_audio(
    file: &Path,
    quality: Quality,
    cancelled: &AtomicBool,
) -> Result<bool, DownloadError> {
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| DownloadError::Backend(format!("cannot inspect m4a: {error}")))?;
    if !probe.status.success()
        || String::from_utf8_lossy(&probe.stdout).trim() != quality.audio_codec()
    {
        return Ok(false);
    }
    let mut decoder = Command::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-err_detect", "explode", "-i"])
        .arg(file)
        .args(["-map", "0:a:0", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| DownloadError::Backend(format!("cannot verify m4a: {error}")))?;
    Ok(wait_child(&mut decoder, cancelled, Duration::from_secs(15 * 60))?.success())
}

#[cfg(test)]
#[path = "downloader/web_tests.rs"]
mod web_tests;

#[cfg(test)]
#[path = "downloader/save_tests.rs"]
mod save_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sync::tests::Fixture;

    #[test]
    fn default_aac_requires_web_sign_in_not_a_wrapper() {
        const CHILD: &str = "IPOD_TEST_WEB_DEFAULT_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let root =
                std::env::temp_dir().join(format!("ipod-web-default-state-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "downloader::tests::default_aac_requires_web_sign_in_not_a_wrapper",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("XDG_DATA_HOME", &root)
                .env("IPOD_WRAPPER_HTTP_PORT", port.to_string())
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
        let fixture = Fixture::new("web_default_download");
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
            matches!(result, Err(DownloadError::SignInRequired)),
            "default AAC must ask for a web session, not contact wrapper: {result:?}"
        );
        let playlist = resolve_playlist("pl.example", &AtomicBool::new(false));
        assert!(
            matches!(playlist, Err(DownloadError::SignInRequired)),
            "basic playlist must use web sign-in, not wrapper: {playlist:?}"
        );
    }

    #[test]
    fn wrapper_ports_use_primary_then_compose_then_default() {
        assert_eq!(port_values(None, None, 8080), 8080);
        assert_eq!(port_values(None, Some("9000"), 8080), 9000);
        assert_eq!(port_values(Some("9001"), Some("9000"), 8080), 9001);
        assert_eq!(port_values(Some("0"), Some("9000"), 8080), 8080);
        assert_eq!(port_values(None, Some("bad"), 8080), 8080);
    }

    #[test]
    fn validates_song_and_playlist_links() {
        assert!(valid_track_id("i.ABC-123"));
        assert!(!valid_track_id("../bad"));
        assert_eq!(
            parse_source("https://music.apple.com/us/album/example/123?i=456&foo=bar"),
            Some((SourceKind::Song, "456".into()))
        );
        assert_eq!(
            parse_source("https://music.apple.com/us/song/example/456"),
            Some((SourceKind::Song, "456".into()))
        );
        assert_eq!(
            parse_source("https://music.apple.com/us/playlist/example/pl.u-ABC123"),
            Some((SourceKind::Playlist, "pl.u-ABC123".into()))
        );
        for bad in [
            "https://example.com/us/album/example/123?i=456",
            "https://music.apple.com/us/album/example/123",
            "https://music.apple.com/us/playlist/example/../bad",
            "https://music.apple.com/us/album/example/123?i=oops",
        ] {
            assert!(parse_source(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn queue_waits_for_sign_in_then_records_the_saved_file() {
        let fixture = Fixture::new("downloader_queue");
        let source = "https://music.apple.com/us/album/example/123?i=456";
        let mut downloader = Downloader::default();
        downloader.enqueue(source, &fixture.lib()).unwrap();
        assert!(matches!(
            downloader.enqueue(source, &fixture.lib()),
            Err(DownloadError::Duplicate)
        ));
        let task = downloader.next_queued().unwrap();
        let index = task.index;
        assert_eq!(task.id, "456");
        assert_eq!(task.kind, SourceKind::Song);
        assert_eq!(task.options.quality, Quality::Aac256);
        assert_eq!(task.library, PathBuf::from(fixture.lib()));
        downloader.finish(index, Err(DownloadError::SignInRequired));
        assert_eq!(
            downloader.last_detail().as_deref(),
            Some("Sign in required; press c")
        );
        downloader.retry_sign_in_required(DownloadBackend::Web);
        assert!(downloader.next_queued().is_some());
        downloader.finish(
            index,
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
        assert!(downloader.last_detail().unwrap().contains("1 saved"));
        assert!(downloader.next_queued().is_none());
    }

    #[test]
    fn sign_in_failure_pauses_remaining_queue() {
        let fixture = Fixture::new("downloader_sign_in_pause");
        let mut downloader = Downloader::default();
        downloader
            .enqueue("https://music.apple.com/us/song/one/1", &fixture.lib())
            .unwrap();
        downloader
            .enqueue("https://music.apple.com/us/song/two/2", &fixture.lib())
            .unwrap();
        let active = downloader.next_queued().unwrap();
        downloader.pause_for_sign_in(DownloadBackend::Web);
        downloader.finish(active.index, Err(DownloadError::SignInRequired));
        assert!(downloader.next_queued().is_none());
        downloader.retry_sign_in_required(DownloadBackend::Web);
        assert!(downloader.next_queued().is_some());
    }

    #[test]
    fn sign_in_pause_and_retry_are_scoped_to_one_backend() {
        let fixture = Fixture::new("downloader_sign_in_backend_scope");
        let mut downloader = Downloader::default();
        downloader
            .enqueue("https://music.apple.com/us/song/web/1", &fixture.lib())
            .unwrap();
        downloader.cycle_quality();
        downloader.cycle_quality();
        assert_eq!(downloader.backend(), DownloadBackend::Wrapper);
        downloader
            .enqueue("https://music.apple.com/us/song/alac/2", &fixture.lib())
            .unwrap();
        fn status(d: &Downloader, i: usize) -> &DownloadStatus {
            &d.queue()[i].status
        }

        // Losing the web session must not pause queued ALAC/wrapper work.
        downloader.pause_for_sign_in(DownloadBackend::Web);
        assert!(matches!(
            status(&downloader, 0),
            DownloadStatus::SignInRequired
        ));
        assert!(matches!(status(&downloader, 1), DownloadStatus::Queued));
        let next = downloader.next_queued().unwrap();
        assert_eq!(next.index, 1);
        downloader.finish(next.index, Err(DownloadError::SignInRequired));

        // Signing in to the wrapper must not requeue web entries, and vice versa.
        downloader.retry_sign_in_required(DownloadBackend::Wrapper);
        assert!(matches!(
            status(&downloader, 0),
            DownloadStatus::SignInRequired
        ));
        assert!(matches!(status(&downloader, 1), DownloadStatus::Queued));
        downloader.pause_for_sign_in(DownloadBackend::Wrapper);
        downloader.retry_sign_in_required(DownloadBackend::Web);
        assert!(matches!(status(&downloader, 0), DownloadStatus::Queued));
        assert!(matches!(
            status(&downloader, 1),
            DownloadStatus::SignInRequired
        ));
    }

    #[test]
    fn playlist_queue_captures_quality_and_logs_partial_failure() {
        let fixture = Fixture::new("downloader_playlist_queue");
        let mut downloader = Downloader::default();
        downloader.cycle_quality();
        downloader
            .enqueue(
                "https://music.apple.com/us/playlist/example/pl.123",
                &fixture.lib(),
            )
            .unwrap();
        assert_eq!(
            downloader.next_queued().unwrap().options.quality,
            Quality::HeAac64
        );
        let parsed = summarize_log(
            "[WARNING] [Track 1/2] Skipping \"First\": Media is not streamable: 123\n[WARNING] [Track 2/2] Skipping \"Second\": Requested format is not available\n[ERROR] 429 Too Many Requests",
        );
        assert_eq!(parsed.unavailable, 1);
        assert_eq!(parsed.other_skipped, 1);
        assert!(parsed.rate_limited);
        assert_eq!(parsed.issues.len(), 3);
        let failed = summarize_log(
            "[ERROR] [Track 2/3] Error downloading \"Third\"\n[CRITICAL] No active Apple Music subscription found",
        );
        assert_eq!(failed.other_skipped, 1);
        assert!(failed.subscription_required);
    }

    #[test]
    fn audio_check_accepts_playable_aac_and_rejects_invalid_files() {
        if Command::new("ffmpeg").arg("-version").output().is_err()
            || Command::new("ffprobe").arg("-version").output().is_err()
        {
            return;
        }
        let fixture = Fixture::new("downloader_audio_check");
        let library = PathBuf::from(fixture.lib());
        let valid = library.join("sample.m4a");
        let result = Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=0.1",
                "-c:a",
                "aac",
                "-y",
            ])
            .arg(&valid)
            .status()
            .unwrap();
        assert!(result.success());
        let cancelled = AtomicBool::new(false);
        assert!(verify_audio(&valid, Quality::Aac256, &cancelled).unwrap());
        assert!(!verify_audio(&valid, Quality::Alac, &cancelled).unwrap());
        let invalid = library.join("invalid.m4a");
        fs::write(&invalid, "not an audio file").unwrap();
        assert!(!verify_audio(&invalid, Quality::Aac256, &cancelled).unwrap());
    }

    pub(crate) fn test_playlist(count: usize) -> PlaylistDetails {
        serde_json::from_value(json!({
            "id": "pl.test", "title": "Test Mix", "folder": "Playlists/Test Mix by Curator",
            "tracks": (1..=count).map(|id| json!({
                "id": id.to_string(), "source": format!("https://music.apple.com/us/song/track/{id}"),
                "title": format!("Song {id:02}"), "artist": "Test Artist", "unavailable": null,
            })).collect::<Vec<_>>()
        })).unwrap()
    }

    #[test]
    fn expands_playlists_into_ordered_track_rows_with_captured_options() {
        let fixture = Fixture::new("playlist_expand");
        let mut downloader = Downloader::default();
        downloader.cycle_quality();
        downloader.toggle_rockbox_art();
        let url = "https://music.apple.com/us/playlist/test/pl.test";
        downloader.enqueue(url, &fixture.lib()).unwrap();
        let parent = downloader.next_queued().unwrap();
        assert_eq!(parent.kind, SourceKind::Playlist);
        assert!(matches!(
            downloader.queue()[0].status,
            DownloadStatus::Resolving
        ));
        let mut details = test_playlist(3);
        details.tracks[1].unavailable = Some("No catalog song".into());
        assert_eq!(downloader.expand_playlist(0, details).unwrap(), 3);
        assert_eq!(
            downloader
                .queue()
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["Song 01", "Song 02", "Song 03"]
        );
        assert!(matches!(
            downloader.queue()[1].status,
            DownloadStatus::Unavailable(_)
        ));
        assert!(matches!(
            downloader.enqueue(url, &fixture.lib()),
            Err(DownloadError::Duplicate)
        ));
        let song = downloader.next_queued().unwrap();
        assert_eq!(song.id, "1");
        assert_eq!(song.kind, SourceKind::Song);
        assert_eq!(song.options.quality, Quality::HeAac64);
        assert!(!song.options.rockbox_art);
        assert_eq!(song.library, PathBuf::from(fixture.lib()));
        assert_eq!(
            song.playlist.unwrap().folder,
            PathBuf::from("Playlists/Test Mix by Curator")
        );
        assert_eq!(downloader.next_queued().unwrap().id, "3");
        assert!(downloader.next_queued().is_none());
    }

    #[test]
    fn rejects_unsafe_playlist_metadata_before_replacing_queue() {
        let fixture = Fixture::new("playlist_unsafe");
        let mut downloader = Downloader::default();
        downloader
            .enqueue(
                "https://music.apple.com/us/playlist/test/pl.test",
                &fixture.lib(),
            )
            .unwrap();
        let mut details = test_playlist(1);
        details.folder = PathBuf::from("Playlists/../../escape");
        assert!(downloader.expand_playlist(0, details).is_err());
        assert_eq!(downloader.queue().len(), 1);
        assert_eq!(downloader.queue()[0].kind, SourceKind::Playlist);
        let mut details = test_playlist(1);
        details.tracks[0].source = "https://example.com/song".into();
        assert!(downloader.expand_playlist(0, details).is_err());
    }

    #[test]
    fn unwritable_library_pauses_queued_songs_until_retry() {
        let fixture = Fixture::new("playlist_blocked");
        let mut downloader = Downloader::default();
        downloader
            .enqueue(
                "https://music.apple.com/us/playlist/test/pl.test",
                &fixture.lib(),
            )
            .unwrap();
        downloader.expand_playlist(0, test_playlist(3)).unwrap();
        let active = downloader.next_queued().unwrap();
        downloader.finish(
            active.index,
            Err(DownloadError::LibraryNotWritable("read-only".into())),
        );
        assert!(
            downloader
                .queue()
                .iter()
                .all(|item| matches!(item.status, DownloadStatus::Blocked(_)))
        );
        assert!(downloader.next_queued().is_none());
        assert_eq!(downloader.completed(), 0);
        downloader.retry_failed();
        assert_eq!(downloader.next_queued().unwrap().index, 0);
    }

    #[test]
    fn rate_limit_detection_ignores_ids_paths_titles_and_progress_numbers() {
        for line in [
            "[INFO] [Track 1/1] Downloading \"Song 429\"",
            "[INFO] [Track 1/1] Downloading \"HTTP Error 429: Too Many Requests\"",
            "[WARNING] [Track 1/1] Skipping \"Too Many Requests\": Media is not streamable: 14291234",
            "[ERROR] [Track 1/1] Error downloading \"Rate Limit Exceeded\"",
            "[INFO] Processing \"https://music.apple.com/us/song/track/14291234\"",
            "[download] 100% of 429.00MiB at 1429.00KiB/s",
            "File already exists: /Tracks/Artist/429.m4a",
            "retry status_forcelist=[429, 500, 502, 503, 504]",
            "HTTP Error 4290",
        ] {
            assert!(!summarize_log(line).rate_limited, "false positive: {line}");
        }
    }

    #[test]
    fn rate_limit_detection_accepts_status_context_and_records_sanitized_evidence() {
        for line in [
            "GamdlApiResponseError: Error fetching license exchange data (Status code: 429)",
            "HTTP Error 429: Too Many Requests",
            "HTTP/1.1 429 Too Many Requests",
            "HTTP/2 429",
            "response status_code=429",
            "[WARNING] Rate limit exceeded",
            "[ERROR] [Track 1/1] Skipping \"Song\": HTTP Error 429: Too Many Requests",
        ] {
            let summary = summarize_log(line);
            assert!(summary.rate_limited, "missed rate limit: {line}");
            assert!(summary.issues[0].starts_with("Rate-limit signal:"));
        }
        let summary =
            summarize_log("HTTP Error 429 for https://example.test/?token=private\nHTTP Error 429");
        assert_eq!(summary.issues.len(), 1);
        assert!(!summary.issues[0].contains("private"));
    }

    #[test]
    fn recovered_rate_limit_does_not_make_a_complete_download_partial() {
        let mut outcome = DownloadOutcome {
            saved: 1,
            existing: 0,
            unavailable: 0,
            other_skipped: 0,
            invalid: 0,
            rate_limited: true,
            issues: vec!["Rate-limit signal: backend reported HTTP 429".into()],
        };
        assert!(!outcome.partial());
        assert!(outcome.detail().contains("rate-limit warning"));
        assert_eq!(DownloadStatus::Finished(outcome).to_string(), "Downloaded");
        outcome = DownloadOutcome {
            saved: 0,
            existing: 1,
            unavailable: 0,
            other_skipped: 0,
            invalid: 0,
            rate_limited: true,
            issues: Vec::new(),
        };
        assert_eq!(
            DownloadStatus::Finished(outcome).to_string(),
            "Already saved"
        );
        outcome = DownloadOutcome {
            saved: 1,
            existing: 0,
            unavailable: 1,
            other_skipped: 0,
            invalid: 0,
            rate_limited: false,
            issues: Vec::new(),
        };
        assert_eq!(DownloadStatus::Finished(outcome).to_string(), "Partial");
    }
}
