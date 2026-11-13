//! Draws the `App`. Only the table scroll positions are written back.

use super::app::{App, Database, KEYS, Level, Modal, Tab};
use crate::downloader::{DownloadBackend, DownloadStatus, Downloader};
use crate::sync::DATA_FILE;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph, Row, Table, Tabs, Wrap};
use std::path::Path;
use std::time::SystemTime;

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 18;
const LOG_HEIGHT: u16 = 6;
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];
const PROGRESS_BAR_WIDTH: usize = 20;

const ACCENT: Color = Color::LightMagenta;
// Bright ANSI colors keep secondary text visible on transparent dark themes.
const SECONDARY: Style = Style::new().fg(Color::LightCyan);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let message = format!("Terminal too small, at least {MIN_WIDTH}x{MIN_HEIGHT} is needed");
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), area);
        return;
    }

    let [header, body, log, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(LOG_HEIGHT),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header);
    match app.tab {
        Tab::Overview => draw_overview(frame, app, body),
        Tab::Library => draw_library(frame, app, body),
        Tab::Sync => draw_sync(frame, app, body),
        Tab::Downloader => draw_downloader(frame, app, body),
    }
    draw_log(frame, app, log);
    draw_footer(frame, app, footer);

    if let Some(modal) = &app.modal {
        draw_modal(frame, modal, area);
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::bordered()
        .title(format!(" {title} "))
        .padding(Padding::horizontal(1))
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let titles = Tab::ALL.iter().enumerate().map(|(idx, tab)| {
        if *tab == Tab::Downloader && !Downloader::IMPLEMENTED {
            format!("{} {} (planned)", idx + 1, tab.title())
        } else {
            format!("{} {}", idx + 1, tab.title())
        }
    });
    let version = format!(" v{} ", env!("CARGO_PKG_VERSION"));
    let tabs = Tabs::new(titles)
        .select(app.tab.index())
        .highlight_style(
            Style::new()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .block(
            Block::bordered()
                .title(Span::styled(" iPod Connection Utility ", BOLD))
                .title_top(Line::from(version).right_aligned()),
        );

    frame.render_widget(tabs, area);
}

fn draw_overview(frame: &mut Frame, app: &App, area: Rect) {
    let [paths, changes, keys] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);

    let found = |path: &str| match Path::new(path).is_dir() {
        true => Span::styled("found", Style::new().fg(Color::LightGreen)),
        false => Span::styled("not found", Style::new().fg(Color::LightRed)),
    };
    let (database_status, database_detail) = match &app.database {
        Database::Loaded => (
            Span::styled("loaded", Style::new().fg(Color::LightGreen)),
            format!(
                "{DATA_FILE}, {} files, {}",
                app.manifest.len(),
                human_size(app.manifest.iter().map(|x| x.size).sum())
            ),
        ),
        Database::Missing => (
            Span::styled("missing", Style::new().fg(Color::LightYellow)),
            format!("{DATA_FILE}, press i to initialize"),
        ),
        Database::Unreadable(error) => (
            Span::styled("unreadable", Style::new().fg(Color::LightRed)),
            format!("{DATA_FILE}, {error}"),
        ),
    };
    let rows = [
        Row::new([
            Span::styled("Local library", SECONDARY),
            found(&app.local_path),
            Span::raw(app.local_path.as_str()),
        ]),
        Row::new([
            Span::styled("iPod", SECONDARY),
            found(&app.ipod_path),
            Span::raw(app.ipod_path.as_str()),
        ]),
        Row::new([
            Span::styled("Database", SECONDARY),
            database_status,
            Span::raw(database_detail),
        ]),
    ];
    let widths = [
        Constraint::Length(13),
        Constraint::Length(10),
        Constraint::Min(0),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .column_spacing(2)
            .block(panel("Status")),
        paths,
    );

    let pending = match (&app.job, &app.plan) {
        (Some(job), _) => Line::from(format!("{} ...", job.title)),
        (None, None) => Line::from("Not scanned yet, press s to scan local and iPod music"),
        (None, Some(plan)) if plan.is_empty() => Line::styled(
            "Up to date as of the last scan",
            Style::new().fg(Color::LightGreen),
        ),
        (None, Some(plan)) => Line::from(vec![
            Span::styled(
                format!(
                    "{} to add, {} to update, {} to delete.",
                    plan.add.len(),
                    plan.update.len(),
                    plan.delete.len()
                ),
                Style::new().fg(Color::LightYellow),
            ),
            Span::raw(" Press a to sync, or 3 to review them first"),
        ]),
    };
    frame.render_widget(
        Paragraph::new(pending).block(panel("Pending changes")),
        changes,
    );

    let rows = KEYS
        .iter()
        .map(|(key, description)| Row::new([Span::styled(*key, BOLD), Span::raw(*description)]));
    let widths = [Constraint::Length(14), Constraint::Min(0)];
    frame.render_widget(
        Table::new(rows, widths)
            .column_spacing(2)
            .block(panel("Keys")),
        keys,
    );
}

