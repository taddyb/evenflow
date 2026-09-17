//! `retrograde reproduce` driven against a fake `ddrs` binary.
//!
//! No test here launches the real ddrs. The fake is a POSIX shell script
//! written into a tempdir that accepts ddrs's argument shape, records every
//! invocation, writes `<workspace>/runs/<id>/manifest.json`, and prints the
//! same `run output -> <dir>` marker on stderr that `ddrs::cli::run::run`
//! does. Drift is simulated the way real ddrs signals it: a warning line on
//! stderr and exit 4 (`ddrs::cli::types::ExitCode::LockDrift`).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use retrograde::reproduce::{reproduce, ReproduceOptions, Verdict};

/// The fake ddrs. `__FAKE_DIR__` is substituted at write time. Every
/// invocation is appended to `invocations.txt` as `sub|workflow|config|backend`.
const FAKE_DDRS: &str = r#"#!/bin/sh
FAKE_DIR='__FAKE_DIR__'
WS=""
CFG=""
SUB=""
WF=""
BK=""
while [ $# -gt 0 ]; do
  case "$1" in
    --workspace) WS="$2"; shift 2 ;;
    --config) CFG="$2"; shift 2 ;;
    --workflow) WF="$2"; shift 2 ;;
    --backend) BK="$2"; shift 2 ;;
    plan) SUB=plan; shift ;;
    run) SUB=run; shift ;;
    *) shift ;;
  esac
done
mkdir -p "$WS"
printf '%s|%s|%s|%s\n' "$SUB" "$WF" "$CFG" "$BK" >> "$FAKE_DIR/invocations.txt"
if [ "$SUB" = "plan" ]; then
  if [ -f "$FAKE_DIR/drift" ]; then
    echo 'error: data source drift since last plan: ["attributes"]' >&2
    exit 4
  fi
  printf 'sources-lock-contents\n' > "$WS/sources.lock"
  echo "fake ddrs: plan complete" >&2
  exit 0
fi
NSE=0.79
[ -f "$FAKE_DIR/nse" ] && NSE=$(cat "$FAKE_DIR/nse")
EXTRA='"median_kge_finite": 0.881,'
[ -f "$FAKE_DIR/drop_kge" ] && EXTRA=''
ID="2026-09-20T00-00-00Z-train-and-test"
RD="$WS/runs/$ID"
mkdir -p "$RD"
echo "run output → $RD" >&2
cat > "$RD/manifest.json" <<JSON
{
  "run_id": "$ID",
  "status": "ok",
  "workflow": "train-and-test",
  "started_at": "2026-09-20T00:00:00Z",
  "finished_at": "2026-09-20T00:10:00Z",
  "git": { "sha": "beef5678", "dirty": false, "branch": "master" },
  "metrics": {
    $EXTRA
    "median_nse_finite": $NSE,
    "n_gauges_total": 1,
    "phase1_seconds": 44.5
  }
}
JSON
echo "run complete → $RD" >&2
exit 0
"#;

const RECORD_CONFIG: &str = r#"mode: training
workflow: train-and-test
seed: 42
experiment:
  epochs: 2
data_sources:
  attributes: examples/juniata/data/attrs.nc
"#;

const RECORD_ID: &str = "2026-09-16T01-21-34Z-train-and-test";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
    fake_dir: PathBuf,
    ddrs: PathBuf,
    /// The HEAD commit of the fixture's `crates/ddrs` git repository.
    head: String,
}

impl Fixture {
    fn opts(&self, target: &str) -> ReproduceOptions {
        ReproduceOptions {
            target: target.to_string(),
            root: Some(self.root.clone()),
            ddrs: Some(self.ddrs.clone()),
            workspace: Some(self.workspace.clone()),
            backend: None,
            tolerance: 0.01,
            allow_drift: false,
            dry_run: false,
        }
    }

