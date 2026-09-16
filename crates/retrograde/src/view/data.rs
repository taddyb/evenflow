//! What the pages show, gathered from disk into plain structs.
//!
//! Nothing here knows about HTTP: every function takes the two directories
//! `view` was pointed at and returns data, so the feed and the profile can
//! be unit-tested without a server. The files are read through the readers
//! the other verbs already use — [`Experiment`], [`crate::cell::cells`],
//! [`Status`], [`Row`], and [`check`] — so a format only ever has one
//! parser in this crate.

use std::fs;
use std::path::{Path, PathBuf};

use crate::cell::{CellStatus, Status};
use crate::check::{check, CheckOptions};
use crate::experiment::Experiment;
use crate::summary::Row;
use crate::Error;

/// How many trailing lines of `run.log` the profile inlines. The whole file
/// is a click away at `/run/<id>/log`.
pub const LOG_TAIL_LINES: usize = 200;

/// Which experiment cell, if any, produced a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub experiment: String,
    pub arm: String,
    pub seed: i64,
}

/// `check`'s verdict for an experiment, or `None` when `expected:` is empty
/// and there is nothing to check against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckBadge {
    Pass,
    Fail,
    None,
}

#[derive(Debug, Clone)]
pub struct CellCard {
    pub arm: String,
    pub seed: i64,
    pub status: CellStatus,
    pub run_id: Option<String>,
    /// `median_nse_finite` / `median_kge_finite`, rendered, when the cell is
    /// `done` and carries a manifest.
    pub median_nse: Option<String>,
    pub median_kge: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExperimentCard {
    pub name: String,
    pub question: String,
    pub arm_count: usize,
    pub cells: Vec<CellCard>,
    pub check: CheckBadge,
}

#[derive(Debug, Clone)]
pub struct RunRow {
    pub run_id: String,
    pub status: String,
    pub workflow: String,
    pub started: String,
    pub duration: String,
    pub median_nse: Option<String>,
    pub median_kge: Option<String>,
    /// First 7 characters of the manifest's `git.sha`.
    pub ddrs_sha: String,
    /// `finished_at` is null: the run never stamped an end.
    pub in_progress: bool,
    pub origin: Option<Origin>,
}

#[derive(Debug, Clone)]
pub struct Feed {
    pub experiments: Vec<ExperimentCard>,
    pub runs: Vec<RunRow>,
}

/// A source's fingerprint against the workspace's current `sources.lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift {
    /// The lock carries the same `fp`.
    Same,
    /// The lock carries a different `fp`: the data moved under the run.
    Drift,
    /// The lock does not carry this source at all (or there is no lock).
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SourceRow {
    pub name: String,
    pub path: String,
    /// The leading characters of `fp`, enough to tell two apart by eye.
    pub fingerprint: String,
    pub drift: Drift,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub run_id: String,
    pub origin: Option<Origin>,
    pub status: String,
    pub workflow: String,
    pub ddrs_sha: String,
    pub branch: String,
    pub dirty: bool,
    pub started: String,
    pub finished: String,
    pub duration: String,
    /// Every key of `manifest.metrics`, by key (serde_json maps are sorted).
    pub metrics: Vec<(String, String)>,
    pub sources: Vec<SourceRow>,
    /// The `config.yaml` snapshot beside the manifest.
    pub config: Option<String>,
    /// File names under `<run_dir>/plots/`, sorted.
    pub plots: Vec<String>,
    /// Last [`LOG_TAIL_LINES`] lines of `run.log`; `None` when there is none.
    pub log_tail: Option<String>,
}

/// The feed: every experiment, then every run in the workspace.
pub fn feed(root: &Path, workspace: &Path) -> Result<Feed, Error> {
    let experiments = experiment_cards(root)?;
    let runs = run_rows(workspace, &experiments)?;
    Ok(Feed { experiments, runs })
}

/// One run's profile. `None` when the workspace has no such run — the
/// caller turns that into a 404.
pub fn profile(root: &Path, workspace: &Path, run_id: &str) -> Result<Option<Profile>, Error> {
    let dir = run_dir(workspace, run_id);
    let manifest_path = dir.join("manifest.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let manifest = crate::read_json(&manifest_path)?;
    let lock = read_lock(workspace);

    Ok(Some(Profile {
        run_id: run_id.to_string(),
        origin: origins(root)?
            .into_iter()
            .find_map(|(id, o)| (id == run_id).then_some(o)),
        status: string_at(&manifest, "status"),
        workflow: string_at(&manifest, "workflow"),
        ddrs_sha: manifest
            .get("git")
            .map(|g| string_at(g, "sha"))
            .unwrap_or_default(),
        branch: manifest
            .get("git")
            .map(|g| string_at(g, "branch"))
            .unwrap_or_default(),
        dirty: manifest
            .get("git")
            .and_then(|g| g.get("dirty"))
            .and_then(|d| d.as_bool())
            .unwrap_or(false),
        started: string_at(&manifest, "started_at"),
        finished: string_at(&manifest, "finished_at"),
        duration: duration(&manifest),
        metrics: metrics(&manifest),
        sources: sources(&manifest, lock.as_ref()),
        config: fs::read_to_string(dir.join("config.yaml")).ok(),
        plots: plots(&dir),
        log_tail: log_tail(&dir),
    }))
}

/// `<workspace>/runs/<id>`. The caller has already checked `id` is a single
/// path component (see `view::safe_component`).
pub fn run_dir(workspace: &Path, run_id: &str) -> PathBuf {
    workspace.join("runs").join(run_id)
}

/// Every `experiments/*/experiment.yaml` under `root`, in directory order.
fn experiment_cards(root: &Path) -> Result<Vec<ExperimentCard>, Error> {
    let mut cards = Vec::new();
    for dir in experiment_dirs(root) {
        let path = dir.join("experiment.yaml");
        let experiment = Experiment::load(&path)?;
        let mut cells = Vec::new();
        for cell in crate::cell::cells(&dir, &experiment)? {
            let status = Status::read(&cell.status_path())?;
            let manifest = cell.manifest_path();
            // `Row::with_manifest` is the crate's manifest-metric reader;
            // its first two columns are median NSE and median KGE.
            let row = if status.status == CellStatus::Done && manifest.is_file() {
                Row::empty(
                    &cell.arm,
                    cell.seed,
                    status.status,
                    status.run_id.as_deref(),
                )
                .with_manifest(&crate::read_json(&manifest)?)
            } else {
                Row::empty(
                    &cell.arm,
                    cell.seed,
                    status.status,
                    status.run_id.as_deref(),
                )
            };
            cells.push(CellCard {
                arm: cell.arm.clone(),
                seed: cell.seed,
                status: status.status,
                run_id: status.run_id.clone(),
                median_nse: short(&row.metrics[0]),
                median_kge: short(&row.metrics[1]),
            });
        }

        // `check` is only meaningful once the experiment claims something.
        let badge = if experiment.expected.is_empty() {
            CheckBadge::None
        } else if check(&CheckOptions {
            experiment: path.clone(),
        })?
        .passed
        {
            CheckBadge::Pass
        } else {
            CheckBadge::Fail
        };

        cards.push(ExperimentCard {
            name: experiment.name.clone(),
            question: experiment.question.trim().to_string(),
            arm_count: experiment.arms.len(),
            cells,
            check: badge,
        });
    }
    Ok(cards)
}

fn experiment_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root.join("experiments")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("experiment.yaml").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// `run_id -> Origin` for every cell of every experiment that recorded one.
fn origins(root: &Path) -> Result<Vec<(String, Origin)>, Error> {
    let mut out = Vec::new();
    for dir in experiment_dirs(root) {
        let experiment = Experiment::load(&dir.join("experiment.yaml"))?;
        for cell in crate::cell::cells(&dir, &experiment)? {
            let status = Status::read(&cell.status_path())?;
            if let Some(id) = status.run_id {
                out.push((
                    id,
                    Origin {
                        experiment: experiment.name.clone(),
                        arm: cell.arm.clone(),
                        seed: cell.seed,
                    },
                ));
            }
        }
    }
    Ok(out)
}

/// Every `<workspace>/runs/*/manifest.json`, newest `started_at` first.
fn run_rows(workspace: &Path, experiments: &[ExperimentCard]) -> Result<Vec<RunRow>, Error> {
    let Ok(entries) = fs::read_dir(workspace.join("runs")) else {
        return Ok(Vec::new());
    };

    let mut rows = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = crate::read_json(&manifest_path)?;
        let run_id = match manifest.get("run_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => entry.file_name().to_string_lossy().into_owned(),
        };
        // Same reader as the summary table, for the same two metrics.
        let row = Row::empty("", 0, CellStatus::Done, None).with_manifest(&manifest);
        rows.push(RunRow {
            status: string_at(&manifest, "status"),
            workflow: string_at(&manifest, "workflow"),
            started: string_at(&manifest, "started_at"),
            duration: duration(&manifest),
            median_nse: short(&row.metrics[0]),
            median_kge: short(&row.metrics[1]),
            ddrs_sha: row.ddrs_sha.chars().take(7).collect(),
            in_progress: manifest
                .get("finished_at")
                .map(|f| f.is_null())
                .unwrap_or(true),
            origin: experiments.iter().find_map(|e| {
                e.cells
                    .iter()
                    .find(|c| c.run_id.as_deref() == Some(run_id.as_str()))
                    .map(|c| Origin {
                        experiment: e.name.clone(),
                        arm: c.arm.clone(),
                        seed: c.seed,
                    })
            }),
            run_id,
        });
    }
    // Newest first. `started_at` is RFC3339 with a fixed offset, so it
    // sorts lexicographically; the run id breaks ties (it is a timestamp).
    rows.sort_by(|a, b| (&b.started, &b.run_id).cmp(&(&a.started, &a.run_id)));
    Ok(rows)
}

/// `<workspace>/sources.lock`, or `None` when there is none to compare to.
fn read_lock(workspace: &Path) -> Option<serde_json::Value> {
    let path = workspace.join("sources.lock");
    if !path.is_file() {
        return None;
    }
    crate::read_json(&path).ok()
}

/// The manifest's sources, each judged against the lock's `fp` for the
/// same name. A name the lock does not carry is `Unknown`, never `Same`.
fn sources(manifest: &serde_json::Value, lock: Option<&serde_json::Value>) -> Vec<SourceRow> {
    let Some(map) = manifest.get("sources").and_then(|s| s.as_object()) else {
        return Vec::new();
    };
    map.iter()
        .map(|(name, fingerprint)| {
            let fp = string_at(fingerprint, "fp");
            let locked = lock
                .and_then(|l| l.get("sources"))
                .and_then(|s| s.get(name))
                .map(|f| string_at(f, "fp"));
            SourceRow {
                name: name.clone(),
                path: string_at(fingerprint, "path"),
                fingerprint: fp.chars().take(19).collect(),
                drift: match locked {
                    None => Drift::Unknown,
                    Some(l) if l == fp => Drift::Same,
                    Some(_) => Drift::Drift,
                },
            }
        })
        .collect()
}

/// Every key of `manifest.metrics`, rendered. `serde_json`'s object is a
/// `BTreeMap`, so the order is the key order.
fn metrics(manifest: &serde_json::Value) -> Vec<(String, String)> {
    let Some(map) = manifest.get("metrics").and_then(|m| m.as_object()) else {
        return Vec::new();
    };
    map.iter()
        .map(|(key, value)| {
            let rendered = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (key.clone(), rendered)
        })
        .collect()
}

fn plots(run_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(run_dir.join("plots")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".png"))
        .collect();
    names.sort();
    names
}

fn log_tail(run_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(run_dir.join("run.log")).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    Some(lines[start..].join("\n"))
}

fn string_at(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// `finished_at - started_at`, empty when either is missing or unparseable.
fn duration(manifest: &serde_json::Value) -> String {
    let started = string_at(manifest, "started_at");
    let finished = string_at(manifest, "finished_at");
    let (Ok(a), Ok(b)) = (
        chrono::DateTime::parse_from_rfc3339(&started),
        chrono::DateTime::parse_from_rfc3339(&finished),
    ) else {
        return String::new();
    };
    let secs = (b - a).num_seconds();
    if secs < 0 {
        return String::new();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// A summary metric field trimmed for a table cell: four decimals, or the
/// field as-is when it is not a float. Empty means the cell has no value.
fn short(field: &str) -> Option<String> {
    if field.is_empty() {
        return None;
    }
    match field.parse::<f64>() {
        Ok(v) if field.contains('.') => Some(format!("{v:.4}")),
        _ => Some(field.to_string()),
    }
}
