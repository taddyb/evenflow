//! `retrograde plot <run-id>`: draw a run's hydrographs and metrics.
//!
//! The plotting itself is Python — matplotlib, numpy and a zarr reader —
//! living in the uv project at `crates/retrograde/py/`. This module is the
//! thin end: it turns a run id into the run directory (the same
//! `<workspace>/runs/<id>` [`crate::view`] serves a profile page out of)
//! and hands it to
//!
//! ```text
//! uv run --project <root>/crates/retrograde/py \
//!     python <root>/crates/retrograde/py/plot_run.py <run_dir>
//! ```
//!
//! uv resolves and syncs the project's locked environment itself, so there
//! is nothing to install first. The child inherits stdout and stderr — its
//! progress is the command's output — and its exit code is the command's
//! exit code.
//!
//! Nothing here writes: `plot_run.py` writes `<run_dir>/plots/*.png` and
//! nothing else, which is exactly what the profile page lists.

use std::path::PathBuf;
use std::process::Command;

use crate::view::data::run_dir;
use crate::Error;

#[derive(Debug, Clone)]
pub struct PlotOptions {
    /// The run to plot: a `<workspace>/runs/<id>` directory name.
    pub run_id: String,
    /// Workspace root holding `crates/retrograde/py` (default: nearest
    /// `Cargo.toml` with a `[workspace]` table, from the current directory).
    pub root: Option<PathBuf>,
    /// ddrs workspace holding `runs/` (default: `<root>/crates/ddrs/.ddrs`).
    pub workspace: Option<PathBuf>,
}

/// Run the plotting script over one run, returning its exit code.
///
/// An `Err` means the plot never started — no such run, no script, no `uv`.
/// A script that started and failed comes back as `Ok(non-zero)`, because
/// it has already said why on stderr.
pub fn plot(opts: &PlotOptions) -> Result<i32, Error> {
    // A run id becomes a path component, so it has to be one. `plot ..`
    // must not walk out of runs/ and plot whatever it lands on.
    if opts.run_id.is_empty()
        || opts.run_id == "."
        || opts.run_id == ".."
        || opts.run_id.contains(['/', '\\', '\0'])
    {
        return Err(Error::Invalid(format!(
            "run id {:?} is not one path component — \
             a run id names a directory under <workspace>/runs/",
            opts.run_id
        )));
    }

    let (root, workspace) = resolve(opts)?;
    let project = root.join("crates/retrograde/py");
    let script = project.join("plot_run.py");
    if !script.is_file() {
        return Err(Error::Invalid(format!(
            "no plot_run.py at {} — is --root the evenflow workspace?",
            script.display()
        )));
    }

    let dir = run_dir(&workspace, &opts.run_id);
    if !dir.is_dir() {
        return Err(Error::Invalid(format!(
            "no run {} in {} — `retrograde view` lists the runs there are",
            opts.run_id,
            workspace.join("runs").display()
        )));
    }
    let dir = crate::canonical(&dir)?;

    // Inherited stdio: uv's resolution notes and the script's own output
    // are this command's output, live, not something buffered and echoed.
    let status = Command::new("uv")
        .arg("run")
        .arg("--project")
        .arg(&project)
        .arg("python")
        .arg(&script)
        .arg(&dir)
        .status()
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => Error::Invalid(
                "no `uv` on PATH — retrograde plots with the uv project at \
                 crates/retrograde/py; install uv (https://docs.astral.sh/uv/)"
                    .to_string(),
            ),
            _ => Error::Spawn {
                bin: PathBuf::from("uv"),
                source,
            },
        })?;

    // A child killed by a signal has no code; 130 is the shell's Ctrl-C.
    Ok(status.code().unwrap_or(130))
}

/// `--root` / `--workspace`, or the same defaults `sweep` and `view` use.
fn resolve(opts: &PlotOptions) -> Result<(PathBuf, PathBuf), Error> {
    let root = match &opts.root {
        Some(r) => crate::canonical(r)?,
        None => {
            let cwd = std::env::current_dir().map_err(|source| Error::Io {
                path: PathBuf::from("."),
                source,
            })?;
            crate::find_workspace_root(&cwd)?
        }
    };
    let workspace = match &opts.workspace {
        Some(w) => w.clone(),
        None => root.join("crates/ddrs/.ddrs"),
    };
    Ok((root, workspace))
}
