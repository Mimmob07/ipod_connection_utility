//! State of the interface and everything a key press can do to it. Nothing here draws.

use super::worker::{Job, JobResult, JobUpdate};
use crate::downloader::{DownloadBackend, DownloadError, Downloader, LoginResult};
use crate::sync::{self, FileMetadata, FilePlan};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const MAX_LOG_LINES: usize = 500;

/// Shown in the help window and on the Overview tab
pub const KEYS: &[(&str, &str)] = &[
    (
        "Tab, Shift+Tab",
        "Next or previous tab (also Left, Right and 1-4)",
    ),
    (
        "Up, Down, j, k",
        "Move the selection (also PgUp, PgDn, Home, End)",
    ),
    (
        "i",
        "Initialize the database (data.json) of the local library",
    ),
    ("s", "Scan local and iPod music for changes"),
    ("a", "Apply the pending changes to the iPod"),
    ("r", "Reload data.json from disk"),
    ("l", "Change the local library path"),
    ("p", "Change the iPod path"),
    ("n", "Queue a song or playlist URL (Downloader tab)"),
    (
        "t",
        "Retry failed or library-blocked downloads (Downloader tab)",
    ),
    ("v", "Change download quality (Downloader tab)"),
    ("b", "Toggle automatic Rockbox artwork (Downloader tab)"),
    (
        "c",
        "Import Apple web cookies; sign in locally for advanced ALAC",
    ),
    (
        "o",
        "Open Apple's real website for browser sign-in (basic AAC)",
    ),
    ("x", "Forget the imported local Apple web session"),
    ("d", "Install or repair basic AAC downloader dependencies"),
    ("u", "Install optional advanced Docker/ALAC backend"),
    ("?", "Show this help"),
    ("q", "Quit (Ctrl+C quits even while a job is running)"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Overview,
    Library,
    Sync,
    Downloader,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Overview, Tab::Library, Tab::Sync, Tab::Downloader];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Library => "Library",
            Tab::Sync => "Sync",
            Tab::Downloader => "Downloader",
        }
    }

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|x| *x == self).unwrap()
    }
}

