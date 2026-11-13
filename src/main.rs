mod downloader;
mod settings;
mod sync;
mod tui;
mod web_auth;
mod wrapper;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{self, IsTerminal};
use std::process::ExitCode;

/// A tool that helps you keep your local and ipod libraries up to date with each other
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Initialize the data.json file, run this the first time you use the program
    #[arg(short, long)]
    initialize: bool,

    /// Music library folder path
    #[arg(short, long)]
    local_path: Option<String>,

    /// iPod mount or Music folder. Required for noninteractive sync; in the TUI
    /// it can be entered later with p.
    #[arg(short, long)]
    path: Option<String>,

    /// Sync right away without opening the interface, this is also what happens when the
    /// program is not run from a terminal
    #[arg(short, long)]
    no_tui: bool,

    /// Scan both libraries and print the plan without copying, deleting or saving anything
    #[arg(long, conflicts_with = "initialize")]
    dry_run: bool,
}

fn initialize(master_path: &str) -> io::Result<()> {
    let progress_bar = ProgressBar::new(0);
    progress_bar.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{bar:.cyan/blue}] {pos}/{len} ({percent}%) {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    sync::initialize_data(master_path, &mut |progress| {
        progress_bar.set_length(progress.total as u64);
        progress_bar.set_position(progress.done as u64);
        progress_bar.set_message(format!(
            "Working on {:?}",
            progress.path.file_name().unwrap_or_default()
        ));
    })?;
    progress_bar.finish_with_message("Done!");

    Ok(())
}

fn sync_once(local_path: &str, ipod_path: &str, dry_run: bool) -> io::Result<()> {
    let json_path = sync::data_json_path(local_path);
    let mut manifest = sync::read_data_json(&json_path)?;

    // Check for changes
    let scan = if dry_run {
        sync::preview_changes
    } else {
        sync::find_changes
    };
    let plan = scan(local_path, ipod_path, &mut manifest, &mut |_| {})?;
    println!(
        "{} to add, {} to update, {} to delete",
        plan.add.len(),
        plan.update.len(),
        plan.delete.len()
    );
    for path in &plan.add {
        println!("Add {}", path.display());
    }
    for file in &plan.update {
        println!("Update {}", file.file_path.display());
    }
    for path in &plan.delete {
        println!("Delete {}", path.display());
    }
    if dry_run {
        return Ok(());
    }

    // Deal with changes
    let result = sync::apply_plan(&mut manifest, plan, ipod_path, &mut |progress| {
        println!("{} {}", progress.action, progress.path.display());
    });

    // Save the local cache and completed destination fingerprints even after a failure.
    sync::write_data_json(&json_path, &manifest)?;

    result
}

fn main() -> ExitCode {
    // Display, not Debug, so errors read as sentences.
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run() -> io::Result<()> {
    if wrapper::run_manager_if_requested()? {
        return Ok(());
    }
    let args = Args::parse();

    if args.initialize {
        initialize(args.local_path.as_deref().unwrap_or("./"))
    } else if args.dry_run
        || args.no_tui
        || !io::stdout().is_terminal()
        || !io::stdin().is_terminal()
    {
        let ipod_path = args.path.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--path is required for noninteractive sync",
            )
        })?;
        sync_once(
            args.local_path.as_deref().unwrap_or("./"),
            ipod_path,
            args.dry_run,
        )
    } else {
        let (local, ipod) = settings::load().interactive_paths(args.local_path, args.path);
        tui::run(local, ipod)
    }
}
