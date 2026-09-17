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
use crate::notes::{Note, Notes};
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

/// A run claimed by an experiment cell: who ran it, and where that cell
/// lives on disk.
#[derive(Debug)]
struct Claim {
    origin: Origin,
    /// `experiments/<dir>/results/<arm>/seed-<s>/`
    dir: PathBuf,
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
    /// The experiment's `name:`, or the directory name when the file could
    /// not be loaded.
    pub name: String,
    pub question: String,
    pub arm_count: usize,
    pub cells: Vec<CellCard>,
    pub check: CheckBadge,
    /// Why this card is empty. One unreadable `experiment.yaml` must not
    /// take the feed down with it, so the failure is rendered on its own
    /// card and every other card still gathers.
    pub error: Option<String>,
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

/// A reproduction of a run: what `retrograde reproduce <run-id>` left in
/// `<workspace>/reproductions/<run-id>/`.
#[derive(Debug, Clone)]
pub struct Reproduction {
    /// `started_at` of the new run's manifest — when the re-run began.
    pub at: String,
    /// The new run's id, which is a run of this workspace like any other.
    pub run_id: String,
    /// The report's last non-empty line: `REPRODUCED`, `NOT REPRODUCED`,
    /// `DRIFT`, or the dry-run line. Verbatim, so a wording `view` has
    /// never heard of still reaches the page.
    pub verdict: String,
    /// The whole `report.txt`, metric table included.
    pub report: String,
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
    /// This run's notes, newest first.
    pub notes: Vec<Note>,
    /// Why there are none. The notes database is the one thing `view`
    /// writes, and a repo it cannot write to must still browse: the card
    /// carries the failure instead of the page returning 500.
    pub notes_error: Option<String>,
    /// The reproduction of this run, when one has been attempted. `None`
    /// renders no card at all.
    pub reproduction: Option<Reproduction>,
}

/// The feed: every experiment, then every run in the workspace.
pub fn feed(root: &Path, workspace: &Path) -> Result<Feed, Error> {
    let experiments = experiment_cards(root);
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
    // Opening the notes database creates it if it is not there; a root
    // that refuses is reported on the card, not on the whole page.
    let (notes, notes_error) = match Notes::open(root).and_then(|n| n.list(run_id)) {
        Ok(notes) => (notes, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };

    Ok(Some(Profile {
        run_id: run_id.to_string(),
        origin: claims(root)
            .into_iter()
            .find_map(|(id, c)| (id == run_id).then_some(c.origin)),
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
        notes,
        notes_error,
        reproduction: reproduction(workspace, run_id),
    }))
}

/// What `retrograde reproduce <run_id>` wrote, if anything. The directory
/// existing is what puts the card on the page; the files inside it are read
/// best-effort, so a reproduction that died before writing its report still
/// says that it was attempted.
fn reproduction(workspace: &Path, run_id: &str) -> Option<Reproduction> {
    let dir = crate::reproduce::reproduction_dir(workspace, run_id);
    if !dir.is_dir() {
        return None;
    }
    let report = fs::read_to_string(dir.join("report.txt")).unwrap_or_default();
    let manifest = crate::read_json(&dir.join("manifest.json")).unwrap_or(serde_json::Value::Null);
    Some(Reproduction {
        at: string_at(&manifest, "started_at"),
        run_id: string_at(&manifest, "run_id"),
        verdict: verdict(&report),
        report,
    })
}

/// The report's verdict: its last non-empty line, trimmed. `reproduce`
/// writes the verdict last, whatever the verdict is.
fn verdict(report: &str) -> String {
    report
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `<workspace>/runs/<id>`. The caller has already checked `id` is a single
/// path component (see `view::safe_component`).
pub fn run_dir(workspace: &Path, run_id: &str) -> PathBuf {
    workspace.join("runs").join(run_id)
}

/// Every `experiments/*/experiment.yaml` under `root`, in directory order.
/// A directory whose experiment could not be read becomes a card carrying
/// the error instead of one, so the rest of the feed still renders.
fn experiment_cards(root: &Path) -> Vec<ExperimentCard> {
    experiment_dirs(root)
        .into_iter()
        .map(|dir| {
            card_for(&dir).unwrap_or_else(|error| ExperimentCard {
                name: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                question: String::new(),
                arm_count: 0,
                cells: Vec::new(),
                check: CheckBadge::None,
                error: Some(error.to_string()),
            })
        })
        .collect()
}

fn card_for(dir: &Path) -> Result<ExperimentCard, Error> {
    let path = dir.join("experiment.yaml");
    let experiment = Experiment::load(&path)?;
    let mut cells = Vec::new();
    for cell in crate::cell::cells(dir, &experiment)? {
        let status = Status::read(&cell.status_path())?;
        let manifest = cell.manifest_path();
        // `Row::with_manifest` is the crate's manifest-metric reader;
        // its first two columns are median NSE and median KGE.
        let row = Row::empty(
            &cell.arm,
            cell.seed,
            status.status,
            status.run_id.as_deref(),
        );
        let row = if status.status == CellStatus::Done && manifest.is_file() {
            row.with_manifest(&crate::read_json(&manifest)?)
        } else {
            row
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

    Ok(ExperimentCard {
        name: experiment.name.clone(),
        question: experiment.question.trim().to_string(),
        arm_count: experiment.arms.len(),
        cells,
        check: badge,
        error: None,
    })
}

fn experiment_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root.join("experiments")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("experiment.yaml").is_file())
        .filter(|p| !is_template(p))
        .collect();
    dirs.sort();
    dirs
}

/// `experiments/_template/` — and anything else whose directory name starts
/// with `_` — is the shape of an experiment, not one. `experiments/README.md`
/// is where that convention lives.
fn is_template(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('_'))
        .unwrap_or(false)
}

/// `run_id -> Claim` for every cell of every experiment that recorded one.
/// Best-effort: an experiment that cannot be read attributes nothing rather
/// than failing the profile page that asked.
fn claims(root: &Path) -> Vec<(String, Claim)> {
    let mut out = Vec::new();
    for dir in experiment_dirs(root) {
        let Ok(experiment) = Experiment::load(&dir.join("experiment.yaml")) else {
            continue;
        };
        let Ok(cells) = crate::cell::cells(&dir, &experiment) else {
            continue;
        };
        for cell in cells {
            let Ok(status) = Status::read(&cell.status_path()) else {
                continue;
            };
            if let Some(id) = status.run_id {
                out.push((
                    id,
                    Claim {
                        origin: Origin {
                            experiment: experiment.name.clone(),
                            arm: cell.arm.clone(),
                            seed: cell.seed,
                        },
                        dir: cell.dir.clone(),
                    },
                ));
            }
        }
    }
    out
}

/// The cell directory that claims `run_id`, if one does — the directory,
/// not the experiment's `name:`, because the two need not agree and only
/// the directory exists on disk. `notes export` writes into it.
pub fn claiming_cell(root: &Path, run_id: &str) -> Option<PathBuf> {
    claims(root)
        .into_iter()
        .find_map(|(id, claim)| (id == run_id).then_some(claim.dir))
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
