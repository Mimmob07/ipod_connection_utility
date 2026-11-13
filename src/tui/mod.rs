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
    // Dropped on return: closing the session stops the backend it started.
    let mut wrapper = None;
    let result = event_loop(&mut terminal, &mut app, &mut wrapper);
    ratatui::restore();
    // Let a cancelled download kill gamdl and remove its staging folder before exiting.
    if let Some(job) = app.job.take() {
        job.cancel_and_wait(Duration::from_secs(5));
    }
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    wrapper: &mut Option<crate::wrapper::Session>,
) -> io::Result<()> {
    while !app.should_quit {
        if let Some(backend) = app.pending_setup.take()
            && app.job.is_none()
        {
            // Stop this app's backend, even a failed one; it restarts after setup if needed.
            *wrapper = None;
            app.wrapper_ready = false;
            ratatui::restore();
            let result = setup::run(backend);
            *terminal = ratatui::try_init()?;
            match result {
                Ok(status) if status.success() => app.log(app::Level::Info, "Setup closed. Current queue preserved; sign in/check the session before downloading."),
                Ok(_) => app.log(app::Level::Error, "Setup exited unsuccessfully. Review the installer output; you can retry. Queue preserved."),
                Err(_) => app.log(app::Level::Error, "Setup could not start. Reinstall/extract the complete archive, including its installer wizard."),
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
        terminal.draw(|frame| ui::draw(frame, app))?;
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
