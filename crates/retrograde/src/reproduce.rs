//! `retrograde reproduce <target>`: re-run a past run from its record and
//! say whether the numbers came back.
//!
//! `sweep` re-runs an experiment and `check` judges it, but a run no
//! experiment claims has no path back: all that survives it is its record
//! directory — `manifest.json` and the `config.yaml` snapshot beside it.
//! That is enough. `reproduce` reads the record, says what code and what
//! data it was made with, re-runs the same config through the same
//! [`Ddrs`](crate::runner::Ddrs) invocation `sweep` uses, and compares the
//! new manifest's metrics to the recorded ones.
//!
//! Five sections, in order:
//!
//! 1. **record** — what is being reproduced. A record whose `status` is not
//!    `ok` is refused: a failed run makes no claim to reproduce.
//! 2. **code** — the record's ddrs sha against the `crates/ddrs` submodule
//!    HEAD now. A different commit is not an error; it changes the
//!    question, and the report says so.
//! 3. **sources** — ddrs is the authority on whether the data moved. The
//!    record's `config.yaml` is copied into the reproduction directory and
//!    `ddrs plan` is run against it; drift stops the reproduction (exit 4)
//!    unless `--allow-drift`.
//! 4. **run** — `ddrs run --workflow <the record's> --strict`.
//! 5. **compare** — every metric in both manifests, within `--tolerance`.
//!
//! Everything written goes to `<workspace>/reproductions/<original run
//! id>/`: the copied `config.yaml`, the rendered `report.txt`, and the NEW
//! run's `manifest.json`. Nothing is ever written next to the record, so
//! reproducing a committed experiment cell leaves git untouched.
//!
//! ## Why `plan` and not `plan --strict`
//!
//! The brief for this verb called for `ddrs plan --strict`. ddrs has no
//! such flag: `--strict` is `run`'s (`ddrs::cli::plan::PlanInput::strict`
//! is set to `false` for every `Cmd::Plan`), and a plain `plan` *relocks*
//! `sources.lock` on drift after warning about it. So drift is read off the
//! `plan` invocation two ways — a non-zero exit (real ddrs exits 4,
//! `ExitCode::LockDrift`, when a strict caller aborts) **or** a stderr line
//! mentioning drift (what a non-strict `plan` prints before it relocks).
//! Either is drift, and ddrs's own lines are what the report prints. The
//! `--strict` that does reach ddrs is `run`'s, which `Ddrs::run` already
//! passes.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::check::{num, render_rows, TableRow, Verdict as MetricVerdict};
use crate::runner::{self, Ddrs};
use crate::Error;

#[derive(Debug, Clone)]
pub struct ReproduceOptions {
    /// A run id under `<workspace>/runs/`, or a path to a `manifest.json`
    /// (or the directory holding one).
    pub target: String,
    /// Workspace root (default: nearest `Cargo.toml` with `[workspace]`).
    pub root: Option<PathBuf>,
    /// ddrs binary (default: `<root>/target/release/ddrs`).
    pub ddrs: Option<PathBuf>,
    /// ddrs workspace (default: `<root>/crates/ddrs/.ddrs`).
    pub workspace: Option<PathBuf>,
    /// Passed through to `ddrs run` when given.
    pub backend: Option<String>,
    /// Absolute tolerance every compared metric is judged against.
    pub tolerance: f64,
    /// Reproduce even though the data sources moved.
    pub allow_drift: bool,
    /// Verify the record and the sources, then stop without running.
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every compared metric was within tolerance.
    Reproduced,
    /// At least one metric was outside tolerance, or was in one manifest
    /// and not the other.
    NotReproduced,
    /// The data sources moved and `--allow-drift` was not given.
    Drift,
    /// `--dry-run`: the record and the sources were verified, nothing ran.
    DryRun,
}

impl Verdict {
    /// The process exit code. Drift is ddrs's own `ExitCode::LockDrift`.
    pub fn exit_code(self) -> u8 {
        match self {
            Verdict::Reproduced | Verdict::DryRun => 0,
            Verdict::NotReproduced => 1,
            Verdict::Drift => 4,
        }
    }
}

#[derive(Debug)]
pub struct ReproduceOutcome {
    pub verdict: Verdict,
    /// Everything that was printed, which is also `report.txt`.
    pub report: String,
}

/// The report, printed as it is built. The sections are printed rather than
/// returned at the end because step 4 is a training run: a reader watching
/// the terminal has to know what is being reproduced before it starts.
#[derive(Default)]
struct Report {
    text: String,
}

impl Report {
    fn line(&mut self, line: &str) {
        println!("{line}");
        self.text.push_str(line);
        self.text.push('\n');
    }

