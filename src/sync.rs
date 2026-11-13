use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::{DirEntry, WalkDir};
use xxhash_rust::xxh3::Xxh3;

mod checkpoint;
mod copy;
use checkpoint::Checkpoint;

pub const DATA_FILE: &str = "data.json";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileMetadata {
    pub file_path: PathBuf,
    pub mtime: SystemTime,
    pub size: u64,
    pub hash: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ipod: Option<IpodMetadata>,
}

/// Cache destination hashes, invalidating them when its path, size or timestamp changes.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct IpodMetadata {
    file_path: PathBuf,
    mtime: SystemTime,
    size: u64,
    hash: u64,
}

#[derive(Debug, Default)]
pub struct FilePlan {
    pub update: Vec<FileMetadata>,
    // Actual iPod paths, including songs never recorded in data.json.
    pub delete: Vec<PathBuf>,
    pub add: Vec<PathBuf>,
    library: PathBuf,
    target: PathBuf,
    destinations: HashMap<PathBuf, PathBuf>,
}

impl FilePlan {
    pub fn len(&self) -> usize {
        self.update.len() + self.delete.len() + self.add.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct Progress<'a> {
    pub done: usize,
    pub total: usize,
    pub action: &'static str,
    pub path: &'a Path,
}

pub fn data_json_path(master_path: &str) -> PathBuf {
    Path::new(master_path).join(DATA_FILE)
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry.depth() > 0 && entry.file_name().to_string_lossy().starts_with('.')
}

fn is_music(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "m4a"
                    | "mp3"
                    | "flac"
                    | "aac"
                    | "ogg"
                    | "opus"
                    | "wav"
                    | "aiff"
                    | "aif"
                    | "alac"
                    | "wma"
                    | "m4b"
                    | "mp4"
                    | "ape"
                    | "wv"
                    | "mpc"
                    | "spx"
            )
        })
}

fn library_files(master_path: &str) -> impl Iterator<Item = walkdir::Result<DirEntry>> {
    WalkDir::new(master_path)
        .into_iter()
        .filter_entry(|entry| !is_hidden(entry))
        .filter(|entry| {
            entry.as_ref().map_or(true, |entry| {
                entry.file_type().is_file() && is_music(entry.path())
            })
        })
}

fn read_metadata(path: &Path) -> io::Result<FileMetadata> {
    let mut file = at(File::open(path), "Open audio file", path)?;
    let before = at(file.metadata(), "Read audio metadata", path)?;
    let mut hasher = Xxh3::new();
    let mut buffer = [0; 64 * 1024];
    let mut bytes_read = 0;
    loop {
        let count = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => at(result, "Read audio contents", path)?,
        };
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        bytes_read += count as u64;
    }
    let metadata = at(file.metadata(), "Read audio metadata", path)?;
    let mtime = at(metadata.modified(), "Read audio timestamp", path)?;
    if before.len() != metadata.len()
        || bytes_read != metadata.len()
        || at(before.modified(), "Read audio timestamp", path)? != mtime
    {
        return Err(io::Error::other(format!(
            "Audio file changed while being read: {}. Scan again",
            path.display()
        )));
    }
    Ok(FileMetadata {
        file_path: path.to_path_buf(),
        mtime,
        size: metadata.len(),
        hash: hasher.digest(),
        ipod: None,
    })
}

fn at<T>(result: io::Result<T>, action: &str, path: &Path) -> io::Result<T> {
    result.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("{action} {}: {error}", path.display()),
        )
    })
}

/// Accept either a mounted iPod root or an explicit music folder.
fn music_folder(ipod_path: &str) -> io::Result<PathBuf> {
    let root = at(
        Path::new(ipod_path).canonicalize(),
        "Resolve iPod folder",
        Path::new(ipod_path),
    )?;
    if !at(fs::metadata(&root), "Read iPod folder", &root)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "iPod path is not a folder",
        ));
    }
    let music = root.join("Music");
    if at(music.try_exists(), "Find iPod Music folder", &music)? {
        if !at(fs::metadata(&music), "Read iPod Music folder", &music)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("iPod Music path is not a folder: {}", music.display()),
            ));
        }
        at(music.canonicalize(), "Resolve iPod Music folder", &music)
    } else if root.join(".rockbox").exists() || root.join("iPod_Control").exists() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "iPod Music folder was not found",
        ))
    } else {
        Ok(root)
    }
}

fn name_key(path: &Path) -> String {
    // The iPod uses a case-insensitive filesystem.
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
}