fn draw_library(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.manifest.is_empty() {
        let message = match app.database {
            Database::Loaded => {
                "The database is empty, the local library had no files when it was created"
            }
            _ => "No database loaded, press i to initialize one for the local library",
        };
        frame.render_widget(Paragraph::new(message).block(panel("Library")), area);
        return;
    }

    let title = format!(
        "Library, {} files, {}",
        app.manifest.len(),
        human_size(app.manifest.iter().map(|x| x.size).sum())
    );
    let rows = app.manifest.iter().map(|file| {
        Row::new([
            file_name(&file.file_path),
            folder(&file.file_path, &app.local_path),
            human_size(file.size),
            age(file.mtime),
            format!("{:016x}", file.hash),
        ])
    });
    let widths = [
        Constraint::Fill(3),
        Constraint::Fill(2),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(16),
    ];
    let table = Table::new(rows, widths)
        .header(Row::new(["File", "Folder", "Size", "Modified", "Hash"]).style(SECONDARY))
        .column_spacing(2)
        .row_highlight_style(SELECTED)
        .block(panel(&title));

    frame.render_stateful_widget(table, area, &mut app.library_table);
}

fn draw_sync(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(plan) = app.plan.as_ref().filter(|plan| !plan.is_empty()) else {
        let message = match (&app.job, &app.plan) {
            (Some(job), _) => format!("{} ...", job.title),
            (None, None) => {
                "Not scanned yet, press s to scan local and iPod music for changes".to_string()
            }
            (None, Some(_)) => "Up to date as of the last scan, press s to scan again".to_string(),
        };
        frame.render_widget(
            Paragraph::new(message).block(panel("Pending changes")),
            area,
        );
        return;
    };

    let title = format!(
        "Pending changes, {} to add, {} to update, {} to delete",
        plan.add.len(),
        plan.update.len(),
        plan.delete.len()
    );
    let change = |action: &'static str, color: Color, path: &Path| {
        Row::new([
            Span::styled(action, Style::new().fg(color)),
            Span::raw(file_name(path)),
            Span::raw(folder(path, &app.local_path)),
        ])
    };
    let rows = (plan
        .add
        .iter()
        .map(|path| change("Add", Color::LightGreen, path)))
    .chain(
        plan.update
            .iter()
            .map(|file| change("Update", Color::LightYellow, &file.file_path)),
    )
    .chain(
        plan.delete
            .iter()
            .map(|path| change("Delete", Color::LightRed, path)),
    );
    let widths = [
        Constraint::Length(6),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ];
    let table = Table::new(rows, widths)
        .header(Row::new(["Action", "File", "Folder"]).style(SECONDARY))
        .column_spacing(2)
        .row_highlight_style(SELECTED)
        .block(panel(&title));

    frame.render_stateful_widget(table, area, &mut app.changes_table);
}