    /// A block that already ends in a newline (a rendered table).
    fn block(&mut self, block: &str) {
        print!("{block}");
        self.text.push_str(block);
    }

    fn section(&mut self, name: &str) {
        self.line("");
        self.line(name);
    }

    fn field(&mut self, name: &str, value: &str) {
        self.line(&format!("  {name:<9} {value}"));
    }
}

pub fn reproduce(opts: &ReproduceOptions) -> Result<ReproduceOutcome, Error> {
    let (root, workspace) = resolve(opts)?;
    let record_dir = resolve_target(&opts.target, &workspace)?;
    let manifest = crate::read_json(&record_dir.join("manifest.json"))?;

    let mut report = Report::default();

    // 1. Record -------------------------------------------------------
    let run_id = record_run_id(&manifest, &record_dir)?;
    let workflow = string_at(&manifest, "workflow");
    if workflow.is_empty() {
        return Err(Error::Invalid(format!(
            "{}: no workflow in the manifest — there is nothing to re-run",
            record_dir.join("manifest.json").display()
        )));
    }
    let git = manifest.get("git").cloned().unwrap_or(Value::Null);
    let recorded_sha = string_at(&git, "sha");
    let dirty = git.get("dirty").and_then(Value::as_bool).unwrap_or(false);
    let status = string_at(&manifest, "status");

    report.line("record");
    report.field("run id", &run_id);
    report.field("record", &record_dir.display().to_string());
    report.field("workflow", &workflow);
    report.field("ddrs sha", &recorded_sha);
    report.field("branch", &string_at(&git, "branch"));
    report.field("dirty", if dirty { "true" } else { "false" });
    report.field("started", &string_at(&manifest, "started_at"));
    report.field("status", &status);

    if status != "ok" {
        return Err(Error::Invalid(format!(
            "run {run_id} has status {status:?}, not \"ok\" — a run that did \
             not finish makes no claim to reproduce; pick a successful run"
        )));
    }

    // 2. Code ---------------------------------------------------------
    report.section("code");
    for line in code_lines(&root, &recorded_sha, dirty) {
        report.line(&format!("  {line}"));
    }

    // 3. Sources ------------------------------------------------------
    // The record's config is the thing that gets re-run, and ddrs is the
    // authority on whether the data underneath it moved.
    let repro_dir = reproduction_dir(&workspace, &run_id);
    let config = repro_dir.join("config.yaml");
    let record_config = record_dir.join("config.yaml");
    if !record_config.is_file() {
        return Err(Error::Invalid(format!(
            "{} has no config.yaml — the record does not carry the config \
             that produced it, so there is nothing to re-run",
            record_dir.display()
        )));
    }
    crate::copy_file(&record_config, &config)?;

    let ddrs = Ddrs {
        bin: ddrs_bin(opts, &root)?,
        cwd: root.join("crates/ddrs"),
        workspace: workspace.clone(),
        backend: opts.backend.clone(),
        workflow: workflow.clone(),
    };

    report.section("sources");
    let plan = ddrs.plan(&config)?;
    let drift = drift_lines(&plan);
    let drifted = !drift.is_empty();
    if drifted {
        for line in &drift {
            report.line(&format!("  {line}"));
        }
    } else {
        report.line("  no drift: ddrs plan locked the same sources");
    }

    if drifted && !opts.allow_drift {
        report.line("");
        report.line(
            "the data sources moved since this run — reproducing against \
             different data answers a different question; pass --allow-drift \
             to do it anyway",
        );
        report.line("DRIFT");
        return finish(Verdict::Drift, report, &repro_dir);
    }
    if drifted {
        report.line("  DRIFT ALLOWED");
    }

    if opts.dry_run {
        report.section("DRY RUN: record and sources verified, nothing run");
        return finish(Verdict::DryRun, report, &repro_dir);
    }

    // 4. Run ----------------------------------------------------------
    report.section("run");
    report.field("config", &config.display().to_string());
    report.field("workflow", &workflow);
    let launched = SystemTime::now();
    let outcome = ddrs.run(&config)?;
    let new_dir = outcome
        .run_dir
        .clone()
        .or_else(|| runner::newest_run_dir_since(&workspace, launched));

    let Some(new_dir) = new_dir.filter(|d| d.join("manifest.json").is_file()) else {
        finish(Verdict::NotReproduced, report, &repro_dir)?;
        return Err(Error::Invalid(format!(
            "ddrs run exited {} and left no manifest to compare{}",
            outcome.code.map(|c| c.to_string()).unwrap_or_else(|| "on a signal".into()),
            outcome
                .last_stderr
                .map(|l| format!(" — last line: {l}"))
                .unwrap_or_default(),
        )));
    };
    let new_manifest_path = repro_dir.join("manifest.json");
    crate::copy_file(&new_dir.join("manifest.json"), &new_manifest_path)?;
    let new_manifest = crate::read_json(&new_manifest_path)?;
    report.field("new run", &string_at(&new_manifest, "run_id"));
    report.field("status", &string_at(&new_manifest, "status"));

    // 5. Compare ------------------------------------------------------
    report.section("compare");
    let (rows, reproduced) = compare(&manifest, &new_manifest, opts.tolerance);
    report.block(&render_rows(&rows));
    let verdict = if reproduced {
        Verdict::Reproduced
    } else {
        Verdict::NotReproduced
    };
    report.line(if reproduced {
        "REPRODUCED"
    } else {
        "NOT REPRODUCED"
    });
    finish(verdict, report, &repro_dir)
}

