use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub local_path: Option<String>,
    pub ipod_path: Option<String>,
}

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(crate) fn load() -> Settings {
    fs::read(crate::wrapper::user_data_dir().join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

impl Settings {
    pub(crate) fn interactive_paths(
        &self,
        local: Option<String>,
        ipod: Option<String>,
    ) -> (String, String) {
        let saved = |path: &Option<String>| {
            path.as_ref()
                .filter(|p| Path::new(p).is_absolute())
                .cloned()
        };
        let local = local
            .or_else(|| saved(&self.local_path))
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("Music"))
                    .filter(|path| path.is_absolute() && path.is_dir())
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        (
            local,
            ipod.or_else(|| saved(&self.ipod_path)).unwrap_or_default(),
        )
    }
}

pub(crate) fn save(local: &str, ipod: &str) -> io::Result<()> {
    let normalize = |value: &str| -> io::Result<Option<String>> {
        if value.is_empty() {
            return Ok(None);
        }
        let path = Path::new(value);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let path = fs::canonicalize(&absolute).unwrap_or(absolute);
        Ok(Some(path.to_string_lossy().into_owned()))
    };
    let settings = Settings {
        local_path: normalize(local)?,
        ipod_path: normalize(ipod)?,
    };
    let directory = crate::wrapper::user_data_dir();
    fs::create_dir_all(&directory)?;
    let (temporary, mut file) = loop {
        let temporary = directory.join(format!(
            ".settings-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match File::create_new(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(&serde_json::to_vec_pretty(&settings).map_err(io::Error::other)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, directory.join("settings.json"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