pub fn initialize_data(
    master_path: &str,
    on_progress: &mut dyn FnMut(Progress),
) -> io::Result<Vec<FileMetadata>> {
    let json_path = data_json_path(master_path);
    if json_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", json_path.display()),
        ));
    }
    let root = Path::new(master_path).canonicalize()?;
    let paths: Vec<_> =
        library_files(root.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Library path is not UTF-8")
        })?)
        .map(|entry| entry.map(|entry| entry.into_path()))
        .collect::<Result<_, _>>()?;
    let mut manifest = Vec::new();
    for path in &paths {
        manifest.push(read_metadata(path)?);
        on_progress(Progress {
            done: manifest.len(),
            total: paths.len(),
            action: "Hashed",
            path,
        });
    }
    let mut file = File::create_new(json_path)?;
    file.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    Ok(manifest)
}

pub fn read_data_json(path: &Path) -> io::Result<Vec<FileMetadata>> {
    let mut json = String::new();
    File::open(path)?.read_to_string(&mut json)?;
    Ok(serde_json::from_str(&json)?)
}

pub fn write_data_json(path: &Path, manifest: &[FileMetadata]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(serde_json::to_string_pretty(manifest)?.as_bytes())
}

/// Reconcile current music on both sides. The manifest is a local metadata/hash cache;
/// destination inventory, rather than manifest history, determines removals and repairs.
/// Save completed hashes locally; publish a plan/manifest only after the complete scan.
pub fn find_changes(
    master_path: &str,
    ipod_path: &str,
    manifest: &mut Vec<FileMetadata>,
    on_progress: &mut dyn FnMut(Progress),
) -> io::Result<FilePlan> {
    scan(master_path, ipod_path, manifest, true, on_progress)
}

/// Preview without writing even a checkpoint; existing checkpoints may still be reused.
pub fn preview_changes(
    master_path: &str,
    ipod_path: &str,
    manifest: &mut Vec<FileMetadata>,
    on_progress: &mut dyn FnMut(Progress),
) -> io::Result<FilePlan> {
    scan(master_path, ipod_path, manifest, false, on_progress)
}

