//! Runs the slow library operations on a background thread so the interface keeps drawing.

use crate::downloader::{
    self, DownloadError, DownloadOutcome, DownloadProgress, DownloadTask, LoginResult,
    PlaylistDetails, SourceKind,
};
use crate::sync::{self, FileMetadata, FilePlan, Progress};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

enum JobEvent {
    DownloadProgress(DownloadProgress),
    Progress {
        done: usize,
        total: usize,
        message: String,
    },
    Finished(JobResult),
}

pub enum JobResult {
    Initialized(io::Result<Vec<FileMetadata>>),
    Scanned {
        manifest: Vec<FileMetadata>,
        plan: io::Result<FilePlan>,
    },
    Synced {
        manifest: Vec<FileMetadata>,
        result: io::Result<()>,
    },
    Downloaded {
        index: usize,
        result: Result<DownloadOutcome, DownloadError>,
    },
    PlaylistResolved {
        index: usize,
        result: Result<PlaylistDetails, DownloadError>,
    },
    LoggedIn(Result<LoginResult, DownloadError>),
    WebLoggedIn(Result<LoginResult, DownloadError>),
}

pub enum JobUpdate {
    Running,
    Finished(JobResult),
    // The worker thread went away without reporting a result
    Crashed,
}

pub struct Job {
    pub title: &'static str,
    pub done: usize,
    pub total: usize,
    pub message: String,
    pub download_progress: Option<DownloadProgress>,
    events: Receiver<JobEvent>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Job {
    #[cfg(test)]
    pub(super) fn test_finished(result: JobResult) -> Job {
        let (sender, events) = mpsc::channel();
        sender.send(JobEvent::Finished(result)).unwrap();
        Job {
            title: "Test job",
            done: 0,
            total: 0,
            message: String::new(),
            download_progress: None,
            events,
            cancel: None,
        }
    }
    fn spawn(
        title: &'static str,
        work: impl FnOnce(&mut dyn FnMut(Progress)) -> JobResult + Send + 'static,
    ) -> Job {
        let (sender, events): (Sender<JobEvent>, Receiver<JobEvent>) = mpsc::channel();

        thread::spawn(move || {
            let progress_sender = sender.clone();
            let mut on_progress = move |progress: Progress| {
                // A failed send means the interface is gone, the work still has to finish
                let _ = progress_sender.send(JobEvent::Progress {
                    done: progress.done,
                    total: progress.total,
                    message: format!("{} {}", progress.action, progress.path.display()),
                });
            };
            let result = work(&mut on_progress);
            let _ = sender.send(JobEvent::Finished(result));
        });

        Job {
            title,
            done: 0,
            total: 0,
            message: String::new(),
            download_progress: None,
            events,
            cancel: None,
        }
    }

    pub fn initialize(local_path: String) -> Job {
        Job::spawn("Initializing database", move |on_progress| {
            JobResult::Initialized(sync::initialize_data(&local_path, on_progress))
        })
    }

    pub fn scan(local_path: String, ipod_path: String, mut manifest: Vec<FileMetadata>) -> Job {
        Job::spawn("Scanning local and iPod", move |on_progress| {
            let plan = sync::find_changes(&local_path, &ipod_path, &mut manifest, on_progress);
            JobResult::Scanned { manifest, plan }
        })
    }

    pub fn sync(
        local_path: String,
        ipod_path: String,
        mut manifest: Vec<FileMetadata>,
        plan: FilePlan,
    ) -> Job {
        Job::spawn("Syncing to iPod", move |on_progress| {
            let applied = sync::apply_plan(&mut manifest, plan, &ipod_path, on_progress);
            // Save the cache even after a failure; the next scan rechecks actual device files.
            let saved = sync::write_data_json(&sync::data_json_path(&local_path), &manifest);

            JobResult::Synced {
                manifest,
                result: applied.and(saved),
            }
        })
    }

    pub fn download(task: DownloadTask) -> Job {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let title = if task.kind == SourceKind::Playlist {
            "Loading playlist songs"
        } else {
            "Downloading Apple Music"
        };
        let (sender, events) = mpsc::channel();
        thread::spawn(move || {
            let progress_sender = sender.clone();
            let mut on_progress = move |progress| {
                let _ = progress_sender.send(JobEvent::DownloadProgress(progress));
            };
            let result = if task.kind == SourceKind::Playlist {
                JobResult::PlaylistResolved {
                    index: task.index,
                    result: match task.options.backend() {
                        downloader::DownloadBackend::Web => {
                            downloader::resolve_playlist(&task.id, &worker_cancel)
                        }
                        downloader::DownloadBackend::Wrapper => {
                            downloader::resolve_playlist_wrapper(&task.id, &worker_cancel)
                        }
                    },
                }
            } else {
                JobResult::Downloaded {
                    index: task.index,
                    result: downloader::download(&task, &worker_cancel, &mut on_progress),
                }
            };
            let _ = sender.send(JobEvent::Finished(result));
        });
        Job {
            title,
            done: 0,
            total: 0,
            message: String::new(),
            download_progress: None,
            events,
            cancel: Some(cancel),
        }
    }

    pub fn web_login(source: String) -> Job {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, events) = mpsc::channel();
        thread::spawn(move || {
            // import_cookies errors carry only allowlisted, user-facing text.
            let result = crate::web_auth::import_cookies(&source)
                .map_err(|error| DownloadError::Backend(error.to_string()))
                .and_then(|()| downloader::check_web_session(&worker_cancel));
            let _ = sender.send(JobEvent::Finished(JobResult::WebLoggedIn(result)));
        });
        Job {
            title: "Checking web sign-in",
            done: 0,
            total: 0,
            message: String::new(),
            download_progress: None,
            events,
            cancel: Some(cancel),
        }
    }

