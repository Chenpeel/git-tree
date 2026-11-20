mod changes;
mod graph;
#[cfg(feature = "gui")]
mod gui;
#[cfg(feature = "gui")]
mod history;
mod rails;
#[cfg(feature = "gui")]
mod renderer;
mod repository;
#[cfg(feature = "gui")]
mod scene;
mod state;
mod tui;
mod worker;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum View {
    #[default]
    #[value(name = "3d")]
    Graph3d,
    Rails,
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Local Git history in 3D or compact 2D rails. No system Git required."
)]
struct Args {
    /// Repository or a directory inside it. Defaults to the current directory.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Open the mouse-enabled terminal interface instead of the native GUI.
    #[arg(long)]
    tui: bool,

    /// GUI view: abstract Git 3D (default) or compact 2D rails.
    #[arg(long, value_enum, default_value_t = View::Graph3d, conflicts_with = "tui")]
    view: View,

    /// Branch or full ref to use as the central trunk (auto: main, master, HEAD).
    #[arg(long)]
    main_branch: Option<String>,

    /// Print visual asset licensing information without opening a repository.
    #[arg(long)]
    licenses: bool,

    /// Limit loaded commits. Omit to read all reachable history.
    #[arg(long, value_parser = clap::value_parser!(usize))]
    max_commits: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.licenses {
        println!(
            "The Git 3D view and switchable 2D rails use procedural geometry. No third-party visual textures or models are bundled in this executable."
        );
        return Ok(());
    }
    anyhow::ensure!(
        args.max_commits != Some(0),
        "--max-commits must be positive"
    );
    let location = repository::Location::discover(&args.path)?;
    let config = repository::LoadOptions {
        main_branch: args.main_branch,
        max_commits: args.max_commits,
    };
    let worker = worker::Worker::start(location, config)?;
    let state = state::AppState::new(worker);

    if args.tui {
        return tui::run(state);
    }
    #[cfg(feature = "gui")]
    {
        gui::run(state, args.view)
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = state;
        anyhow::bail!("This build has no GUI. Run with --tui or install the default build.")
    }
}
