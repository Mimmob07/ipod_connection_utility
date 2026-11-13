//! Append one durable record per completed file. A failed/torn final write never
//! invalidates earlier records, and no scan state is written to the iPod.

use super::{FileMetadata, IpodMetadata, at};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub(super) const FILE_NAME: &str = ".ipod-scan-cache.jsonl";

#[derive(Serialize, Deserialize, PartialEq)]
struct Header {
    version: u32,
    library: PathBuf,
    target: PathBuf,
    uuid: Option<String>,
}

#[derive(Serialize, Deserialize)]
enum Record {
    Local(FileMetadata),
    Ipod(IpodMetadata),
}

pub(super) struct Checkpoint {
    path: PathBuf,
    writer: Option<File>,
    pub local: HashMap<PathBuf, FileMetadata>,
    pub ipod: HashMap<PathBuf, IpodMetadata>,
}

impl Checkpoint {
    pub fn open(library: &Path, target: &Path, writable: bool) -> io::Result<Self> {
        let path = library.join(FILE_NAME);
        let expected = Header {
            version: 1,
            library: library.to_path_buf(),
            target: target.to_path_buf(),
            uuid: filesystem_uuid(target),
        };
        let mut cache = Self {
            path,
            writer: None,
            local: HashMap::new(),
            ipod: HashMap::new(),
        };
        let mut valid_bytes = 0;
        let mut matching_header = false;
        let file = match File::open(&cache.path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return at(Err(error), "Read scan checkpoint", &cache.path),
        };
        if let Some(file) = file {
            let mut reader = BufReader::new(file);
            let mut line = Vec::new();
            let count = at(
                reader.read_until(b'\n', &mut line),
                "Read scan checkpoint",
                &cache.path,
            )?;
            if line.ends_with(b"\n")
                && serde_json::from_slice::<Header>(&line).is_ok_and(|header| header == expected)
            {
                matching_header = true;
                valid_bytes = count as u64;
                loop {
                    line.clear();
                    let count = at(
                        reader.read_until(b'\n', &mut line),
                        "Read scan checkpoint",
                        &cache.path,
                    )?;
                    if count == 0 || !line.ends_with(b"\n") {
                        break;
                    }
                    let Ok(record) = serde_json::from_slice::<Record>(&line) else {
                        break;
                    };
                    match record {
                        Record::Local(file) => {
                            cache.local.insert(file.file_path.clone(), file);
                        }
                        Record::Ipod(file) => {
                            cache.ipod.insert(file.file_path.clone(), file);
                        }
                    }
                    valid_bytes += count as u64;
                }
            }
        }
        if writable {
            if !matching_header {
                // Replace only after the new header is complete. An interrupted reset
                // leaves either the previous journal or a valid empty checkpoint.
                let temporary = cache.path.with_extension("jsonl.tmp");
                let mut file = at(
                    File::create(&temporary),
                    "Create scan checkpoint",
                    &temporary,
                )?;
                let mut bytes = serde_json::to_vec(&expected)?;
                bytes.push(b'\n');
                at(
                    file.write_all(&bytes).and_then(|()| file.sync_all()),
                    "Save scan checkpoint",
                    &temporary,
                )?;
                at(
                    fs::rename(&temporary, &cache.path),
                    "Replace scan checkpoint",
                    &cache.path,
                )?;
            }
            let writer = at(
                OpenOptions::new().append(true).open(&cache.path),
                "Open scan checkpoint",
                &cache.path,
            )?;
            if matching_header {
                // Remove only an incomplete/invalid tail before appending new records.
                at(
                    writer.set_len(valid_bytes),
                    "Recover scan checkpoint",
                    &cache.path,
                )?;
            }
            cache.writer = Some(writer);
        }
        Ok(cache)
    }

    fn append(&mut self, record: &Record) -> io::Result<()> {
        if let Some(writer) = &mut self.writer {
            let mut bytes = serde_json::to_vec(record)?;
            bytes.push(b'\n');
            at(
                writer.write_all(&bytes).and_then(|()| writer.sync_data()),
                "Save scan checkpoint",
                &self.path,
            )?;
        }
        Ok(())
    }

    pub fn save_local(&mut self, file: &FileMetadata) -> io::Result<()> {
        if self.local.get(&file.file_path).is_some_and(|cached| {
            cached.mtime == file.mtime && cached.size == file.size && cached.hash == file.hash
        }) {
            return Ok(());
        }
        let mut file = file.clone();
        file.ipod = None;
        self.append(&Record::Local(file.clone()))?;
        self.local.insert(file.file_path.clone(), file);
        Ok(())
    }

    pub fn save_ipod(&mut self, file: &IpodMetadata) -> io::Result<()> {
        if self.ipod.get(&file.file_path).is_some_and(|cached| {
            cached.mtime == file.mtime && cached.size == file.size && cached.hash == file.hash
        }) {
            return Ok(());
        }
        self.append(&Record::Ipod(file.clone()))?;
        self.ipod.insert(file.file_path.clone(), file.clone());
        Ok(())
    }
}

fn filesystem_uuid(target: &Path) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let device = fs::metadata(target).ok()?.dev();
        // UUID survives unplug/replug even when /dev/sda2 becomes /dev/sdb2.
        for entry in fs::read_dir("/dev/disk/by-uuid").ok()?.flatten() {
            if fs::metadata(entry.path()).is_ok_and(|metadata| metadata.rdev() == device) {
                return Some(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    let _ = target;
    None
}
