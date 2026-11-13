mod app;
mod setup;
mod ui;
mod worker;

use app::App;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::io;
use std::time::Duration;

// How long to wait for a key before redrawing, this is what keeps job progress moving
const REDRAW_INTERVAL: Duration = Duration::from_millis(100);

pub fn run(local_path: String, ipod_path: String) -> io::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let mut app = App::new(local_path, ipod_path);
    app.wrapper_ready = false;
    app.wrapper_status = "Open Downloader to start Apple Music".into();
    let mut wrapper = None;
    let result = event_loop(&mut terminal, app, &mut wrapper);
    ratatui::restore();
    if let Some(wrapper) = &mut wrapper {
        wrapper.shutdown();
    }
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    mut app: App,
    wrapper: &mut Option<crate::wrapper::Session>,
) -> io::Result<()> {
    while !app.should_quit {
        if let Some(backend) = app.pending_setup.take() {
            if wrapper.is_some() {
                app.log(app::Level::Error, "Advanced backend is already active. Restart the app and use setup before selecting ALAC; active account/session state was not changed.");
            } else if app.job.is_none() {
                ratatui::restore();
                let result = setup::run(backend);
                *terminal = ratatui::try_init()?;
                match result {
                    Ok(status) if status.success() => app.log(app::Level::Info, "Setup closed. Current queue preserved; sign in/check the session before downloading."),
                    Ok(_) => app.log(app::Level::Error, "Setup exited unsuccessfully. Review the installer output; you can retry. Queue preserved."),
                    Err(_) => app.log(app::Level::Error, "Setup could not start. Reinstall/extract the complete archive, including its installer wizard."),
                }
            }
        }
        if (app.downloader.backend() == crate::downloader::DownloadBackend::Wrapper
            || app.downloader.next_backend() == Some(crate::downloader::DownloadBackend::Wrapper))
            && wrapper.is_none()
        {
            app.wrapper_status = "Starting Apple Music...".into();
            *wrapper = Some(crate::wrapper::Session::start());
        }
        if let Some(event) = wrapper.as_ref().and_then(crate::wrapper::Session::poll) {
            app.wrapper_event(event);
        }
        app.poll_job();
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        app.tick += 1;

        if event::poll(REDRAW_INTERVAL)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            let before = (app.local_path.clone(), app.ipod_path.clone());
            app.on_key(key);
            if before != (app.local_path.clone(), app.ipod_path.clone())
                && let Err(error) = crate::settings::save(&app.local_path, &app.ipod_path)
            {
                app.log(
                    app::Level::Error,
                    format!("Could not remember folders: {error}"),
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
