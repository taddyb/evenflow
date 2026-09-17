//! `retrograde` is the evenflow experiment operator.
//!
//! `sweep` turns `experiments/<name>/experiment.yaml` into runs: every
//! arm x seed is a **cell** under `results/<arm>/seed-<s>/` holding the
//! derived `config.yaml`, the `status.json` that says what happened, and a
//! copy of the ddrs run's `manifest.json`. `results/summary.csv` is the
//! table over all cells. The point is that a paper's experiment becomes a
//! committed directory instead of a shell script.
//!
//! retrograde writes only under `experiments/<name>/results/`, plus
//! `experiments/<name>/sources.lock`. It never rewrites `experiment.yaml`.
//! The one other thing it writes is `.retrograde/notes.sqlite`, which is
//! gitignored; [`notes::export`] is how a note gets from there into
//! `results/<arm>/seed-<s>/notes.md` and therefore into git.

pub mod cell;
pub mod check;
pub mod experiment;
pub mod notes;
pub mod plot;
pub mod reproduce;
pub mod runner;
pub mod summary;
pub mod view;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use crate::check::{check, CheckOptions, CheckOutcome};
pub use crate::plot::{plot, PlotOptions};
pub use crate::reproduce::{reproduce, ReproduceOptions, ReproduceOutcome};