/// `<workspace>/reproductions/<original run id>/`: where a reproduction's
/// copied `config.yaml`, its `report.txt` and the new run's `manifest.json`
/// are written. `view`'s profile page reads the same directory, so the
/// layout is spelled once.
pub(crate) fn reproduction_dir(workspace: &Path, original_run_id: &str) -> PathBuf {
    workspace.join("reproductions").join(original_run_id)
}

/// Write `report.txt` beside the copied config and hand back the outcome.
fn finish(verdict: Verdict, report: Report, repro_dir: &Path) -> Result<ReproduceOutcome, Error> {
    crate::write_file(&repro_dir.join("report.txt"), &report.text)?;
    Ok(ReproduceOutcome {
        verdict,
        report: report.text,
    })
}

/// `--root` / `--workspace`, or the same defaults every other verb uses.
fn resolve(opts: &ReproduceOptions) -> Result<(PathBuf, PathBuf), Error> {
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

fn ddrs_bin(opts: &ReproduceOptions, root: &Path) -> Result<PathBuf, Error> {
    let bin = match &opts.ddrs {
        Some(b) => b.clone(),
        None => root.join("target/release/ddrs"),
    };
    if !bin.is_file() {
        return Err(Error::Invalid(format!(
            "no ddrs binary at {} — build it with `cargo build --release -p ddrs --bin ddrs`",
            bin.display()
        )));
    }
    Ok(bin)
}

/// The record directory: a path to a `manifest.json` (or to the directory
/// holding one), or a run id under `<workspace>/runs/`. Both forms land on
/// a directory holding `manifest.json` and `config.yaml`.
fn resolve_target(target: &str, workspace: &Path) -> Result<PathBuf, Error> {
    let as_path = Path::new(target);
    let dir = if as_path.is_file() {
        as_path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else if as_path.is_dir() {
        as_path.to_path_buf()
    } else {
        // A run id is one path component: `reproduce ..` must not walk out
        // of runs/ and re-run whatever it lands on.
        if target.is_empty() || target.contains(['/', '\\', '\0']) || target.starts_with('.') {
            return Err(Error::Invalid(format!(
                "{target:?} is neither a path to a manifest.json nor a run id \
                 (a run id names one directory under <workspace>/runs/)"
            )));
        }
        crate::view::data::run_dir(workspace, target)
    };

    if !dir.join("manifest.json").is_file() {
        return Err(Error::Invalid(format!(
            "no manifest.json in {} — `reproduce` takes a run id or the path \
             to a record's manifest.json; `retrograde view` lists the runs \
             there are",
            dir.display()
        )));
    }
    crate::canonical(&dir)
}

/// The run id the reproduction is filed under. The manifest's `run_id` when
/// it has one — a committed experiment cell carries the id of the run it
/// copied — and the directory name otherwise.
fn record_run_id(manifest: &Value, record_dir: &Path) -> Result<String, Error> {
    let id = string_at(manifest, "run_id");
    if !id.is_empty() {
        return Ok(id);
    }
    record_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{}: no run_id in the manifest and no directory to name it after",
                record_dir.display()
            ))
        })
}

/// The `code` section's lines: the record's sha against the `crates/ddrs`
/// submodule HEAD, plus the warning a dirty record earns.
fn code_lines(root: &Path, recorded_sha: &str, dirty: bool) -> Vec<String> {
    let mut lines = Vec::new();
    match submodule_head(root) {
        None => lines.push(format!(
            "current commit unknown: `git -C {} rev-parse HEAD` did not answer",
            root.join("crates/ddrs").display()
        )),
        Some(now) if now == recorded_sha => lines.push(format!("same commit {}", short(&now))),
        Some(now) => {
            lines.push(format!(
                "different commit: record {} vs now {}",
                short(recorded_sha),
                short(&now)
            ));
            lines.push(
                "this reproduction tests whether the claim survives the code \
                 change, not whether the original run was correct"
                    .to_string(),
            );
        }
    }
    if dirty {
        lines.push(
            "warning: the original run had uncommitted changes, so it cannot \
             be reproduced exactly"
                .to_string(),
        );
    }
    lines
}

