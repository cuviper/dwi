mod app;
mod dwarf;
mod text;
mod tree;
mod ui;

use std::{io::IsTerminal, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};

use app::App;
use dwarf::{Database, LoadLimits};

#[derive(Parser)]
#[command(
    name = "dwi",
    version,
    about = "DWARF Inspector: browse DIEs and their attributes",
    group(clap::ArgGroup::new("input").required(true).multiple(true).args(["file", "kernel", "module"]))
)]
struct Args {
    /// Executable, object, or debug file (discovers debuginfo by build ID or .gnu_debuglink)
    #[arg(conflicts_with_all = ["kernel", "module"])]
    file: Option<PathBuf>,
    /// Discover vmlinux for a kernel release (defaults to the running kernel)
    #[arg(long, num_args = 0..=1, value_name = "RELEASE")]
    kernel: Option<Option<String>>,
    /// Discover a kernel module by name (use --kernel RELEASE to select its release)
    #[arg(long, value_name = "NAME")]
    module: Option<String>,
    /// Show the DIE and attribute panes side by side instead of stacked
    #[arg(long)]
    side_by_side: bool,
    /// Maximum compressed-section expansion ratio (sections up to 16 MiB are always allowed)
    #[arg(long, default_value_t = LoadLimits::DEFAULT_MAX_DECOMPRESSION_RATIO, value_parser = clap::value_parser!(u64).range(1..))]
    max_decompression_ratio: u64,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {}", text::format_error(&error));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let limits = LoadLimits {
        max_decompression_ratio: args.max_decompression_ratio,
    };
    let release = args.kernel.as_ref().and_then(|release| release.as_deref());
    let (database, file) = match (args.file, args.module) {
        (Some(file), _) => (Database::open(&file, limits)?, file),
        (None, Some(module)) => Database::open_module(&module, release, limits)?,
        (None, None) => Database::open_kernel(release, limits)?,
    };
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "interactive browsing requires a terminal on stdin and stdout"
    );
    let mut app = App::new(database, file.display().to_string(), !args.side_by_side)?;
    // Ratatui installs a panic hook which restores the terminal as well.
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(error).context("initializing the terminal");
        }
    };
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            if let Event::Key(key) = event::read()?
                && key.kind != KeyEventKind::Release
            {
                match app.handle_key(key) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => app.message = format!("Error: {error:#}"),
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests;