/// Whether data.json of the local library could be read
#[derive(PartialEq, Eq, Debug)]
pub enum Database {
    Loaded,
    Missing,
    Unreadable(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Success,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confirm {
    Initialize,
    Sync,
    BasicSetup,
    AdvancedSetup,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    LocalPath,
    IpodPath,
    Download,
    LoginUsername,
    LoginPassword,
    LoginCode,
    CookieFile,
}

/// A window drawn over the tabs that takes every key until it is closed
pub enum Modal {
    Help,
    Confirm {
        title: &'static str,
        lines: Vec<String>,
        action: Confirm,
    },
    Input {
        title: &'static str,
        value: String,
        target: Input,
    },
}

pub struct App {
    pub local_path: String,
    pub ipod_path: String,
    pub tab: Tab,
    pub database: Database,
    pub manifest: Vec<FileMetadata>,
    /// Result of the last scan, invalidated whenever either library path changes.
    pub plan: Option<FilePlan>,
    pub job: Option<Job>,
    pub log: Vec<(Level, String)>,
    pub library_table: TableState,
    pub changes_table: TableState,
    pub download_table: TableState,
    pub modal: Option<Modal>,
    pub downloader: Downloader,
    pub wrapper_status: String,
    pub wrapper_ready: bool,
    pub web_status: String,
    pub pending_setup: Option<DownloadBackend>,
    auth_username: Option<String>,
    active_download: Option<usize>,
    next_download_at: Option<Instant>,
    pub should_quit: bool,
    /// Counts redraws, only used to animate the spinner
    pub tick: usize,
}

impl App {
    pub fn download_cooldown_secs(&self) -> u64 {
        self.next_download_at.map_or(0, |at| {
            at.saturating_duration_since(Instant::now()).as_secs()
        })
    }

    pub fn new(local_path: String, ipod_path: String) -> App {
        let mut app = App {
            local_path,
            ipod_path,
            tab: Tab::Overview,
            database: Database::Missing,
            manifest: Vec::new(),
            plan: None,
            job: None,
            log: Vec::new(),
            library_table: TableState::default().with_selected(0),
            changes_table: TableState::default().with_selected(0),
            download_table: TableState::default().with_selected(0),
            modal: None,
            downloader: Downloader::default(),
            wrapper_status: "Available".into(),
            wrapper_ready: true,
            web_status: if crate::web_auth::has_session() {
                "Web session saved; checked when used".into()
            } else {
                "Not signed in; o opens browser, c imports cookies".into()
            },
            pending_setup: None,
            auth_username: None,
            active_download: None,
            next_download_at: None,
            should_quit: false,
            tick: 0,
        };
        if app.local_path.is_empty() {
            app.log(
                Level::Info,
                "Choose your local music library; no files are changed until you confirm an action",
            );
            app.ask(Input::LocalPath);
        } else {
            app.reload();
        }
        app
    }

    pub fn log(&mut self, level: Level, message: impl Into<String>) {
        self.log.push((level, message.into()));
        if self.log.len() > MAX_LOG_LINES {
            self.log.remove(0);
        }
    }

    /// Read data.json again, used at startup and whenever the local path changes
    pub fn reload(&mut self) {
        let json_path = sync::data_json_path(&self.local_path);
        self.plan = None;
        self.manifest.clear();

        match sync::read_data_json(&json_path) {
            Ok(manifest) => {
                self.log(
                    Level::Info,
                    format!(
                        "Loaded {} tracked files from {}",
                        manifest.len(),
                        json_path.display()
                    ),
                );
                self.manifest = manifest;
                self.database = Database::Loaded;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.log(
                    Level::Info,
                    format!("No {} yet, press i to create it", json_path.display()),
                );
                self.database = Database::Missing;
            }
            Err(error) => {
                self.log(
                    Level::Error,
                    format!("Could not read {}: {error}", json_path.display()),
                );
                self.database = Database::Unreadable(error.to_string());
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if let Some(modal) = self.modal.take() {
            self.on_modal_key(modal, key);
            return;
        }

        match key.code {
            KeyCode::Char('q') => {
                if self.job.is_some() {
                    self.log(
                        Level::Error,
                        "A job is still running, wait for it or press Ctrl+C to quit anyway",
                    );
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('?') => self.modal = Some(Modal::Help),

            KeyCode::Tab | KeyCode::Right => self.switch_tab(1),
            KeyCode::BackTab | KeyCode::Left => self.switch_tab(Tab::ALL.len() - 1),
            KeyCode::Char(c @ '1'..='4') => self.tab = Tab::ALL[c as usize - '1' as usize],

            KeyCode::Down | KeyCode::Char('j') => self.scroll(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1),
            KeyCode::PageDown => self.scroll(10),
            KeyCode::PageUp => self.scroll(-10),
            KeyCode::Home | KeyCode::Char('g') => self.scroll(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.scroll(isize::MAX),

            KeyCode::Char('i') => self.request_initialize(),
            KeyCode::Char('s') => self.start_scan(),
            KeyCode::Char('a') => self.request_sync(),
            KeyCode::Char('r') if !self.busy() => self.reload(),
            KeyCode::Char('l') => self.ask(Input::LocalPath),
            KeyCode::Char('p') => self.ask(Input::IpodPath),
            KeyCode::Char('n') if self.tab == Tab::Downloader => self.ask(Input::Download),
            KeyCode::Char('t') if self.tab == Tab::Downloader && !self.busy() => {
                self.downloader.retry_failed();
                self.next_download_at = None;
                self.log(Level::Info, "Retrying failed and library-blocked downloads");
            }
            KeyCode::Char('v') if self.tab == Tab::Downloader => {
                self.downloader.cycle_quality();
                self.log(
                    Level::Info,
                    format!("Download quality: {}", self.downloader.quality().label()),
                );
            }
            KeyCode::Char('b') if self.tab == Tab::Downloader => {
                self.downloader.toggle_rockbox_art();
                self.log(
                    Level::Info,
                    format!(
                        "Rockbox artwork: {} (new queue entries)",
                        if self.downloader.rockbox_art() {
                            "on"
                        } else {
                            "off"
                        }
                    ),
                );
            }
            KeyCode::Char('c') if self.tab == Tab::Downloader => {
                if self.downloader.backend() == DownloadBackend::Web {
                    self.log(Level::Info, "Sign in at music.apple.com in your browser, export Apple Music cookies locally, then import the file. Never share it or your password.");
                    self.ask(Input::CookieFile);
                } else if self.wrapper_ready {
                    self.ask(Input::LoginUsername);
                } else {
                    self.log(Level::Info, self.wrapper_status.clone());
                }
            }
            KeyCode::Char('o') if self.tab == Tab::Downloader => {
                match crate::web_auth::open_login() {
                    Ok(()) => self.log(Level::Info, "Apple Music browser sign-in opened. Complete login/2FA there; export cookies locally and press c to import. No password forwarding."),
                    Err(_) => self.log(Level::Error, "Could not open a browser. Visit https://music.apple.com yourself; export Apple Music cookies and press c to import."),
                }
            }
            KeyCode::Char('x') if self.tab == Tab::Downloader && !self.busy() => {
                match crate::web_auth::forget() {
                    Ok(()) => {
                        self.web_status = "Web session forgotten; press c to import".into();
                        self.downloader.pause_for_sign_in(DownloadBackend::Web);
                        self.log(Level::Info, "Local imported web session removed. Browser and advanced-wrapper sign-ins are unchanged.");
                    }
                    Err(_) => self.log(Level::Error, "Could not remove the local web session"),
                }
            }
            KeyCode::Char('u') if self.tab == Tab::Downloader => self.request_setup(true),
            KeyCode::Char('d') if self.tab == Tab::Downloader => self.request_setup(false),
            _ => {}
        }
    }

    fn request_setup(&mut self, advanced: bool) {
        if self.busy() {
            return;
        }
        self.modal = Some(Modal::Confirm {
            title: if advanced {
                "Install advanced Apple Music backend?"
            } else {
                "Set up basic AAC downloader?"
            },
            lines: if advanced {
                vec!["Optional Docker/ALAC setup needs your pinned APKM and may request privileged system changes.".into(), "The app will pause for the guided installer, then resume this queue.".into(), "Nothing is installed until you approve the installer steps.".into()]
            } else {
                vec!["Install or repair the private Python AAC downloader. No Docker or APKM needed.".into(), "The app will pause for guided dependency consent, then resume.".into()]
            },
            action: if advanced {
                Confirm::AdvancedSetup
            } else {
                Confirm::BasicSetup
            },
        });
    }

    fn on_modal_key(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            // Any key closes the help
            Modal::Help => {}
            Modal::Confirm { action, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirmed(action),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {}
                _ => self.modal = Some(modal),
            },
            Modal::Input {
                title,
                mut value,
                target,
            } => {
                match key.code {
                    KeyCode::Enter => return self.submitted(target, value.trim()),
                    KeyCode::Esc => return,
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        value.clear()
                    }
                    KeyCode::Char(c) => value.push(c),
                    _ => {}
                }
                self.modal = Some(Modal::Input {
                    title,
                    value,
                    target,
                });
            }
        }
    }

    fn switch_tab(&mut self, steps: usize) {
        self.tab = Tab::ALL[(self.tab.index() + steps) % Tab::ALL.len()];
    }

    fn scroll(&mut self, delta: isize) {
        let (table, len) = match self.tab {
            Tab::Library => (&mut self.library_table, self.manifest.len()),
            Tab::Sync => (
                &mut self.changes_table,
                self.plan.as_ref().map_or(0, FilePlan::len),
            ),
            Tab::Downloader => (&mut self.download_table, self.downloader.queue().len()),
            _ => return,
        };
        if len == 0 {
            return;
        }

        let selected = table.selected().unwrap_or(0) as isize;
        let selected = selected.saturating_add(delta).clamp(0, len as isize - 1);
        table.select(Some(selected as usize));
    }

    /// True, and says so in the log, while a background job owns the library
    fn busy(&mut self) -> bool {
        if self.job.is_some() {
            self.log(Level::Error, "Wait for the running job to finish first");
        }
        self.job.is_some()
    }

    fn ask(&mut self, target: Input) {
        if self.busy() {
            return;
        }

        let (title, value) = match target {
            Input::LocalPath => ("Local library path", self.local_path.clone()),
            Input::IpodPath => ("iPod path", self.ipod_path.clone()),
            Input::Download => ("Apple Music song or playlist URL", String::new()),
            Input::CookieFile => (
                "Apple Music cookie file (local Netscape export)",
                String::new(),
            ),
            Input::LoginUsername => ("Apple Account email", String::new()),
            Input::LoginPassword => ("Apple Account password", String::new()),
            Input::LoginCode => ("Two-factor code", String::new()),
        };
        self.modal = Some(Modal::Input {
            title,
            value,
            target,
        });
    }

    fn request_initialize(&mut self) {
        if self.busy() {
            return;
        }
        if !Path::new(&self.local_path).is_dir() {
            self.log(
                Level::Error,
                format!(
                    "Local library {} is not a folder, press l to change it",
                    self.local_path
                ),
            );
            return;
        }
        let json_path = sync::data_json_path(&self.local_path);
        if json_path.exists() {
            self.log(
                Level::Error,
                format!(
                    "{} already exists, remove it to start over",
                    json_path.display()
                ),
            );
            return;
        }

        self.modal = Some(Modal::Confirm {
            title: "Initialize database",
            lines: vec![
                format!(
                    "Hash music files in {} and write data.json?",
                    self.local_path
                ),
                String::new(),
                "Nothing is copied. Press s afterward to compare both libraries.".to_string(),
            ],
            action: Confirm::Initialize,
        });
    }

    fn start_scan(&mut self) {
        if self.busy() {
            return;
        }
        if self.database != Database::Loaded {
            self.log(
                Level::Error,
                "No database loaded, press i to initialize one",
            );
            return;
        }

        self.plan = None;
        self.job = Some(Job::scan(
            self.local_path.clone(),
            self.ipod_path.clone(),
            self.manifest.clone(),
        ));
    }

    fn request_sync(&mut self) {
        if self.busy() {
            return;
        }
        let Some(plan) = &self.plan else {
            self.log(
                Level::Error,
                "Nothing to sync yet, press s to scan for changes",
            );
            return;
        };
        if plan.is_empty() {
            self.log(
                Level::Info,
                "Nothing to sync, the last scan found no changes",
            );
            return;
        }
        if !Path::new(&self.ipod_path).is_dir() {
            self.log(
                Level::Error,
                format!(
                    "iPod path {} was not found, press p to change it",
                    self.ipod_path
                ),
            );
            return;
        }

        self.modal = Some(Modal::Confirm {
            title: "Sync to iPod",
            lines: vec![
                format!("Target: {}", self.ipod_path),
                String::new(),
                format!(
                    "{} to add, {} to update, {} to delete",
                    plan.add.len(),
                    plan.update.len(),
                    plan.delete.len()
                ),
                "Deleted files are removed from the iPod for good.".to_string(),
            ],
            action: Confirm::Sync,
        });
    }

    fn confirmed(&mut self, action: Confirm) {
        match action {
            Confirm::BasicSetup => self.pending_setup = Some(DownloadBackend::Web),
            Confirm::AdvancedSetup => self.pending_setup = Some(DownloadBackend::Wrapper),
            Confirm::Initialize => self.job = Some(Job::initialize(self.local_path.clone())),
            Confirm::Sync => {
                if let Some(plan) = self.plan.take() {
                    self.job = Some(Job::sync(
                        self.local_path.clone(),
                        self.ipod_path.clone(),
                        self.manifest.clone(),
                        plan,
                    ));
                }
            }
        }
    }

    fn submitted(&mut self, target: Input, value: &str) {
        if value.is_empty() {
            return;
        }

        match target {
            Input::LocalPath => {
                self.local_path = expand_home(value);
                self.reload();
            }
            Input::IpodPath => {
                self.ipod_path = expand_home(value);
                self.plan = None;
                self.log(Level::Info, format!("iPod path set to {}", self.ipod_path));
            }
            Input::Download => match self.downloader.enqueue(value, &self.local_path) {
                Ok(()) => self.log(
                    Level::Info,
                    format!("Queued {value} at {}", self.downloader.quality().label()),
                ),
                Err(error) => self.log(Level::Error, format!("Could not queue {value}: {error}")),
            },
            Input::LoginUsername => {
                self.auth_username = Some(value.to_string());
                self.ask(Input::LoginPassword);
            }
            Input::LoginPassword => {
                if let Some(username) = self.auth_username.take() {
                    self.job = Some(Job::login(username, value.to_string()));
                }
            }
            Input::LoginCode => self.job = Some(Job::login_code(value.to_string())),
            Input::CookieFile => {
                self.web_status = "Importing and checking web session".into();
                self.job = Some(Job::web_login(value.to_string()));
            }
        }
    }

    /// Pick up progress and results of the background job, called once per redraw
    pub fn poll_job(&mut self) {
        let Some(job) = &mut self.job else {
            if self
                .downloader
                .next_backend()
                .is_some_and(|backend| backend != DownloadBackend::Wrapper || self.wrapper_ready)
                && self.modal.is_none()
                && self.next_download_at.is_none_or(|at| Instant::now() >= at)
                && let Some(task) = self.downloader.next_queued()
            {
                self.active_download = Some(task.index);
                self.download_table.select(Some(task.index));
                self.job = Some(Job::download(task));
            }
            return;
        };
        let update = job.poll();
        if let Some(progress) = job.download_progress.take()
            && let Some(index) = self.active_download
        {
            self.downloader.update_progress(index, progress);
        }
        let result = match update {
            JobUpdate::Running => return,
            JobUpdate::Finished(result) => Some(result),
            JobUpdate::Crashed => None,
        };
        let title = job.title;
        let done = job.done;
        self.job = None;

        match result {
            None => {
                if let Some(index) = self.active_download.take() {
                    self.downloader.finish(
                        index,
                        Err(DownloadError::Backend("download worker crashed".into())),
                    );
                }
                self.log(
                    Level::Error,
                    format!("{title} crashed, reloading data.json"),
                );
                self.reload();
            }
            Some(JobResult::Initialized(Ok(manifest))) => {
                self.log(
                    Level::Success,
                    format!("Database created, tracking {} files", manifest.len()),
                );
                self.manifest = manifest;
                self.database = Database::Loaded;
                self.plan = None;
            }
            Some(JobResult::Initialized(Err(error))) => {
                self.log(
                    Level::Error,
                    format!("Could not initialize the database: {error}"),
                );
            }
            Some(JobResult::Scanned {
                manifest,
                plan: Ok(plan),
            }) => {
                let message = if plan.is_empty() {
                    "Scan finished, no changes found".to_string()
                } else {
                    format!(
                        "Scan finished: {} to add, {} to update, {} to delete",
                        plan.add.len(),
                        plan.update.len(),
                        plan.delete.len()
                    )
                };
                self.log(Level::Success, message);
                self.manifest = manifest;
                self.plan = Some(plan);
                self.changes_table.select(Some(0));
            }
            Some(JobResult::Scanned {
                plan: Err(error), ..
            }) => {
                self.log(Level::Info,
                    "Press s to resume using saved hashes. Reconnect the iPod first if it disconnected");
                self.log(Level::Error, format!("Scan failed: {error}"));
            }
            Some(JobResult::Synced { manifest, result }) => {
                self.manifest = manifest;
                match result {
                    Ok(()) => {
                        self.log(Level::Success, format!("Sync finished, {done} changes applied"));
                        self.plan = Some(FilePlan::default());
                    }
                    Err(error) => self.log(
                        Level::Error,
                        format!(
                            "Sync stopped after {done} changes: {error}. Scan again to see what is left"
                        ),
                    ),
                }
            }
            Some(JobResult::Downloaded { index, result }) => {
                self.active_download = None;
                let delay = if matches!(&result, Err(DownloadError::RateLimited(_)))
                    || matches!(&result, Ok(outcome) if outcome.rate_limited)
                {
                    Duration::from_secs(5 * 60)
                } else {
                    Duration::from_secs(1)
                };
                self.next_download_at = Some(Instant::now() + delay);
                match &result {
                    Ok(outcome) => {
                        if outcome.saved > 0 {
                            self.plan = None;
                        }
                        self.log(Level::Success, outcome.detail());
                        for issue in &outcome.issues {
                            self.log(Level::Info, issue);
                        }
                    }
                    Err(error) => {
                        if matches!(error, DownloadError::SignInRequired) {
                            self.sign_in_required(index);
                        }
                        self.log(Level::Error, format!("Download failed: {error}"));
                    }
                }
                self.downloader.finish(index, result);
            }
            Some(JobResult::PlaylistResolved { index, result }) => {
                self.active_download = None;
                let result =
                    result.and_then(|details| self.downloader.expand_playlist(index, details));
                match result {
                    Ok(count) => {
                        self.download_table.select(Some(index));
                        self.log(
                            Level::Success,
                            format!("Playlist loaded: {count} songs in the queue"),
                        );
                    }
                    Err(error) => {
                        if matches!(error, DownloadError::RateLimited(_)) {
                            self.next_download_at =
                                Some(Instant::now() + Duration::from_secs(5 * 60));
                        }
                        if matches!(error, DownloadError::SignInRequired) {
                            self.sign_in_required(index);
                        }
                        self.log(
                            Level::Error,
                            format!("Could not load playlist songs: {error}"),
                        );
                        self.downloader.finish(index, Err(error));
                    }
                }
            }
            Some(JobResult::WebLoggedIn(result)) => match result {
                Ok(LoginResult::Authenticated) => {
                    self.web_status = "Signed in".into();
                    self.downloader.retry_sign_in_required(DownloadBackend::Web);
                    self.log(
                        Level::Success,
                        "Apple Music web session checked. Password/2FA stay in your browser.",
                    );
                }
                Ok(LoginResult::NeedsCode) => self.log(
                    Level::Error,
                    "Complete 2FA in the real browser and import cookies again",
                ),
                Err(error) => {
                    self.web_status = "Web session not verified; press c to retry".into();
                    self.log(Level::Error, format!("Web sign-in check failed: {error}"));
                }
            },
            Some(JobResult::LoggedIn(result)) => match result {
                Ok(LoginResult::Authenticated) => {
                    self.wrapper_status = "Signed in".into();
                    self.log(
                        Level::Success,
                        "Signed in. Restored sessions will be reused while valid",
                    );
                    self.downloader
                        .retry_sign_in_required(DownloadBackend::Wrapper);
                }
                Ok(LoginResult::NeedsCode) => {
                    self.log(Level::Info, "Apple Music requires a two-factor code");
                    self.ask(Input::LoginCode);
                }
                Err(error) => self.log(Level::Error, format!("Sign in failed: {error}")),
            },
        }
    }

    fn sign_in_required(&mut self, index: usize) {
        let backend = self
            .downloader
            .queue()
            .get(index)
            .map_or(DownloadBackend::Web, |item| item.quality.backend());
        if backend == DownloadBackend::Web {
            self.web_status = "Web sign-in required; o browser, c import".into();
        } else {
            self.wrapper_status = "Sign in required; press c".into();
        }
        self.downloader.pause_for_sign_in(backend);
    }

    pub fn wrapper_event(&mut self, event: crate::wrapper::Event) {
        match event {
            crate::wrapper::Event::Ready {
                seconds,
                authenticated,
            } => {
                self.wrapper_ready = true;
                self.wrapper_status = if authenticated {
                    "Signed in"
                } else {
                    "Ready; press c to sign in"
                }
                .into();
                self.log(
                    Level::Success,
                    format!(
                        "Apple Music ready in {seconds:.1}s{}",
                        if authenticated {
                            "; saved session restored"
                        } else {
                            ""
                        }
                    ),
                );
            }
            crate::wrapper::Event::Failed(error) => {
                self.wrapper_ready = false;
                self.wrapper_status = "Unavailable; see log".into();
                self.log(Level::Error, error);
            }
        }
    }
}

fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}