/// `git -C <root>/crates/ddrs rev-parse HEAD`. `None` when git is not there
/// or the submodule is not checked out: an unknown current commit is worth
/// saying, not worth stopping for.
fn submodule_head(root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root.join("crates/ddrs"))
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// ddrs's own account of the drift, empty when there was none. See the
/// module docs for why both signals are read.
fn drift_lines(plan: &crate::runner::Outcome) -> Vec<String> {
    let mentions: Vec<String> = plan
        .stderr
        .iter()
        .filter(|l| l.to_lowercase().contains("drift"))
        .map(|l| l.trim().to_string())
        .collect();
    if !mentions.is_empty() {
        return mentions;
    }
    if plan.ok() {
        return Vec::new();
    }
    // A non-zero plan that never said "drift" still stopped the
    // reproduction; its last line is the only account there is.
    let mut lines = vec![format!(
        "ddrs plan exited {}",
        plan.code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "on a signal".to_string())
    )];
    lines.extend(plan.last_stderr.clone());
    lines
}

/// Every metric in either manifest, compared where both carry a number.
/// Returns the rendered rows and whether every one of them passed.
fn compare(recorded: &Value, actual: &Value, tolerance: f64) -> (Vec<TableRow>, bool) {
    let want = numbers(recorded);
    let got = numbers(actual);

    // serde_json objects are sorted maps, so the key order is stable and the
    // union below is too.
    let mut keys: Vec<&String> = want.keys().chain(got.keys()).collect();
    keys.sort();
    keys.dedup();

    let mut rows = Vec::new();
    let mut passed = true;
    for key in keys {
        let (a, b) = (want.get(key), got.get(key));
        let (verdict, diff, reason) = match (a, b) {
            (Some(a), Some(b)) if wall_clock(key) => (
                MetricVerdict::Skip,
                Some((b - a).abs()),
                Some("wall clock, not judged".to_string()),
            ),
            (Some(a), Some(b)) => {
                let diff = (b - a).abs();
                if diff <= tolerance {
                    (MetricVerdict::Pass, Some(diff), None)
                } else {
                    (
                        MetricVerdict::Fail,
                        Some(diff),
                        Some(format!("outside tolerance {}", num(tolerance))),
                    )
                }
            }
            (Some(_), None) => (
                MetricVerdict::Missing,
                None,
                Some("not a number in the new run's manifest".to_string()),
            ),
            (None, Some(_)) => (
                MetricVerdict::Missing,
                None,
                Some("not a number in the record's manifest".to_string()),
            ),
            (None, None) => unreachable!("the key came from one of the two maps"),
        };
        passed &= verdict != MetricVerdict::Fail && verdict != MetricVerdict::Missing;
        rows.push(TableRow {
            cells: vec![
                key.to_string(),
                a.map(|v| num(*v)).unwrap_or_else(|| "-".to_string()),
                b.map(|v| num(*v)).unwrap_or_else(|| "-".to_string()),
                diff.map(num).unwrap_or_else(|| "-".to_string()),
                verdict.as_str().to_string(),
            ],
            reason,
        });
    }

    if rows.is_empty() {
        rows.push(TableRow {
            cells: vec!["-".to_string(); 5],
            reason: Some("neither manifest carries any numeric metric".to_string()),
        });
        passed = false;
    }
    (rows, passed)
}

/// How long a run took is a measurement of the machine, not of the model:
/// ddrs records `phase1_seconds` and `phase2_seconds` alongside the real
/// results, and on the Juniata record they differ by 0.02 s and 0.24 s
/// between two runs whose every scientific metric is bit-identical. Judged
/// against any tolerance tight enough to mean something for NSE, they would
/// make every reproduction fail for no reason. They are still printed, with
/// their difference — that is worth seeing — but they do not vote.
///
/// `_seconds` is ddrs's whole wall-clock vocabulary (`cli/run.rs`); every
/// other metric key it writes is a result.
fn wall_clock(key: &str) -> bool {
    key.ends_with("_seconds")
}

/// A manifest's `metrics`, keeping only the keys whose value is a number —
/// the only ones a tolerance means anything for.
fn numbers(manifest: &Value) -> std::collections::BTreeMap<String, f64> {
    manifest
        .get("metrics")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n)))
                .collect()
        })
        .unwrap_or_default()
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
