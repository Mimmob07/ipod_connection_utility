use super::*;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct PlaylistTrack {
    pub id: String,
    pub source: String,
    pub title: String,
    pub artist: String,
    pub unavailable: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlaylistDetails {
    pub id: String,
    pub title: String,
    pub folder: PathBuf,
    pub tracks: Vec<PlaylistTrack>,
}

#[derive(Clone)]
pub struct PlaylistContext {
    pub id: String,
    pub title: String,
    pub folder: PathBuf,
}

pub fn resolve_playlist(
    id: &str,
    cancelled: &AtomicBool,
) -> Result<PlaylistDetails, DownloadError> {
    resolve_with_backend(id, DownloadBackend::Web, cancelled)
}

pub fn resolve_playlist_wrapper(
    id: &str,
    cancelled: &AtomicBool,
) -> Result<PlaylistDetails, DownloadError> {
    resolve_with_backend(id, DownloadBackend::Wrapper, cancelled)
}

fn resolve_with_backend(
    id: &str,
    backend: DownloadBackend,
    cancelled: &AtomicBool,
) -> Result<PlaylistDetails, DownloadError> {
    match backend {
        DownloadBackend::Wrapper => check_session()?,
        DownloadBackend::Web if !crate::web_auth::has_session() => {
            return Err(DownloadError::SignInRequired);
        }
        DownloadBackend::Web => {}
    }
    let serial = STAGING_ID.fetch_add(1, Ordering::Relaxed);
    let temporary =
        std::env::temp_dir().join(format!("ipod-playlist-{}-{serial}", std::process::id()));
    create_private_dir(&temporary).map_err(|error| {
        DownloadError::Backend(format!("cannot stage playlist metadata: {error}"))
    })?;
    let temporary = Staging(temporary);
    // A file avoids filling a stdout pipe for large playlists while waiting on the child.
    let output_path = temporary.0.join("playlist.json");
    let output = fs::File::create(&output_path).map_err(|error| {
        DownloadError::Backend(format!("cannot capture playlist metadata: {error}"))
    })?;
    let mut command = Command::new(python_executable());
    command
        .arg("-c")
        .arg(include_str!("../../tools/playlist_tracks.py"))
        .arg(id);
    match backend {
        DownloadBackend::Web => {
            command
                .arg("--cookies-path")
                .arg(crate::web_auth::cookies_path());
        }
        DownloadBackend::Wrapper => {
            command
                .arg("--wrapper-url")
                .arg(format!("http://127.0.0.1:{}", wrapper_port()))
                .arg("--decrypt-port")
                .arg(decrypt_port().to_string());
        }
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| DownloadError::Backend(format!("cannot read playlist: {error}")))?;
    let status = wait_child(&mut child, cancelled, Duration::from_secs(5 * 60))?;
    if !status.success() {
        return Err(DownloadError::Backend(
            "playlist lookup failed; check gamdl and Python setup".into(),
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(&output_path)
        .and_then(|file| file.take(16 * 1024 * 1024).read_to_end(&mut bytes))
        .map_err(|error| {
            DownloadError::Backend(format!("cannot read playlist metadata: {error}"))
        })?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| DownloadError::Backend("invalid or oversized playlist response".into()))?;
    if value.get("error").is_some() {
        return Err(match value["status"].as_u64() {
            Some(401 | 403) => DownloadError::SignInRequired,
            Some(429) => {
                DownloadError::RateLimited("Playlist metadata API returned HTTP 429".into())
            }
            Some(404) => DownloadError::Unavailable,
            _ => DownloadError::Backend(
                "playlist lookup failed; check availability and sign-in".into(),
            ),
        });
    }
    let details: PlaylistDetails = serde_json::from_value(value)
        .map_err(|_| DownloadError::Backend("invalid playlist metadata".into()))?;
    validate_details(id, &details)?;
    Ok(details)
}

pub(super) fn validate_details(id: &str, details: &PlaylistDetails) -> Result<(), DownloadError> {
    if details.id != id || details.tracks.is_empty() {
        return Err(DownloadError::Backend(
            "playlist has no accessible songs".into(),
        ));
    }
    if !details.folder.starts_with("Playlists")
        || details.folder.components().count() != 2
        || details
            .folder
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        || details.tracks.iter().any(|track| {
            track.unavailable.is_none()
                && parse_source(&track.source) != Some((SourceKind::Song, track.id.clone()))
        })
    {
        return Err(DownloadError::Backend("unsafe playlist metadata".into()));
    }
    Ok(())
}