fn scan(
    master_path: &str,
    ipod_path: &str,
    manifest: &mut Vec<FileMetadata>,
    save_progress: bool,
    on_progress: &mut dyn FnMut(Progress),
) -> io::Result<FilePlan> {
    let root = at(
        Path::new(master_path).canonicalize(),
        "Resolve local library",
        Path::new(master_path),
    )?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Local library is not a folder",
        ));
    }
    let target = music_folder(ipod_path)?;
    if root.starts_with(&target) || target.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Local and iPod music folders must be separate",
        ));
    }
    let mut previous = HashMap::new();
    for entry in manifest.iter() {
        // Older manifests used ./Playlists/... relative to the library, or local/...
        // relative to the working directory. Anchor both spellings to this library.
        let path = if entry.file_path.is_absolute() {
            entry.file_path.clone()
        } else {
            root.join(
                entry
                    .file_path
                    .strip_prefix(master_path)
                    .unwrap_or(&entry.file_path),
            )
        };
        previous.insert(path, entry);
    }
    let mut paths: Vec<_> =
        library_files(root.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Library path is not UTF-8")
        })?)
        .map(|entry| entry.map(|entry| entry.into_path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    // Manage only the flat music folder. Playlists, firmware, hidden AppleDouble files,
    // scripts and other files on the device are outside the sync scope.
    let mut remote = BTreeMap::new();
    for entry in at(fs::read_dir(&target), "List iPod Music folder", &target)? {
        let entry = at(entry, "Read iPod directory entry", &target)?;
        let path = entry.path();
        if at(entry.file_type(), "Read iPod file type", &path)?.is_file()
            && !entry.file_name().to_string_lossy().starts_with('.')
            && is_music(&path)
            && remote.insert(name_key(&path), path).is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "iPod has multiple music filenames differing only in case",
            ));
        }
    }
    let mut checkpoint = Checkpoint::open(&root, &target, save_progress)?;
    let total = paths.len() + remote.len();
    let mut current = Vec::new();
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for path in paths {
        let metadata = at(fs::metadata(&path), "Read local metadata", &path)?;
        let old = previous.get(&path);
        let mtime = at(metadata.modified(), "Read local timestamp", &path)?;
        let cached = checkpoint
            .local
            .get(&path)
            .filter(|file| file.mtime == mtime && file.size == metadata.len())
            .or_else(|| {
                old.copied()
                    .filter(|file| file.mtime == mtime && file.size == metadata.len())
            });
        let file = if let Some(cached) = cached {
            let mut file = cached.clone();
            file.file_path = path.clone();
            file.ipod = old.and_then(|old| old.ipod.clone());
            file
        } else {
            let mut file = read_metadata(&path)?;
            file.ipod = old.and_then(|old| old.ipod.clone());
            file
        };
        checkpoint.save_local(&file)?;
        groups
            .entry(name_key(&path))
            .or_default()
            .push(current.len());
        current.push(file);
        on_progress(Progress {
            done: current.len(),
            total,
            action: "Scanned local",
            path: &path,
        });
    }
    let mut plan = FilePlan {
        library: root,
        target,
        ..FilePlan::default()
    };
    for (done, (key, remote_path)) in (current.len() + 1..).zip(&remote) {
        on_progress(Progress {
            done: done - 1,
            total,
            action: "Checking iPod",
            path: remote_path,
        });
        let mut action = "Scanned iPod";
        if let Some(indexes) = groups.get(key) {
            let metadata = at(fs::metadata(remote_path), "Read iPod metadata", remote_path)?;
            let mtime = at(metadata.modified(), "Read iPod timestamp", remote_path)?;
            let cached = checkpoint.ipod.get(remote_path).filter(|cached| {
                cached.file_path == *remote_path
                    && cached.size == metadata.len()
                    && cached.mtime == mtime
            });
            let hash = if let Some(cached) = cached {
                action = "Reused iPod hash";
                cached.hash
            } else {
                action = "Hashed iPod";
                read_metadata(remote_path)?.hash
            };
            let state = IpodMetadata {
                file_path: remote_path.clone(),
                mtime,
                size: metadata.len(),
                hash,
            };
            checkpoint.save_ipod(&state)?;
            // Existing duplicate basenames share one iPod song. Preserve it if it matches
            // any local copy; otherwise choose the first sorted local path consistently.
            let matches = indexes
                .iter()
                .any(|idx| current[*idx].hash == hash && current[*idx].size == metadata.len());
            for idx in indexes {
                current[*idx].ipod = Some(state.clone());
            }
            if !matches {
                let file = current[indexes[0]].clone();
                plan.destinations
                    .insert(file.file_path.clone(), remote_path.clone());
                plan.update.push(file);
            }
        } else {
            plan.delete.push(remote_path.clone());
        }
        on_progress(Progress {
            done,
            total,
            action,
            path: remote_path,
        });
    }
    for (key, indexes) in groups {
        if !remote.contains_key(&key) {
            let path = current[indexes[0]].file_path.clone();
            plan.destinations
                .insert(path.clone(), plan.target.join(path.file_name().unwrap()));
            plan.add.push(path);
        }
    }
    *manifest = current;
    Ok(plan)
}