fn draw_downloader(frame: &mut Frame, app: &mut App, area: Rect) {
    let [about, progress, queue] = Layout::vertical([
        Constraint::Length(11),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);
    let web = app.downloader.backend() == DownloadBackend::Web;
    let status = Span::styled(
        if web {
            app.web_status.as_str()
        } else {
            app.wrapper_status.as_str()
        },
        Style::new().fg(if web || app.wrapper_ready {
            Color::LightGreen
        } else {
            Color::LightYellow
        }),
    );
    let selected_detail = app
        .download_table
        .selected()
        .and_then(|index| app.downloader.queue().get(index))
        .and_then(|item| match &item.status {
            DownloadStatus::Unavailable(reason)
            | DownloadStatus::Blocked(reason)
            | DownloadStatus::Failed(reason) => Some(reason.clone()),
            _ => None,
        });
    let lines = vec![
        Line::from(vec![
            Span::styled("Mode         ", SECONDARY),
            Span::raw(app.downloader.backend().label()),
        ]),
        Line::from(vec![Span::styled("Status       ", SECONDARY), status]),
        Line::from(vec![
            Span::styled("Destination  ", SECONDARY),
            Span::raw(app.local_path.as_str()),
        ]),
        Line::from(vec![
            Span::styled("Quality      ", SECONDARY),
            Span::raw(app.downloader.quality().label()),
            Span::styled("  (v to change)", SECONDARY),
        ]),
        Line::from(vec![
            Span::styled("Artwork      ", SECONDARY),
            Span::raw(if app.downloader.rockbox_art() {
                "Rockbox: 500x500 JPEG"
            } else {
                "Original"
            }),
            Span::styled("  (b to change)", SECONDARY),
        ]),
        Line::from(if web {
            "o: browser login | c: import cookie file | x: forget session | d: basic setup | u: advanced setup"
        } else if !app.wrapper_ready {
            "Apple Music is starting; queue song or playlist URLs with n while you wait."
        } else if app.wrapper_status == "Signed in" {
            "Press n for songs/playlists; t retries failed or library-blocked entries."
        } else {
            "Press c to sign in, then n to queue a song or playlist URL."
        }),
        Line::from(if app.download_cooldown_secs() > 0 {
            format!(
                "Next song in {}s. Press s to scan saved songs, then a to sync.",
                app.download_cooldown_secs()
            )
        } else {
            "Press s to scan saved songs, then a to sync.".into()
        }),
        Line::from(
            selected_detail
                .or_else(|| app.downloader.last_detail())
                .unwrap_or_default(),
        ),
    ];
    frame.render_widget(Paragraph::new(lines).block(panel("Song downloader")), about);

    let downloads = app.downloader.queue();
    let complete = app.downloader.completed();
    let total = downloads.len();
    let filled = (PROGRESS_BAR_WIDTH * complete)
        .checked_div(total)
        .unwrap_or(0);
    let saved = downloads
        .iter()
        .filter(
            |item| matches!(&item.status, DownloadStatus::Finished(outcome) if outcome.saved > 0),
        )
        .count();
    let failed = downloads
        .iter()
        .filter(|item| matches!(&item.status, DownloadStatus::Failed(_)))
        .count();
    let summary = format!(
        "[{}{}] {complete}/{total} processed, {saved} downloaded, {failed} failed",
        "#".repeat(filled),
        "-".repeat(PROGRESS_BAR_WIDTH - filled)
    );
    frame.render_widget(
        Paragraph::new(summary).block(panel("Queue progress")),
        progress,
    );
    if downloads.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "The queue is empty, press n to queue a download",
                SECONDARY,
            ))
            .block(panel("Queue")),
            queue,
        );
        return;
    }
    let rows = downloads.iter().map(|download| {
        let (status, color) = match &download.status {
            DownloadStatus::Downloading => (
                match download.percent {
                    Some(percent) => format!("{} {percent}%", download.phase),
                    None if !download.phase.is_empty() => download.phase.clone(),
                    None => "Downloading".into(),
                },
                Color::LightYellow,
            ),
            DownloadStatus::Finished(_) => (download.status.to_string(), Color::LightGreen),
            DownloadStatus::Failed(_) | DownloadStatus::Blocked(_) => {
                (download.status.to_string(), Color::LightRed)
            }
            _ => (download.status.to_string(), Color::LightCyan),
        };
        Row::new(vec![
            Span::raw(download.title.as_str()),
            Span::raw(download.artist.as_str()),
            Span::raw(
                download
                    .playlist
                    .as_ref()
                    .map_or("", |playlist| playlist.title.as_str()),
            ),
            Span::styled(status, Style::new().fg(color)),
        ])
    });
    let widths = [
        Constraint::Fill(3),
        Constraint::Fill(2),
        Constraint::Fill(2),
        Constraint::Length(22),
    ];
    let table = Table::new(rows, widths)
        .header(Row::new(["Song", "Artist", "Playlist", "Status"]).style(SECONDARY))
        .column_spacing(2)
        .row_highlight_style(SELECTED)
        .block(panel("Queue (Up/Down to scroll)"));
    frame.render_stateful_widget(table, queue, &mut app.download_table);
}

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let visible = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = app.log[app.log.len().saturating_sub(visible)..]
        .iter()
        .map(|(level, message)| {
            let label = match level {
                Level::Info => Span::styled("info   ", SECONDARY),
                Level::Success => Span::styled("done   ", Style::new().fg(Color::LightGreen)),
                Level::Error => Span::styled("error  ", Style::new().fg(Color::LightRed)),
            };
            Line::from(vec![label, Span::raw(message.as_str())])
        })
        .collect();

    // Give the latest error the full panel so paths and OS errors can wrap.
    let paragraph = if app
        .log
        .last()
        .is_some_and(|(level, _)| *level == Level::Error)
    {
        Paragraph::new(lines.pop().unwrap_or_default()).wrap(Wrap { trim: false })
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(paragraph.block(panel("Log")), area);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let Some(job) = &app.job else {
        let hints = match app.tab {
            Tab::Overview => "Tab switch tab | s scan | a sync | i initialize | ? help | q quit",
            Tab::Library => "Up/Down move | s scan | r reload | Tab switch tab | ? help | q quit",
            Tab::Sync => "Up/Down move | s scan | a sync | Tab switch tab | ? help | q quit",
            Tab::Downloader => {
                "n URL | t retry | v quality | b art | c login/import | o browser | d/u setup | q quit"
            }
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {hints}"), SECONDARY)),
            area,
        );
        return;
    };

    let mut spans = vec![Span::styled(
        format!(" {} {}", SPINNER[app.tick % SPINNER.len()], job.title),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    if let Some(filled) = (PROGRESS_BAR_WIDTH * job.done).checked_div(job.total) {
        spans.push(Span::raw(format!(
            "  [{}{}] {}/{}  ",
            "#".repeat(filled),
            "-".repeat(PROGRESS_BAR_WIDTH - filled),
            job.done,
            job.total
        )));
        spans.push(Span::styled(job.message.as_str(), SECONDARY));
    } else if !job.message.is_empty() {
        spans.push(Span::styled(format!("  {}", job.message), SECONDARY));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_modal(frame: &mut Frame, modal: &Modal, area: Rect) {
    let hint = |text: &'static str| Line::styled(text, SECONDARY);
    let (title, mut lines, footer) = match modal {
        Modal::Help => (
            "Help",
            KEYS.iter()
                .map(|(key, description)| {
                    Line::from(vec![
                        Span::styled(format!("{key:<16}"), BOLD),
                        Span::raw(*description),
                    ])
                })
                .collect::<Vec<Line>>(),
            hint("Press any key to close"),
        ),
        Modal::Confirm { title, lines, .. } => (
            *title,
            lines.iter().map(|line| Line::from(line.as_str())).collect(),
            hint("y confirm | n or Esc cancel"),
        ),
        Modal::Input { title, .. } => (
            *title,
            vec![Line::default()],
            hint("Enter confirm | Esc cancel | Ctrl+U clear"),
        ),
    };
    lines.push(Line::default());
    lines.push(footer);

    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = (widest.max(50) + 4).min(area.width - 4);
    let height = (lines.len() as u16 + 2).min(area.height - 2);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    if let Modal::Input { value, target, .. } = modal {
        // Keep the end of a long value in view, that is where the typing happens
        let room = popup.width.saturating_sub(5) as usize;
        let length = value.chars().count();
        let shown: String = if matches!(
            target,
            super::app::Input::LoginPassword | super::app::Input::LoginCode
        ) {
            "*".repeat(length.min(room))
        } else {
            value.chars().skip(length.saturating_sub(room)).collect()
        };
        frame.set_cursor_position((popup.x + 2 + length.min(room) as u16, popup.y + 1));
        lines[0] = Line::from(shown);
    }

    let block = panel(title).border_style(Style::new().fg(ACCENT));
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Folder of `path` inside the local library, empty for files at the top of it
fn folder(path: &Path, local_path: &str) -> String {
    let parent = path.parent().unwrap_or(Path::new(""));
    parent
        .strip_prefix(local_path)
        .unwrap_or(parent)
        .to_string_lossy()
        .into_owned()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    match unit {
        0 => format!("{bytes} B"),
        _ => format!("{size:.1} {}", UNITS[unit]),
    }
}

/// How long ago `time` was, the standard library has no calendar to print a date with
fn age(time: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(time) else {
        return "just now".to_string();
    };

    match elapsed.as_secs() {
        0..60 => "just now".to_string(),
        secs @ 60..3600 => format!("{} min ago", secs / 60),
        secs @ 3600..86400 => format!("{} h ago", secs / 3600),
        secs => format!("{} d ago", secs / 86400),
    }
}
