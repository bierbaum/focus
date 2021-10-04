mod app;
mod backed_up_file;
mod coordinate;
mod coordinate_resolver;
mod detail;
mod git_helper;
mod model;
mod sandbox;
mod sandbox_command;
mod sparse_repos;
mod subcommands;
mod temporary_working_directory;
mod testing;
mod tracker;
mod ui;
mod working_tree_synchronizer;

#[macro_use]
extern crate lazy_static;

use anyhow::{bail, Context, Result};
use env_logger::{self, Env};

use tracker::Tracker;

use std::{
    fmt::Display,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Instant,
};
use structopt::StructOpt;

use crate::app::App;

#[derive(Debug)]
struct CommaSeparatedStrings(Vec<String>);

impl FromStr for CommaSeparatedStrings {
    type Err = std::string::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let splayed: Vec<_> = s.split(",").map(|s| s.to_owned()).collect();
        Ok(CommaSeparatedStrings(splayed))
    }
}

impl Display for CommaSeparatedStrings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join(", "))
    }
}

#[derive(StructOpt, Debug)]
enum Subcommand {
    /// Create a sparse clone from named layers or ad-hoc build coordinates
    Clone {
        /// Path to the existing dense repository that the sparse clone shall be based upon.
        #[structopt(long, parse(from_os_str))]
        dense_repo: PathBuf,

        /// Path where the new sparse repository should be created.
        #[structopt(long, parse(from_os_str))]
        sparse_repo: PathBuf,

        /// The name of the branch to clone.
        #[structopt(long)]
        branch: String,

        /// Bazel build coordinates to include as an ad-hoc layer set, cannot be specified in combination with 'layers'.
        #[structopt(long, default_value = "")]
        coordinates: CommaSeparatedStrings,

        /// Named layers to include. Comma separated, loaded from the dense repository's `focus/projects` directory), cannot be specified in combination with 'coordinates'.
        #[structopt(long, default_value = "")]
        layers: CommaSeparatedStrings,
    },

    /// Update the sparse checkout to reflect changes to the build graph.
    Sync {
        /// Path to the sparse repository.
        #[structopt(parse(from_os_str), default_value = ".")]
        sparse_repo: PathBuf,
    },

    /// List available layers
    AvailableLayers {
        /// Path to the repository.
        #[structopt(long, parse(from_os_str), default_value = ".")]
        repo: PathBuf,
    },

    /// List currently selected layers
    SelectedLayers {
        /// Path to the repository.
        #[structopt(long, parse(from_os_str), default_value = ".")]
        repo: PathBuf,
    },

    /// Push a layer onto the top of the stack of currently selected layers
    PushLayer {
        /// Path to the repository.
        #[structopt(long, parse(from_os_str), default_value = ".")]
        repo: PathBuf,

        /// Names of layers to push.
        names: Vec<String>,
    },

    /// Pop one or more layer(s) from the top of the stack of current selected layers
    PopLayer {
        /// Path to the repository.
        #[structopt(long, parse(from_os_str), default_value = ".")]
        repo: PathBuf,

        /// The number of layers to pop.
        #[structopt(long, default_value = "1")]
        count: usize,
    },

    /// Filter out one or more layer(s) from the stack of currently selected layers
    RemoveLayer {
        /// Path to the repository.
        #[structopt(long, parse(from_os_str), default_value = ".")]
        repo: PathBuf,

        /// Names of the layers to be removed.
        names: Vec<String>,
    },

    /// List focused repositories
    ListRepos {},

    /// Detect whether there are changes to the build graph (used internally)
    DetectBuildGraphChanges {
        // Path to the repository.
        #[structopt(long, parse(from_os_str), default_value = ".")]
        repo: PathBuf,
    },

    /// Test the user interface
    UserInterfaceTest {},
}

#[derive(StructOpt, Debug)]
#[structopt(about = "Focused Development Tools")]
struct ParachuteOpts {
    /// Preserve the created sandbox directory for inspecting logs and other files.
    #[structopt(long)]
    preserve_sandbox: bool,

    /// Disable textual user interface; happens by default on non-interactive terminals.
    #[structopt(long)]
    ugly: bool,

    #[structopt(subcommand)]
    cmd: Subcommand,
}

fn filter_empty_strings(string_list: Vec<String>) -> Vec<String> {
    string_list
        .iter()
        .filter_map(|s| {
            if !s.is_empty() {
                Some(s.to_owned())
            } else {
                None
            }
        })
        .collect()
}