/// Apply only to the folder scanned. A subsequent scan always inspects the device again,
/// so partial failures are recoverable even for orphan songs absent from the manifest.
pub fn apply_plan(
    manifest: &mut [FileMetadata],
    plan: FilePlan,
    ipod_path: &str,
    on_progress: &mut dyn FnMut(Progress),
) -> io::Result<()> {
    if music_folder(ipod_path)? != plan.target {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "iPod path changed; scan again",
        ));
    }
    let total = plan.len();
    let mut checkpoint = Checkpoint::open(&plan.library, &plan.target, true)?;
    let mut done = 0;
    let mut report = |action: &'static str, path: &Path| {
        done += 1;
        on_progress(Progress {
            done,
            total,
            action,
            path,
        });
    };
    for (action, path) in plan
        .update
        .iter()
        .map(|file| ("Updated", &file.file_path))
        .chain(plan.add.iter().map(|path| ("Added", path)))
    {
        let destination = &plan.destinations[path];
        let local = read_metadata(path)?;
        at(
            copy::publish(path, destination),
            "Publish audio copy",
            destination,
        )?;
        let metadata = fs::metadata(destination)?;
        let state = IpodMetadata {
            file_path: destination.clone(),
            mtime: metadata.modified()?,
            size: metadata.len(),
            hash: local.hash,
        };
        for file in manifest
            .iter_mut()
            .filter(|file| name_key(&file.file_path) == name_key(path))
        {
            file.ipod = Some(state.clone());
            if file.file_path == *path {
                file.mtime = local.mtime;
                file.size = local.size;
                file.hash = local.hash;
            }
        }
        checkpoint.save_local(&local)?;
        checkpoint.save_ipod(&state)?;
        report(action, path);
    }
    for path in plan.delete {
        match fs::remove_file(&path) {
            Ok(()) => {}
            // Already removed since the scan: the desired state has been reached.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        report("Deleted", &path);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A scratch library and iPod folder that are removed again on drop
    pub(crate) struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        pub(crate) fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("ipod_util_{name}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("lib/album")).unwrap();
            fs::create_dir_all(root.join("ipod")).unwrap();
            // Scans report canonical paths; macOS temp_dir() is under the /var symlink.
            let root = root.canonicalize().unwrap();

            let fixture = Fixture { root };
            for name in ["a.mp3", "b.mp3", "c.mp3", "d.mp3", "album/e.mp3"] {
                fixture.write(name, name);
                fs::copy(
                    fixture.root.join("lib").join(name),
                    fixture.ipod_file(name.rsplit('/').next().unwrap()),
                )
                .unwrap();
            }
            fixture
        }

        pub(crate) fn lib(&self) -> String {
            self.root.join("lib").to_str().unwrap().to_string()
        }

        pub(crate) fn ipod(&self) -> String {
            self.root.join("ipod").to_str().unwrap().to_string()
        }

        pub(crate) fn ipod_file(&self, name: &str) -> PathBuf {
            self.root.join("ipod").join(name)
        }

        pub(crate) fn write(&self, name: &str, contents: &str) {
            fs::write(self.root.join("lib").join(name), contents).unwrap();
        }

        pub(crate) fn remove(&self, name: &str) {
            fs::remove_file(self.root.join("lib").join(name)).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn names(manifest: &[FileMetadata]) -> Vec<String> {
        let mut names: Vec<String> = manifest
            .iter()
            .map(|x| {
                x.file_path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn initialize_tracks_every_file_and_reports_progress() {
        let fixture = Fixture::new("init");
        fixture.write(".hidden", "skip me");
        let mut reports = Vec::new();

        let manifest =
            initialize_data(&fixture.lib(), &mut |p| reports.push((p.done, p.total))).unwrap();

        assert_eq!(
            names(&manifest),
            ["a.mp3", "b.mp3", "c.mp3", "d.mp3", "e.mp3"]
        );
        assert_eq!(reports.last(), Some(&(5, 5)));
        let saved = read_data_json(&data_json_path(&fixture.lib())).unwrap();
        assert_eq!(names(&saved), names(&manifest));

        let again = initialize_data(&fixture.lib(), &mut |_| {});
        assert_eq!(again.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn relative_dot_path_is_walked() {
        let fixture = Fixture::new("relative_dot");
        assert_eq!(library_files(&format!("{}/.", fixture.lib())).count(), 5);
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_update_preserves_old_audio() {
        const CHILD: &str = "IPOD_TEST_INTERRUPTED_UPDATE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sync::tests::interrupted_update_preserves_old_audio",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "copy-failure child failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let fixture = Fixture::new("interrupted_update");
        fixture.write("a.mp3", &"new audio".repeat(32_768));
        let mut manifest = Vec::new();
        let plan = scan(&fixture, &mut manifest);
        assert_eq!(plan.update.len(), 1);
        let before = serde_json::to_string(&manifest).unwrap();
        // Only this disposable child gets a file-size limit. The real copy writes
        // a prefix, then fails with EFBIG, without timing races or mocked I/O.
        unsafe {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            let limit = libc::rlimit {
                rlim_cur: 16_384,
                rlim_max: 16_384,
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
        }
        let error = apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap_err();
        assert!(error.to_string().contains("File too large"), "{error}");
        assert_eq!(fs::read(fixture.ipod_file("a.mp3")).unwrap(), b"a.mp3");
        assert_eq!(serde_json::to_string(&manifest).unwrap(), before);
        assert_eq!(fs::read_dir(fixture.ipod()).unwrap().count(), 5);
    }

    #[test]
    fn abandoned_sync_stages_are_excluded_from_both_inventories() {
        let fixture = Fixture::new("sync_stage_inventory");
        for name in [".ipod-sync-123-0.tmp", ".ipod-sync-123-1.mp3"] {
            fixture.write(name, "incomplete local copy");
            fs::write(fixture.ipod_file(name), "incomplete device copy").unwrap();
        }
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
        assert_eq!(manifest.len(), 5);
        assert!(scan(&fixture, &mut manifest).is_empty());
        assert_eq!(manifest.len(), 5);
        assert!(fixture.ipod_file(".ipod-sync-123-0.tmp").exists());
    }

    #[test]
    fn failed_publication_cleans_stage_without_removing_destination() {
        let fixture = Fixture::new("sync_rename_failure");
        let destination = fixture.ipod_file("occupied.mp3");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), "untouched").unwrap();
        let source = Path::new(&fixture.lib()).join("a.mp3");
        assert!(copy::publish(&source, &destination).is_err());
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"untouched");
        assert_eq!(fs::read_dir(fixture.ipod()).unwrap().count(), 6);
    }

    #[cfg(unix)]
    #[test]
    fn successful_update_atomically_replaces_instead_of_mutating_old_file() {
        let fixture = Fixture::new("sync_atomic_success");
        let destination = fixture.ipod_file("a.mp3");
        let mut old_reader = File::open(&destination).unwrap();
        let bytes: Vec<_> = (0..200_123).map(|index| (index % 251) as u8).collect();
        fs::write(Path::new(&fixture.lib()).join("a.mp3"), &bytes).unwrap();
        let mut manifest = Vec::new();
        let plan = scan(&fixture, &mut manifest);
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        let mut old_bytes = Vec::new();
        old_reader.read_to_end(&mut old_bytes).unwrap();
        assert_eq!(old_bytes, b"a.mp3");
        assert_eq!(fs::read_dir(fixture.ipod()).unwrap().count(), 5);
        assert!(scan(&fixture, &mut manifest).is_empty());
    }

    #[test]
    fn sync_applies_adds_updates_and_deletes() {
        let fixture = Fixture::new("sync");
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();

        fixture.remove("a.mp3");
        fixture.remove("c.mp3");
        fixture.write("d.mp3", "a longer, changed d");
        fixture.write("new.mp3", "new");

        let plan =
            find_changes(&fixture.lib(), &fixture.ipod(), &mut manifest, &mut |_| {}).unwrap();
        assert_eq!(
            (plan.add.len(), plan.update.len(), plan.delete.len()),
            (1, 1, 2)
        );

        let mut actions = Vec::new();
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |p| {
            actions.push(p.action)
        })
        .unwrap();

        assert_eq!(actions, ["Updated", "Added", "Deleted", "Deleted"]);
        assert_eq!(names(&manifest), ["b.mp3", "d.mp3", "e.mp3", "new.mp3"]);
        assert!(!fixture.ipod_file("a.mp3").exists());
        assert!(!fixture.ipod_file("c.mp3").exists());
        assert!(fixture.ipod_file("new.mp3").exists());
        assert_eq!(
            fs::read_to_string(fixture.ipod_file("d.mp3")).unwrap(),
            "a longer, changed d"
        );

        // Nothing left to do once the plan was applied
        assert!(
            find_changes(&fixture.lib(), &fixture.ipod(), &mut manifest, &mut |_| {})
                .unwrap()
                .is_empty()
        );
    }

    fn scan(fixture: &Fixture, manifest: &mut Vec<FileMetadata>) -> FilePlan {
        find_changes(&fixture.lib(), &fixture.ipod(), manifest, &mut |_| {}).unwrap()
    }

    #[test]
    fn removes_orphans_absent_from_manifest_and_restores_missing_or_modified_copies() {
        let fixture = Fixture::new("reconcile");
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
        fs::write(fixture.ipod_file("old.m4a"), "never in manifest").unwrap();
        fs::remove_file(fixture.ipod_file("a.mp3")).unwrap();
        // Same size, different content: destination hashing must detect this too.
        fs::write(fixture.ipod_file("b.mp3"), "xxxxx").unwrap();
        let plan = scan(&fixture, &mut manifest);
        assert_eq!(
            (plan.add.len(), plan.update.len(), plan.delete.len()),
            (1, 1, 1)
        );
        assert_eq!(plan.delete, [fixture.ipod_file("old.m4a")]);
        assert!(fixture.ipod_file("old.m4a").exists());
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap();
        assert_eq!(
            fs::read_to_string(fixture.ipod_file("b.mp3")).unwrap(),
            "b.mp3"
        );
        assert!(scan(&fixture, &mut manifest).is_empty());

        // Cached hashes must be invalidated after subsequent device changes.
        fs::write(fixture.ipod_file("b.mp3"), "changed again").unwrap();
        assert_eq!(scan(&fixture, &mut manifest).update.len(), 1);
    }

    #[test]
    fn mount_root_uses_only_music_and_preserves_device_files() {
        let fixture = Fixture::new("mount_root");
        let music = fixture.ipod_file("Music");
        fs::create_dir(&music).unwrap();
        for entry in fs::read_dir(fixture.ipod()).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                fs::rename(entry.path(), music.join(entry.file_name())).unwrap();
            }
        }
        fs::create_dir(fixture.ipod_file(".rockbox")).unwrap();
        fs::write(fixture.ipod_file(".rockbox/firmware.mp3"), "firmware").unwrap();
        fs::write(fixture.ipod_file("outside.mp3"), "outside").unwrap();
        fs::write(music.join("._old.mp3"), "AppleDouble").unwrap();
        fs::write(music.join("notes.txt"), "notes").unwrap();
        fs::write(music.join("orphan.mp3"), "orphan").unwrap();
        fixture.write("artwork_helper.py", "script");
        fixture.write("ipod_connection_utility", "binary");
        let mut manifest = Vec::new();
        let plan = scan(&fixture, &mut manifest);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan.delete, [music.join("orphan.mp3")]);
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap();
        fixture.write("new.mp3", "new song");
        let plan = scan(&fixture, &mut manifest);
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap();
        assert!(music.join("new.mp3").exists());
        assert!(!fixture.ipod_file("new.mp3").exists());
        for path in [
            ".rockbox/firmware.mp3",
            "outside.mp3",
            "Music/._old.mp3",
            "Music/notes.txt",
        ] {
            assert!(fixture.ipod_file(path).exists());
        }
        assert!(scan(&fixture, &mut manifest).is_empty());
    }

    #[test]
    fn duplicate_basenames_preserve_any_matching_local_copy() {
        let fixture = Fixture::new("duplicates");
        fixture.write("album/a.mp3", "different recording");
        let mut manifest = Vec::new();
        assert!(scan(&fixture, &mut manifest).is_empty());
        fixture.remove("a.mp3");
        let plan = scan(&fixture, &mut manifest);
        assert_eq!(plan.update.len(), 1);
        assert!(plan.delete.is_empty());
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap();
        assert_eq!(
            fs::read_to_string(fixture.ipod_file("a.mp3")).unwrap(),
            "different recording"
        );
        assert!(scan(&fixture, &mut manifest).is_empty());
    }

    #[test]
    fn missing_on_both_sides_is_cleaned_without_a_failing_delete() {
        let fixture = Fixture::new("missing_both");
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
        fixture.remove("a.mp3");
        fs::remove_file(fixture.ipod_file("a.mp3")).unwrap();
        assert!(scan(&fixture, &mut manifest).is_empty());
        assert_eq!(manifest.len(), 4);
    }

    #[test]
    fn legacy_relative_manifest_paths_are_anchored_to_library() {
        let fixture = Fixture::new("relative_manifest");
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
        for file in &mut manifest {
            file.file_path =
                Path::new(".").join(file.file_path.strip_prefix(fixture.lib()).unwrap());
        }
        assert!(scan(&fixture, &mut manifest).is_empty());
        assert!(manifest.iter().all(|file| file.file_path.is_absolute()));
    }

    #[test]
    fn failed_scan_does_not_replace_the_manifest() {
        let fixture = Fixture::new("failed_scan");
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
        let before = serde_json::to_string(&manifest).unwrap();
        fixture.remove("a.mp3");
        assert!(
            find_changes(
                &fixture.lib(),
                &format!("{}/missing", fixture.ipod()),
                &mut manifest,
                &mut |_| {}
            )
            .is_err()
        );
        assert_eq!(serde_json::to_string(&manifest).unwrap(), before);
        // Walk failures must propagate, rather than turn unreadable music into deletions.
        assert!(
            find_changes(
                &format!("{}/missing", fixture.lib()),
                &fixture.ipod(),
                &mut manifest,
                &mut |_| {}
            )
            .is_err()
        );
        assert_eq!(serde_json::to_string(&manifest).unwrap(), before);
    }

    #[test]
    fn case_insensitive_matching_updates_the_existing_spelling() {
        let fixture = Fixture::new("case_insensitive");
        fs::rename(fixture.ipod_file("a.mp3"), fixture.ipod_file("A.MP3")).unwrap();
        let mut manifest = Vec::new();
        assert!(scan(&fixture, &mut manifest).is_empty());
        fixture.write("a.mp3", "changed song");
        let plan = scan(&fixture, &mut manifest);
        assert_eq!(plan.update.len(), 1);
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap();
        assert_eq!(
            fs::read_to_string(fixture.ipod_file("A.MP3")).unwrap(),
            "changed song"
        );
        // exists() would also match "A.MP3" on case-insensitive filesystems (macOS).
        let names: Vec<_> = fs::read_dir(fixture.ipod())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(names.contains(&"A.MP3".into()) && !names.contains(&"a.mp3".into()));
    }

    #[test]
    fn partial_sync_failure_is_recoverable_from_device_inventory() {
        let fixture = Fixture::new("partial_failure");
        fs::write(fixture.ipod_file("old.mp3"), "orphan").unwrap();
        fixture.write("new.mp3", "new song");
        let mut manifest = Vec::new();
        let plan = scan(&fixture, &mut manifest);
        fixture.remove("new.mp3");
        assert!(apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).is_err());
        let plan = scan(&fixture, &mut manifest);
        assert_eq!(plan.delete.len(), 1);
        apply_plan(&mut manifest, plan, &fixture.ipod(), &mut |_| {}).unwrap();
        assert!(scan(&fixture, &mut manifest).is_empty());
    }

    #[test]
    fn changed_target_requires_a_new_scan() {
        let fixture = Fixture::new("changed_target");
        fs::write(fixture.ipod_file("old.mp3"), "orphan").unwrap();
        let mut manifest = Vec::new();
        let plan = scan(&fixture, &mut manifest);
        let other = fixture.ipod_file("other");
        fs::create_dir(&other).unwrap();
        let error =
            apply_plan(&mut manifest, plan, other.to_str().unwrap(), &mut |_| {}).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(fixture.ipod_file("old.mp3").exists());
    }

    fn interrupt_after_first_ipod_hash(
        fixture: &Fixture,
        manifest: &mut Vec<FileMetadata>,
    ) -> io::Error {
        let error = find_changes(&fixture.lib(), &fixture.ipod(), manifest, &mut |progress| {
            if progress.action == "Checking iPod" && progress.path == fixture.ipod_file("b.mp3") {
                // Directory inventory succeeded, then device access to this file fails.
                fs::remove_file(progress.path).unwrap();
            }
        })
        .unwrap_err();
        assert!(error.to_string().contains("Read iPod metadata"));
        assert!(error.to_string().contains("b.mp3"));
        error
    }

    #[test]
    fn failed_scan_checkpoints_completed_files_and_resumes_after_restart() {
        let fixture = Fixture::new("checkpoint_resume");
        let json_path = data_json_path(&fixture.lib());
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
        let before = serde_json::to_string(&manifest).unwrap();
        interrupt_after_first_ipod_hash(&fixture, &mut manifest);
        // An incomplete scan cannot commit a manifest or a partial deletion plan.
        assert_eq!(serde_json::to_string(&manifest).unwrap(), before);
        let cache =
            Checkpoint::open(Path::new(&fixture.lib()), Path::new(&fixture.ipod()), false).unwrap();
        assert!(cache.ipod.contains_key(&fixture.ipod_file("a.mp3")));
        assert!(!cache.ipod.contains_key(&fixture.ipod_file("b.mp3")));
        drop(cache);
        fs::write(fixture.ipod_file("b.mp3"), "b.mp3").unwrap();
        // Simulate a fresh app process with only data.json and the disk checkpoint.
        let mut restarted = read_data_json(&json_path).unwrap();
        let mut actions = Vec::new();
        let plan = find_changes(&fixture.lib(), &fixture.ipod(), &mut restarted, &mut |p| {
            actions.push((p.action, p.path.to_path_buf()));
        })
        .unwrap();
        assert!(plan.is_empty());
        assert!(actions.contains(&("Reused iPod hash", fixture.ipod_file("a.mp3"))));
        assert!(!actions.contains(&("Hashed iPod", fixture.ipod_file("a.mp3"))));
        assert!(actions.contains(&("Hashed iPod", fixture.ipod_file("b.mp3"))));
    }

    #[test]
    fn torn_checkpoint_tail_preserves_prior_records_and_recovers_for_future_scans() {
        let fixture = Fixture::new("checkpoint_torn");
        let mut manifest = initialize_data(&fixture.lib(), &mut |_| {}).unwrap();
        interrupt_after_first_ipod_hash(&fixture, &mut manifest);
        let cache_path = Path::new(&fixture.lib()).join(checkpoint::FILE_NAME);
        let mut cache = fs::OpenOptions::new()
            .append(true)
            .open(&cache_path)
            .unwrap();
        cache.write_all(b"{\"Ipod\":").unwrap();
        drop(cache);
        fs::write(fixture.ipod_file("b.mp3"), "b.mp3").unwrap();
        let mut actions = Vec::new();
        let plan = find_changes(&fixture.lib(), &fixture.ipod(), &mut manifest, &mut |p| {
            actions.push((p.action, p.path.to_path_buf()));
        })
        .unwrap();
        assert!(plan.is_empty());
        assert!(actions.contains(&("Reused iPod hash", fixture.ipod_file("a.mp3"))));
        for line in fs::read_to_string(&cache_path).unwrap().lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
        // The newly appended hashes must themselves survive another restart.
        let mut restarted = Vec::new();
        let mut hashes = 0;
        assert!(
            find_changes(&fixture.lib(), &fixture.ipod(), &mut restarted, &mut |p| {
                hashes += usize::from(p.action == "Hashed iPod");
            })
            .unwrap()
            .is_empty()
        );
        assert_eq!(hashes, 0);
    }

    #[test]
    fn resume_rechecks_inventory_and_invalidates_changed_files() {
        let fixture = Fixture::new("checkpoint_invalidate");
        let mut manifest = Vec::new();
        assert!(scan(&fixture, &mut manifest).is_empty());
        fs::write(fixture.ipod_file("a.mp3"), "xxxxx").unwrap();
        // Force a distinct mtime even on filesystems with coarse timestamps.
        let file = File::options()
            .write(true)
            .open(fixture.ipod_file("a.mp3"))
            .unwrap();
        file.set_times(
            fs::FileTimes::new()
                .set_modified(SystemTime::now() + std::time::Duration::from_secs(4)),
        )
        .unwrap();
        fs::remove_file(fixture.ipod_file("b.mp3")).unwrap();
        fixture.remove("c.mp3");
        let mut restarted = Vec::new();
        let mut actions = Vec::new();
        let plan = find_changes(&fixture.lib(), &fixture.ipod(), &mut restarted, &mut |p| {
            actions.push((p.action, p.path.to_path_buf()));
        })
        .unwrap();
        assert_eq!(
            (plan.add.len(), plan.update.len(), plan.delete.len()),
            (1, 1, 1)
        );
        assert!(actions.contains(&("Hashed iPod", fixture.ipod_file("a.mp3"))));
        assert_eq!(plan.delete, [fixture.ipod_file("c.mp3")]);
    }

    #[test]
    fn dry_run_neither_creates_nor_repairs_checkpoints() {
        let fixture = Fixture::new("checkpoint_dry_run");
        let mut manifest = Vec::new();
        let cache_path = Path::new(&fixture.lib()).join(checkpoint::FILE_NAME);
        assert!(
            preview_changes(&fixture.lib(), &fixture.ipod(), &mut manifest, &mut |_| {})
                .unwrap()
                .is_empty()
        );
        assert!(!cache_path.exists());
        assert!(scan(&fixture, &mut manifest).is_empty());
        let mut cache = fs::OpenOptions::new()
            .append(true)
            .open(&cache_path)
            .unwrap();
        cache.write_all(b"partial record").unwrap();
        drop(cache);
        let before = fs::read(&cache_path).unwrap();
        let mut restarted = Vec::new();
        let mut hashes = 0;
        assert!(
            preview_changes(&fixture.lib(), &fixture.ipod(), &mut restarted, &mut |p| {
                hashes += usize::from(p.action == "Hashed iPod");
            })
            .unwrap()
            .is_empty()
        );
        assert_eq!(hashes, 0);
        assert_eq!(fs::read(&cache_path).unwrap(), before);
    }

    #[test]
    fn checkpoint_with_different_device_identity_is_not_reused() {
        let fixture = Fixture::new("checkpoint_identity");
        let mut manifest = Vec::new();
        assert!(scan(&fixture, &mut manifest).is_empty());
        let cache_path = Path::new(&fixture.lib()).join(checkpoint::FILE_NAME);
        let text = fs::read_to_string(&cache_path).unwrap();
        let (_, records) = text.split_once('\n').unwrap();
        let mut header: serde_json::Value =
            serde_json::from_str(text.lines().next().unwrap()).unwrap();
        header["uuid"] = "another-device".into();
        fs::write(&cache_path, format!("{header}\n{records}")).unwrap();
        // Older data.json fingerprints must not bypass the device identity check.
        let mut restarted = manifest.clone();
        let mut hashes = 0;
        assert!(
            find_changes(&fixture.lib(), &fixture.ipod(), &mut restarted, &mut |p| {
                hashes += usize::from(p.action == "Hashed iPod");
            })
            .unwrap()
            .is_empty()
        );
        assert_eq!(hashes, 5);
    }

    #[test]
    fn streaming_hash_matches_legacy_manifest_hashes_across_buffer_boundaries() {
        let fixture = Fixture::new("streaming_hash");
        let bytes: Vec<_> = (0..200_123).map(|index| (index % 251) as u8).collect();
        let path = fixture.ipod_file("long.m4a");
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            read_metadata(&path).unwrap().hash,
            xxhash_rust::xxh3::xxh3_64(&bytes)
        );
    }
}