    /// Write a record directory holding `manifest.json` and `config.yaml`.
    fn record(&self, dir: &Path, sha: &str, status: &str, dirty: bool) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("config.yaml"), RECORD_CONFIG).unwrap();
        fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{
  "run_id": "{RECORD_ID}",
  "status": "{status}",
  "workflow": "train-and-test",
  "started_at": "2026-09-16T01:21:34Z",
  "finished_at": "2026-09-16T01:21:53Z",
  "git": {{ "sha": "{sha}", "dirty": {dirty}, "branch": "master" }},
  "sources": {{
    "attributes": {{ "path": "a.nc", "fp": "blake3:aa" }}
  }},
  "metrics": {{
    "median_nse_finite": 0.79,
    "median_kge_finite": 0.881,
    "n_gauges_total": 1,
    "phase1_seconds": 8.06
  }}
}}
"#
            ),
        )
        .unwrap();
    }

    /// The default record: a successful run in the workspace, at the
    /// fixture's real `crates/ddrs` HEAD.
    fn default_record(&self) -> PathBuf {
        let dir = self.workspace.join("runs").join(RECORD_ID);
        self.record(&dir, &self.head, "ok", false);
        dir
    }

    fn reproduction(&self) -> PathBuf {
        self.workspace.join("reproductions").join(RECORD_ID)
    }

    /// One line per fake invocation: `sub|workflow|config|backend`.
    fn invocations(&self) -> Vec<String> {
        match fs::read_to_string(self.fake_dir.join("invocations.txt")) {
            Ok(s) => s.lines().map(str::to_string).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn runs(&self) -> Vec<String> {
        self.invocations()
            .into_iter()
            .filter(|l| l.starts_with("run|"))
            .collect()
    }

    fn drift(&self) {
        fs::write(self.fake_dir.join("drift"), "1").unwrap();
    }

    fn nse(&self, value: &str) {
        fs::write(self.fake_dir.join("nse"), value).unwrap();
    }

    fn drop_kge(&self) {
        fs::write(self.fake_dir.join("drop_kge"), "1").unwrap();
    }
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    fs::create_dir_all(root.join("crates/ddrs")).unwrap();
    // Canonicalized so paths the fake records back match the ones the tests
    // build (TMPDIR is a symlink on some systems).
    let root = fs::canonicalize(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    let head = git_repo(&root.join("crates/ddrs"));

    let fake_dir = tmp.path().join("fake");
    fs::create_dir_all(&fake_dir).unwrap();
    let ddrs = fake_dir.join("fake-ddrs");
    fs::write(
        &ddrs,
        FAKE_DDRS.replace("__FAKE_DIR__", fake_dir.to_str().unwrap()),
    )
    .unwrap();
    fs::set_permissions(&ddrs, fs::Permissions::from_mode(0o755)).unwrap();

    let workspace = root.join("crates/ddrs/.ddrs");
    Fixture {
        _tmp: tmp,
        root,
        workspace,
        fake_dir,
        ddrs,
        head,
    }
}

/// An empty git repository with one commit, so `git rev-parse HEAD` in the
/// fixture's `crates/ddrs` answers with a known sha. Output is captured, not
/// inherited, so `cargo test` stays quiet.
fn git_repo(dir: &Path) -> String {
    let git = |args: &[&str]| -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git on PATH");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "--quiet"]);
    git(&[
        "-c",
        "user.email=retrograde@test",
        "-c",
        "user.name=retrograde",
        "commit",
        "--allow-empty",
        "--quiet",
        "-m",
        "fixture",
    ]);
    git(&["rev-parse", "HEAD"])
}

/// The report line for one metric, split on whitespace.
fn line(report: &str, metric: &str) -> Vec<String> {
    let found: Vec<&str> = report
        .lines()
        .filter(|l| l.split_whitespace().next() == Some(metric))
        .collect();
    assert_eq!(found.len(), 1, "one line for {metric} in:\n{report}");
    found[0].split_whitespace().map(str::to_string).collect()
}

// ---------------------------------------------------------------------------

#[test]
fn matching_metrics_reproduce_and_write_the_reproduction_directory() {
    let f = fixture();
    f.default_record();

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    assert_eq!(out.verdict, Verdict::Reproduced, "report:\n{}", out.report);
    assert_eq!(out.verdict.exit_code(), 0);
    assert!(
        out.report.trim_end().ends_with("REPRODUCED"),
        "report:\n{}",
        out.report
    );

    let dir = f.reproduction();
    assert!(dir.join("config.yaml").is_file(), "config.yaml copied");
    assert!(dir.join("report.txt").is_file(), "report.txt written");
    assert!(dir.join("manifest.json").is_file(), "new manifest copied");

    // The copied config is the record's, byte for byte.
    assert_eq!(
        fs::read_to_string(dir.join("config.yaml")).unwrap(),
        RECORD_CONFIG
    );
    // The copied manifest is the NEW run's, not the record's.
    let new: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(new["run_id"], "2026-09-20T00-00-00Z-train-and-test");
    // report.txt holds what was printed.
    assert_eq!(fs::read_to_string(dir.join("report.txt")).unwrap(), out.report);

    // Nothing was written next to the original record.
    let record = f.workspace.join("runs").join(RECORD_ID);
    let mut names: Vec<String> = fs::read_dir(&record)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["config.yaml", "manifest.json"]);

    // plan then run, both carrying the record's workflow, run against the
    // copied config.
    let calls = f.invocations();
    assert_eq!(calls.len(), 2, "plan then run: {calls:?}");
    let config = dir.join("config.yaml");
    assert_eq!(
        calls[0],
        format!("plan|train-and-test|{}|", config.display())
    );
    assert_eq!(
        calls[1],
        format!("run|train-and-test|{}|", config.display())
    );
}

#[test]
fn every_shared_metric_is_compared_within_tolerance() {
    let f = fixture();
    f.default_record();

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    // metric recorded actual |diff| verdict
    assert_eq!(
        line(&out.report, "median_nse_finite"),
        ["median_nse_finite", "0.79", "0.79", "0", "PASS"]
    );
    assert_eq!(
        line(&out.report, "median_kge_finite"),
        ["median_kge_finite", "0.881", "0.881", "0", "PASS"]
    );
    assert_eq!(
        line(&out.report, "n_gauges_total"),
        ["n_gauges_total", "1", "1", "0", "PASS"]
    );
}

#[test]
fn one_metric_outside_tolerance_is_not_reproduced() {
    let f = fixture();
    f.default_record();
    f.nse("0.9");

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    assert_eq!(out.verdict, Verdict::NotReproduced, "report:\n{}", out.report);
    assert_eq!(out.verdict.exit_code(), 1);
    assert!(
        out.report.trim_end().ends_with("NOT REPRODUCED"),
        "report:\n{}",
        out.report
    );
    assert_eq!(line(&out.report, "median_nse_finite")[4], "FAIL");
    // The metrics that did match still pass.
    assert_eq!(line(&out.report, "median_kge_finite")[4], "PASS");
}

#[test]
fn a_metric_in_one_manifest_and_not_the_other_is_missing() {
    let f = fixture();
    f.default_record();
    f.drop_kge();

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    assert_eq!(out.verdict, Verdict::NotReproduced, "report:\n{}", out.report);
    let kge = line(&out.report, "median_kge_finite");
    assert_eq!(kge[1], "0.881", "recorded value still shown: {kge:?}");
    assert!(
        kge.contains(&"MISSING".to_string()),
        "kge line: {kge:?}\n{}",
        out.report
    );
}

#[test]
fn drift_stops_before_the_run_with_exit_4() {
    let f = fixture();
    f.default_record();
    f.drift();

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    assert_eq!(out.verdict, Verdict::Drift, "report:\n{}", out.report);
    assert_eq!(out.verdict.exit_code(), 4);
    assert!(f.runs().is_empty(), "no run: {:?}", f.invocations());
    // ddrs's own drift line is what is printed.
    assert!(
        out.report.contains(r#"data source drift since last plan: ["attributes"]"#),
        "report:\n{}",
        out.report
    );
    // The reproduction directory still records what was learned.
    assert!(f.reproduction().join("report.txt").is_file());
    assert!(!f.reproduction().join("manifest.json").exists());
}

#[test]
fn allow_drift_continues_and_marks_the_report() {
    let f = fixture();
    f.default_record();
    f.drift();

    let mut opts = f.opts(RECORD_ID);
    opts.allow_drift = true;
    let out = reproduce(&opts).unwrap();

    assert_eq!(out.verdict, Verdict::Reproduced, "report:\n{}", out.report);
    assert_eq!(f.runs().len(), 1, "the run happened: {:?}", f.invocations());
    assert!(
        out.report.contains("DRIFT ALLOWED"),
        "report:\n{}",
        out.report
    );
}

#[test]
fn dry_run_verifies_the_record_and_never_invokes_run() {
    let f = fixture();
    f.default_record();

    let mut opts = f.opts(RECORD_ID);
    opts.dry_run = true;
    let out = reproduce(&opts).unwrap();

    assert_eq!(out.verdict, Verdict::DryRun, "report:\n{}", out.report);
    assert_eq!(out.verdict.exit_code(), 0);
    assert!(f.runs().is_empty(), "no run: {:?}", f.invocations());
    assert_eq!(f.invocations().len(), 1, "plan only");
    assert!(
        out.report
            .trim_end()
            .ends_with("DRY RUN: record and sources verified, nothing run"),
        "report:\n{}",
        out.report
    );
    assert!(f.reproduction().join("config.yaml").is_file());
    assert!(!f.reproduction().join("manifest.json").exists());
}

#[test]
fn a_record_that_did_not_finish_ok_is_refused_by_name() {
    let f = fixture();
    let dir = f.workspace.join("runs").join(RECORD_ID);
    f.record(&dir, &f.head, "failed", false);

    let err = reproduce(&f.opts(RECORD_ID)).unwrap_err().to_string();

    assert!(err.contains("failed"), "names the status: {err}");
    assert!(err.contains("successful"), "says to pick a good run: {err}");
    assert!(f.invocations().is_empty(), "nothing was invoked");
}

#[test]
fn a_committed_manifest_path_resolves_like_a_run_id() {
    let f = fixture();
    let cell = f.root.join("experiments/juniata-repro/results/kan/seed-42");
    f.record(&cell, &f.head, "ok", false);

    let target = cell.join("manifest.json");
    let out = reproduce(&f.opts(target.to_str().unwrap())).unwrap();

    assert_eq!(out.verdict, Verdict::Reproduced, "report:\n{}", out.report);
    // The reproduction is keyed on the record's run id wherever the record
    // lives, and lands in the workspace — never beside the committed cell.
    assert!(f.reproduction().join("report.txt").is_file());
    assert!(!cell.join("report.txt").exists());
}

#[test]
fn the_code_section_reports_the_same_commit_when_the_submodule_matches() {
    let f = fixture();
    f.default_record();

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    assert!(
        out.report.contains("same commit"),
        "report:\n{}",
        out.report
    );
    assert!(
        !out.report.contains("different commit"),
        "report:\n{}",
        out.report
    );
}

#[test]
fn the_code_section_reports_a_different_commit_and_what_it_means() {
    let f = fixture();
    let dir = f.workspace.join("runs").join(RECORD_ID);
    f.record(&dir, "05ab47fa03f6584afad6f6310fec1c5680e2d04c", "ok", false);

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    assert!(
        out.report
            .contains(&format!("different commit: record 05ab47f vs now {}", &f.head[..7])),
        "report:\n{}",
        out.report
    );
    assert!(
        out.report.contains("survives the code change"),
        "report:\n{}",
        out.report
    );
}

#[test]
fn a_dirty_record_is_flagged_as_not_exactly_reproducible() {
    let f = fixture();
    let dir = f.workspace.join("runs").join(RECORD_ID);
    f.record(&dir, &f.head, "ok", true);

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    assert!(
        out.report.contains("uncommitted changes"),
        "report:\n{}",
        out.report
    );
}

#[test]
fn the_record_section_names_the_run_the_workflow_and_where_it_came_from() {
    let f = fixture();
    f.default_record();

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    for expected in [
        RECORD_ID,
        "train-and-test",
        &f.head,
        "master",
        "2026-09-16T01:21:34Z",
    ] {
        assert!(
            out.report.contains(expected),
            "record section is missing {expected}:\n{}",
            out.report
        );
    }
}

#[test]
fn an_unknown_run_id_says_so_without_invoking_ddrs() {
    let f = fixture();
    f.default_record();

    let err = reproduce(&f.opts("no-such-run")).unwrap_err().to_string();

    assert!(err.contains("no-such-run"), "{err}");
    assert!(f.invocations().is_empty(), "nothing was invoked");
}

#[test]
fn the_backend_is_passed_through_to_the_run() {
    let f = fixture();
    f.default_record();

    let mut opts = f.opts(RECORD_ID);
    opts.backend = Some("cpu".to_string());
    let out = reproduce(&opts).unwrap();

    assert_eq!(out.verdict, Verdict::Reproduced, "report:\n{}", out.report);
    assert!(
        f.runs()[0].ends_with("|cpu"),
        "backend passed through: {:?}",
        f.runs()
    );
}

#[test]
fn wall_clock_metrics_are_shown_but_do_not_decide_the_verdict() {
    let f = fixture();
    f.default_record();

    let out = reproduce(&f.opts(RECORD_ID)).unwrap();

    // The record took 8.06 s and the reproduction 44.5 s — a difference
    // hundreds of times the tolerance, and no evidence at all about whether
    // the science came back.
    assert_eq!(out.verdict, Verdict::Reproduced, "report:\n{}", out.report);
    let timing = line(&out.report, "phase1_seconds");
    assert_eq!(timing[1..5], ["8.06", "44.5", "36.44", "SKIP"]);
    assert!(
        out.report.contains("wall clock"),
        "the line says why it is not judged:\n{}",
        out.report
    );
}