    pub fn login(username: String, password: String) -> Job {
        Job::spawn("Signing in", move |_| {
            JobResult::LoggedIn(downloader::login(username, password))
        })
    }

    pub fn login_code(code: String) -> Job {
        Job::spawn("Checking code", move |_| {
            JobResult::LoggedIn(downloader::login_code(code))
        })
    }

    /// Take in everything the worker reported since the last call
    pub fn poll(&mut self) -> JobUpdate {
        loop {
            match self.events.try_recv() {
                Ok(JobEvent::DownloadProgress(mut progress)) => {
                    if progress.title.is_none() {
                        progress.title = self
                            .download_progress
                            .as_ref()
                            .and_then(|previous| previous.title.clone());
                    }
                    self.done = progress.percent.map_or(0, usize::from);
                    self.total = if progress.percent.is_some() { 100 } else { 0 };
                    self.message = progress.phase.into();
                    self.download_progress = Some(progress);
                }
                Ok(JobEvent::Progress {
                    done,
                    total,
                    message,
                }) => {
                    self.done = done;
                    self.total = total;
                    self.message = message;
                }
                Ok(JobEvent::Finished(result)) => return JobUpdate::Finished(result),
                Err(TryRecvError::Empty) => return JobUpdate::Running,
                Err(TryRecvError::Disconnected) => return JobUpdate::Crashed,
            }
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batched_progress_retains_the_song_title_alongside_percent() {
        let (sender, events) = mpsc::channel();
        let mut job = Job {
            title: "Test",
            done: 0,
            total: 0,
            message: String::new(),
            download_progress: None,
            events,
            cancel: None,
        };
        sender
            .send(JobEvent::DownloadProgress(DownloadProgress {
                percent: None,
                phase: "Downloading",
                title: Some("Song".into()),
            }))
            .unwrap();
        sender
            .send(JobEvent::DownloadProgress(DownloadProgress {
                percent: Some(42),
                phase: "Downloading",
                title: None,
            }))
            .unwrap();
        assert!(matches!(job.poll(), JobUpdate::Running));
        assert_eq!((job.done, job.total), (42, 100));
        assert_eq!(
            job.download_progress.as_ref().unwrap().title.as_deref(),
            Some("Song")
        );
    }
}