fn ensure_directories_exist() -> Result<()> {
    Tracker::default()
        .ensure_directories_exist()
        .context("creating directories for the tracker")?;

    Ok(())
}
fn expand_tilde<P: AsRef<Path>>(path_user_input: P) -> Result<PathBuf> {
    let p = path_user_input.as_ref();
    if !p.starts_with("~") {
        return Ok(p.to_path_buf());
    }
    if p == Path::new("~") {
        if let Some(home_dir) = dirs::home_dir() {
            return Ok(home_dir);
        } else {
            bail!("Could not determine home directory");
        }
    }

    let result = dirs::home_dir().map(|mut h| {
        if h == Path::new("/") {
            // Corner case: `h` root directory;
            // don't prepend extra `/`, just drop the tilde.
            p.strip_prefix("~").unwrap().to_path_buf()
        } else {
            h.push(p.strip_prefix("~/").unwrap());
            h
        }
    });

    if let Some(path) = result {
        Ok(path)
    } else {
        bail!("Failed to expand tildes in path '{}'", p.display());
    }
}

fn run_subcommand(app: Arc<App>, options: ParachuteOpts, interactive: bool) -> Result<()> {
    let cloned_app = app.clone();

    match options.cmd {
        Subcommand::Clone {
            dense_repo,
            sparse_repo,
            branch,
            layers,
            coordinates,
        } => {
            let dense_repo = expand_tilde(dense_repo)?;
            let sparse_repo = expand_tilde(sparse_repo)?;

            let dense_repo = git_helper::find_top_level(cloned_app.clone(), &dense_repo)
                .context("Failed to canonicalize dense repo path")?;

            let layers = filter_empty_strings(layers.0);
            let coordinates = filter_empty_strings(coordinates.0);

            if !(coordinates.is_empty() ^ layers.is_empty()) {
                bail!("Either layers or coordinates must be specified");
            }

            let spec = if !coordinates.is_empty() {
                sparse_repos::Spec::Coordinates(coordinates.to_vec())
            } else if !layers.is_empty() {
                sparse_repos::Spec::Layers(layers.to_vec())
            } else {
                unreachable!()
            };

            let ui = cloned_app.ui();
            let _ = ui.status(format!(
                "Cloning {} into {}",
                dense_repo.display(),
                sparse_repo.display()
            ));
            ui.set_enabled(interactive);

            subcommands::clone::run(
                &dense_repo,
                &sparse_repo,
                &branch,
                &spec,
                cloned_app.clone(),
            )
        }

        Subcommand::Sync { sparse_repo } => {
            let sparse_repo = expand_tilde(sparse_repo)?;
            app.ui().set_enabled(interactive);
            subcommands::sync::run(app, &sparse_repo)
        }

        Subcommand::AvailableLayers { repo } => {
            let repo = expand_tilde(repo)?;
            let repo = git_helper::find_top_level(app, &repo)
                .context("Failed to canonicalize repo path")?;
            subcommands::available_layers::run(&repo)
        }

        Subcommand::SelectedLayers { repo } => {
            let repo = expand_tilde(repo)?;
            let repo = git_helper::find_top_level(app, &repo)
                .context("Failed to canonicalize repo path")?;
            subcommands::selected_layers::run(&repo)
        }

        Subcommand::PushLayer { repo, names } => {
            let repo = expand_tilde(repo)?;
            let repo = git_helper::find_top_level(app, &repo)
                .context("Failed to canonicalize repo path")?;
            subcommands::push_layer::run(&repo, names)
        }

        Subcommand::PopLayer { repo, count } => {
            let repo = expand_tilde(repo)?;
            let repo = git_helper::find_top_level(app, &repo)
                .context("Failed to canonicalize repo path")?;
            subcommands::pop_layer::run(&repo, count)
        }

        Subcommand::RemoveLayer { repo, names } => {
            let repo = expand_tilde(repo)?;
            let repo = git_helper::find_top_level(app, &repo)
                .context("Failed to canonicalize repo path")?;
            subcommands::remove_layer::run(&repo, names)
        }

        Subcommand::ListRepos {} => subcommands::list_repos::run(),

        Subcommand::DetectBuildGraphChanges { repo } => {
            let repo = expand_tilde(repo)?;
            let repo = git_helper::find_top_level(app.clone(), &repo)
                .context("Failed to canonicalize repo path")?;
            subcommands::detect_build_graph_changes::run(app, &repo)
        }

        Subcommand::UserInterfaceTest {} => {
            let ui = cloned_app.ui();
            let _ = ui.status(format!("UI Test"));
            ui.set_enabled(interactive);
            subcommands::user_interface_test::run(app)
        }
    }
}

fn main() -> Result<()> {
    let started_at = Instant::now();
    let options = ParachuteOpts::from_args();

    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let interactive = if options.ugly {
        false
    } else {
        termion::is_tty(&std::io::stdout())
    };

    ensure_directories_exist().context("Failed to create necessary directories")?;
    let app = Arc::from(App::new(options.preserve_sandbox, interactive)?);
    run_subcommand(app, options, interactive)?;

    let total_runtime = started_at.elapsed();
    log::info!("Finished normally in {:.2}s", total_runtime.as_secs_f32());

    Ok(())
}