use crate::cell::{Cell, CellStatus, Status};
use crate::experiment::Experiment;
use crate::runner::Ddrs;
use crate::summary::Row;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: {source}")]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("could not run {bin}: {source}")]
    Spawn {
        bin: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct SweepOptions {
    /// Path to `experiments/<name>/experiment.yaml`.
    pub experiment: PathBuf,
    /// Run only cells that are `failed` or `pending`.
    pub only_failed: bool,
    /// Workspace root (default: nearest `Cargo.toml` with `[workspace]`).
    pub root: Option<PathBuf>,
    /// ddrs binary (default: `<root>/target/release/ddrs`).
    pub ddrs: Option<PathBuf>,
    /// ddrs workspace (default: `<root>/crates/ddrs/.ddrs`).
    pub workspace: Option<PathBuf>,
    /// Passed through to `ddrs run` when given.
    pub backend: Option<String>,
}

#[derive(Debug)]
pub struct SweepOutcome {
    /// True when every cell is `done` — the caller's exit code.
    pub all_done: bool,
    /// `summary.csv`'s contents, for printing.
    pub table: String,
}

pub fn sweep(opts: &SweepOptions) -> Result<SweepOutcome, Error> {
    let exp_path = canonical(&opts.experiment)?;
    let exp_dir = exp_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let experiment = Experiment::load(&exp_path)?;

    let root = match &opts.root {
        Some(r) => canonical(r)?,
        None => find_workspace_root(&exp_dir)?,
    };
    let ddrs_bin = match &opts.ddrs {
        Some(b) => absolute(b)?,
        None => root.join("target/release/ddrs"),
    };
    if !ddrs_bin.is_file() {
        return Err(Error::Invalid(format!(
            "no ddrs binary at {} — build it with `cargo build --release -p ddrs --bin ddrs`",
            ddrs_bin.display()
        )));
    }
    let workspace = match &opts.workspace {
        Some(w) => absolute(w)?,
        None => root.join("crates/ddrs/.ddrs"),
    };
    let ddrs = Ddrs {
        bin: ddrs_bin,
        cwd: root.join("crates/ddrs"),
        workspace: workspace.clone(),
        backend: opts.backend.clone(),
        workflow: "train-and-test".to_string(),
    };

    // Cells, sorted by arm then seed — the order they run and the order
    // summary.csv lists them.
    let cells = cell::cells(&exp_dir, &experiment)?;

    let lock_dest = exp_dir.join("sources.lock");
    let mut ran_any = false;
    let mut rows = Vec::new();

    for cell in &cells {
        let mut status = Status::read(&cell.status_path())?;
        if status.status == CellStatus::Running {
            reconcile_running(cell, &mut status, &root)?;
        }

        let should_run = if opts.only_failed {
            matches!(status.status, CellStatus::Failed | CellStatus::Pending)
        } else {
            status.status != CellStatus::Done
        };

        if should_run {
            ran_any = true;
            status = run_cell(cell, &ddrs, &root)?;
        } else {
            println!("{} skipped, {}", cell.label(), status.status.as_str());
        }

        rows.push(row_for(cell, &status)?);
    }

    // Pin the lock AFTER the cells run. Every cell begins with `ddrs plan`,
    // which rewrites `<workspace>/sources.lock`, so the file that describes
    // what the arms actually read does not exist until they have run.
    // Copying it beforehand commits the PREVIOUS sweep's pins, which makes
    // the record claim data the arms never saw. A sweep that ran nothing
    // leaves the committed lock alone: it is still the original record.
    if ran_any {
        pin_sources_lock(&workspace, &lock_dest)?;
    }

    let table = summary::write(&exp_dir.join("results/summary.csv"), &rows)?;
    Ok(SweepOutcome {
        all_done: rows.iter().all(|r| r.status == CellStatus::Done),
        table,
    })
}

/// Write `running`, run `plan` then `run`, then record what happened.
/// `run_dir` and `log` are recorded relative to `root` so a committed
/// `status.json` means the same thing on any clone.
fn run_cell(cell: &Cell, ddrs: &Ddrs, root: &Path) -> Result<Status, Error> {
    println!("{} running", cell.label());
    write_file(&cell.config_path(), &cell::derive_config(&cell.arm_config, cell.seed)?)?;

    let mut status = Status {
        status: CellStatus::Running,
        started_at: Some(now()),
        ..Status::default()
    };
    status.write(&cell.status_path())?;

    // Ctrl-C here leaves the cell `running`; the next sweep reconciles it.
    let launched = SystemTime::now();
    let plan = ddrs.plan(&cell.config_path())?;
    let outcome = if plan.ok() {
        ddrs.run(&cell.config_path())?
    } else {
        plan
    };

    let run_dir = outcome
        .run_dir
        .clone()
        .or_else(|| runner::newest_run_dir_since(&ddrs.workspace, launched));

    let mut manifest_ok = false;
    if let Some(dir) = &run_dir {
        let src = dir.join("manifest.json");
        if src.is_file() {
            copy_file(&src, &cell.manifest_path())?;
            let manifest = read_json(&cell.manifest_path())?;
            manifest_ok = manifest_status_ok(&manifest);
            if let Some(id) = manifest.get("run_id").and_then(|v| v.as_str()) {
                status.run_id = Some(id.to_string());
            }
        }
        if status.run_id.is_none() {
            status.run_id = dir.file_name().map(|n| n.to_string_lossy().into_owned());
        }
        status.log = Some(root_relative(&dir.join("run.log"), root));
    }

    let done = outcome.ok() && manifest_ok;
    status.status = if done { CellStatus::Done } else { CellStatus::Failed };
    status.run_dir = run_dir.map(|d| root_relative(&d, root));
    status.finished_at = Some(now());
    status.exit_code = outcome.code;
    status.error = if done { None } else { outcome.last_stderr };
    status.write(&cell.status_path())?;

    println!("{} {}", cell.label(), status.status.as_str());
    Ok(status)
}

/// A crash (or Ctrl-C) leaves `running` behind. If the run it points at
/// finished successfully, adopt it; otherwise the cell failed.
fn reconcile_running(cell: &Cell, status: &mut Status, root: &Path) -> Result<(), Error> {
    let manifest = status
        .run_dir
        .as_ref()
        .map(|d| resolve_from_root(d, root).join("manifest.json"))
        .filter(|m| m.is_file())
        .map(|m| read_json(&m).map(|v| (m, v)))
        .transpose()?
        .filter(|(_, v)| {
            // "finished" = ddrs got as far as stamping finished_at.
            v.get("finished_at").map(|f| !f.is_null()).unwrap_or(false)
                && manifest_status_ok(v)
        });

    match manifest {
        Some((path, value)) => {
            copy_file(&path, &cell.manifest_path())?;
            status.status = CellStatus::Done;
            if let Some(id) = value.get("run_id").and_then(|v| v.as_str()) {
                status.run_id = Some(id.to_string());
            }
            if let Some(at) = value.get("finished_at").and_then(|v| v.as_str()) {
                status.finished_at = Some(at.to_string());
            }
            status.error = None;
            println!("{} reconciled running -> done", cell.label());
        }
        None => {
            status.status = CellStatus::Failed;
            status.finished_at = Some(now());
            status.error = Some(
                "left `running` by an interrupted sweep; no finished run to adopt".to_string(),
            );
            println!("{} reconciled running -> failed", cell.label());
        }
    }
    status.write(&cell.status_path())
}

fn row_for(cell: &Cell, status: &Status) -> Result<Row, Error> {
    let row = Row::empty(&cell.arm, cell.seed, status.status, status.run_id.as_deref());
    if status.status != CellStatus::Done || !cell.manifest_path().is_file() {
        return Ok(row);
    }
    Ok(row.with_manifest(&read_json(&cell.manifest_path())?))
}

fn manifest_status_ok(manifest: &serde_json::Value) -> bool {
    manifest.get("status").and_then(|s| s.as_str()) == Some("ok")
}

/// Copy `<workspace>/sources.lock` beside experiment.yaml. Returns whether
/// there was a lock to copy.
fn pin_sources_lock(workspace: &Path, dest: &Path) -> Result<bool, Error> {
    let src = workspace.join("sources.lock");
    if !src.is_file() {
        return Ok(false);
    }
    copy_file(&src, dest)?;
    Ok(true)
}

/// The nearest ancestor holding a `Cargo.toml` with a `[workspace]` table.
pub(crate) fn find_workspace_root(start: &Path) -> Result<PathBuf, Error> {
    for dir in start.ancestors() {
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = fs::read_to_string(&manifest).map_err(|source| Error::Io {
            path: manifest.clone(),
            source,
        })?;
        if text.lines().any(|l| l.trim() == "[workspace]") {
            return Ok(dir.to_path_buf());
        }
    }
    Err(Error::Invalid(format!(
        "no Cargo.toml with a [workspace] table above {} — pass --root",
        start.display()
    )))
}

/// A path recorded in `status.json`. Paths under the workspace root are
/// stored relative to it (`crates/ddrs/.ddrs/runs/<id>`) so that a committed
/// `results/` tree is portable; a path outside the root — a `--workspace`
/// somewhere else — stays absolute, because nothing else would name it.
fn root_relative(path: &Path, root: &Path) -> PathBuf {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => path.to_path_buf(),
    }
}

/// The inverse of [`root_relative`]: read a recorded path back. Absolute
/// paths are used as they are, so both forms work.
fn resolve_from_root(path: &Path, root: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(crate) fn canonical(path: &Path) -> Result<PathBuf, Error> {
    fs::canonicalize(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn absolute(path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(cwd.join(path))
}

pub(crate) fn read_json(path: &Path) -> Result<serde_json::Value, Error> {
    let text = fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn create_parent(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

pub(crate) fn write_file(path: &Path, contents: &str) -> Result<(), Error> {
    create_parent(path)?;
    fs::write(path, contents).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn copy_file(src: &Path, dest: &Path) -> Result<(), Error> {
    create_parent(dest)?;
    fs::copy(src, dest).map_err(|source| Error::Io {
        path: src.to_path_buf(),
        source,
    })?;
    Ok(())
}
