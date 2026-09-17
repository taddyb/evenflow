//! `retrograde sweep` driven against a fake `ddrs` binary.
//!
//! No test here launches the real ddrs. The fake is a POSIX shell script
//! written into a tempdir that accepts ddrs's argument shape, writes
//! `<workspace>/runs/<id>/manifest.json`, and prints the same
//! `run output -> <dir>` marker on stderr that `ddrs::cli::run::run` does.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use retrograde::{sweep, SweepOptions};

/// The fake ddrs. `__FAKE_DIR__` is substituted at write time; everything it
/// prints goes to stderr so `cargo test` output stays clean.
const FAKE_DDRS: &str = r#"#!/bin/sh
FAKE_DIR='__FAKE_DIR__'
WS=""
CFG=""
SUB=""
while [ $# -gt 0 ]; do
  case "$1" in
    --workspace) WS="$2"; shift 2 ;;
    --config) CFG="$2"; shift 2 ;;
    plan) SUB=plan; shift ;;
    run) SUB=run; shift ;;
    *) shift ;;
  esac
done
mkdir -p "$WS"
if [ "$SUB" = "plan" ]; then
  printf 'sources-lock-contents\n' > "$WS/sources.lock"
  echo "fake ddrs: plan complete" >&2
  exit 0
fi
printf '%s\n' "$CFG" >> "$FAKE_DIR/run_invocations.txt"
if [ -f "$FAKE_DIR/fail_match" ]; then
  M=$(cat "$FAKE_DIR/fail_match")
  case "$CFG" in
    *"$M"*) echo "fake ddrs: deliberate failure" >&2; exit 3 ;;
  esac
fi
N=$(wc -l < "$FAKE_DIR/run_invocations.txt" | tr -d ' ')
ID="2026-09-15T00-00-0${N}Z-train-and-test"
RD="$WS/runs/$ID"
mkdir -p "$RD"
echo "run output → $RD" >&2
cat > "$RD/manifest.json" <<JSON
{
  "run_id": "$ID",
  "status": "ok",
  "started_at": "2026-09-15T00:00:00Z",
  "finished_at": "2026-09-15T00:10:00Z",
  "git": { "sha": "cafe1234", "dirty": false, "branch": "master" },
  "metrics": {
    "median_nse_finite": 0.79,
    "median_kge_finite": 0.881,
    "mean_nse_finite": 0.7,
    "n_gauges_finite_nse": 1,
    "n_gauges_total": 1
  }
}
JSON
echo "run complete → $RD" >&2
exit 0
"#;

/// A ddrs.yaml-shaped arm config: `seed` already present (must be replaced),
/// `np_seed` absent (must be inserted), everything else untouched.
const ARM_CONFIG: &str = r#"mode: training
workflow: train-and-test
seed: 1
experiment:
  epochs: 2
  learning_rate: 0.01
data_sources:
  attributes: examples/juniata/data/attrs.nc
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    exp_yaml: PathBuf,
    exp_dir: PathBuf,
    workspace: PathBuf,
    fake_dir: PathBuf,
    ddrs: PathBuf,
}

impl Fixture {
    fn opts(&self) -> SweepOptions {
        SweepOptions {
            experiment: self.exp_yaml.clone(),
            only_failed: false,
            root: Some(self.root.clone()),
            ddrs: Some(self.ddrs.clone()),
            workspace: Some(self.workspace.clone()),
            backend: None,
        }
    }

    fn cell(&self, arm: &str, seed: i64) -> PathBuf {
        self.exp_dir.join("results").join(arm).join(format!("seed-{seed}"))
    }

    fn status(&self, arm: &str, seed: i64) -> serde_json::Value {
        let p = self.cell(arm, seed).join("status.json");
        serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap()
    }

    /// One line per `ddrs run` invocation the fake saw.
    fn run_invocations(&self) -> Vec<String> {
        let p = self.fake_dir.join("run_invocations.txt");
        match fs::read_to_string(&p) {
            Ok(s) => s.lines().map(|l| l.to_string()).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn fail_on(&self, needle: &str) {
        fs::write(self.fake_dir.join("fail_match"), needle).unwrap();
    }

    fn clear_failures(&self) {
        let _ = fs::remove_file(self.fake_dir.join("fail_match"));
    }
}

/// `arms` is (arm name, seeds). A seed list of one is written as the scalar
/// `seed:` form so both spellings are exercised.
fn fixture(arms: &[(&str, &[i64])]) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalized so the cell paths the fake records back match the ones
    // the tests build (TMPDIR is a symlink on some systems).
    let root = tmp.path().join("root");
    fs::create_dir_all(root.join("crates/ddrs")).unwrap();
    let root = fs::canonicalize(&root).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();

    let exp_dir = root.join("experiments/exp1");
    fs::create_dir_all(&exp_dir).unwrap();

    let mut yaml = String::from("name: exp1\nquestion: >\n  does the layer work\narms:\n");
    for (arm, seeds) in arms {
        let arm_dir = exp_dir.join("arms").join(arm);
        fs::create_dir_all(&arm_dir).unwrap();
        fs::write(arm_dir.join("ddrs.yaml"), ARM_CONFIG).unwrap();
        yaml.push_str(&format!("  - name: {arm}\n    config: arms/{arm}/ddrs.yaml\n"));
        if seeds.len() == 1 {
            yaml.push_str(&format!("    seed: {}\n", seeds[0]));
        } else {
            let list: Vec<String> = seeds.iter().map(|s| s.to_string()).collect();
            yaml.push_str(&format!("    seeds: [{}]\n", list.join(", ")));
        }
    }
    yaml.push_str("expected: {}\nresult: \"\"\nconclusion: \"\"\n");
    let exp_yaml = exp_dir.join("experiment.yaml");
    fs::write(&exp_yaml, yaml).unwrap();

    let fake_dir = tmp.path().join("fake");
    fs::create_dir_all(&fake_dir).unwrap();
    let ddrs = fake_dir.join("fake-ddrs");
    fs::write(&ddrs, FAKE_DDRS.replace("__FAKE_DIR__", fake_dir.to_str().unwrap())).unwrap();
    fs::set_permissions(&ddrs, fs::Permissions::from_mode(0o755)).unwrap();

    let workspace = root.join("crates/ddrs/.ddrs");
    Fixture { _tmp: tmp, root, exp_yaml, exp_dir, workspace, fake_dir, ddrs }
}

fn yaml_of(path: &Path) -> serde_yaml::Value {
    serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

// ---------------------------------------------------------------------------

#[test]
fn derived_config_sets_both_seeds_and_leaves_every_other_key_unchanged() {
    let f = fixture(&[("kan", &[42])]);
    let out = sweep(&f.opts()).unwrap();
    assert!(out.all_done, "fake ddrs should have succeeded");

    let derived = yaml_of(&f.cell("kan", 42).join("config.yaml"));
    assert_eq!(derived["seed"].as_i64(), Some(42), "seed replaced");
    assert_eq!(derived["np_seed"].as_i64(), Some(42), "np_seed inserted");

    // Every other key survives, values and nesting intact.
    let original = yaml_of(&f.exp_dir.join("arms/kan/ddrs.yaml"));
    for key in ["mode", "workflow", "experiment", "data_sources"] {
        assert_eq!(derived[key], original[key], "key {key} changed");
    }
    assert_eq!(derived["experiment"]["epochs"].as_i64(), Some(2));
    assert_eq!(
        derived["data_sources"]["attributes"].as_str(),
        Some("examples/juniata/data/attrs.nc"),
    );
}

#[test]
fn two_arms_two_seeds_produce_four_done_cells_manifests_and_a_sorted_summary() {
    let f = fixture(&[("kan", &[1, 2]), ("mlp", &[1, 2])]);
    let out = sweep(&f.opts()).unwrap();

    assert!(out.all_done);
    assert_eq!(f.run_invocations().len(), 4, "one ddrs run per cell");

    for (arm, seed) in [("kan", 1), ("kan", 2), ("mlp", 1), ("mlp", 2)] {
        let cell = f.cell(arm, seed);
        assert_eq!(f.status(arm, seed)["status"], "done", "{arm}/seed-{seed}");
        assert!(cell.join("manifest.json").is_file(), "{arm}/seed-{seed} manifest");
        assert!(cell.join("config.yaml").is_file(), "{arm}/seed-{seed} config");
        assert!(
            f.status(arm, seed)["run_id"].is_string(),
            "{arm}/seed-{seed} run_id recorded",
        );

        // run_dir and log are recorded relative to the workspace root, so a
        // committed results/ tree means the same thing on any clone.
        let st = f.status(arm, seed);
        let run_id = st["run_id"].as_str().unwrap();
        assert_eq!(
            st["run_dir"].as_str(),
            Some(format!("crates/ddrs/.ddrs/runs/{run_id}").as_str()),
            "{arm}/seed-{seed} run_dir is root-relative",
        );
        assert_eq!(
            st["log"].as_str(),
            Some(format!("crates/ddrs/.ddrs/runs/{run_id}/run.log").as_str()),
            "{arm}/seed-{seed} log is root-relative",
        );
    }

    let csv = fs::read_to_string(f.exp_dir.join("results/summary.csv")).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        lines[0],
        "arm,seed,status,run_id,median_nse_finite,median_kge_finite,mean_nse_finite,\
         n_gauges_finite_nse,n_gauges_total,ddrs_sha",
    );
    assert_eq!(lines.len(), 5, "header + four rows");
    let cells: Vec<(&str, &str)> = lines[1..]
        .iter()
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            (f[0], f[1])
        })
        .collect();
    assert_eq!(cells, vec![("kan", "1"), ("kan", "2"), ("mlp", "1"), ("mlp", "2")]);

    // Metrics and the ddrs sha are carried across from the manifest.
    let row: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(row[2], "done");
    assert_eq!(row[4], "0.79");
    assert_eq!(row[5], "0.881");
    assert_eq!(row[9], "cafe1234");

    // The sources.lock the sweep ran against is pinned beside experiment.yaml.
    assert_eq!(
        fs::read_to_string(f.exp_dir.join("sources.lock")).unwrap(),
        "sources-lock-contents\n",
    );
}

#[test]
fn a_failing_cell_is_recorded_failed_and_only_failed_reruns_just_that_cell() {
    let f = fixture(&[("kan", &[1, 2])]);
    f.fail_on("seed-2");

    let out = sweep(&f.opts()).unwrap();
    assert!(!out.all_done, "sweep reports failure -> exit 1");
    assert_eq!(f.status("kan", 1)["status"], "done");

    let bad = f.status("kan", 2);
    assert_eq!(bad["status"], "failed");
    assert_eq!(bad["exit_code"].as_i64(), Some(3));
    assert_eq!(bad["error"].as_str(), Some("fake ddrs: deliberate failure"));
    assert_eq!(f.run_invocations().len(), 2);

    // Non-done cells get an empty metric block in the summary.
    let csv = fs::read_to_string(f.exp_dir.join("results/summary.csv")).unwrap();
    let failed_row: Vec<&str> = csv.lines().nth(2).unwrap().split(',').collect();
    assert_eq!(&failed_row[0..4], &["kan", "2", "failed", ""]);
    assert_eq!(&failed_row[4..10], &["", "", "", "", "", ""]);

    // --only-failed reruns the failed cell and nothing else.
    f.clear_failures();
    let out = sweep(&SweepOptions { only_failed: true, ..f.opts() }).unwrap();
    assert!(out.all_done);
    assert_eq!(f.run_invocations().len(), 3, "exactly one more ddrs run");
    assert_eq!(f.run_invocations()[2], f.cell("kan", 2).join("config.yaml").to_str().unwrap());
    assert_eq!(f.status("kan", 2)["status"], "done");
}

#[test]
fn a_done_cell_is_not_rerun_even_without_only_failed() {
    let f = fixture(&[("kan", &[1])]);
    let out = sweep(&f.opts()).unwrap();
    assert!(out.all_done);
    assert_eq!(f.run_invocations().len(), 1);

    let before = fs::read_to_string(f.cell("kan", 1).join("status.json")).unwrap();
    let out = sweep(&f.opts()).unwrap();
    assert!(out.all_done);
    assert_eq!(f.run_invocations().len(), 1, "done cell must not be rerun");
    assert_eq!(
        fs::read_to_string(f.cell("kan", 1).join("status.json")).unwrap(),
        before,
        "a skipped cell's status is left untouched",
    );
}

#[test]
fn a_running_cell_with_a_finished_manifest_is_reconciled_to_done() {
    // The absolute spelling: a run outside the workspace root is recorded
    // that way, and is still read back correctly.
    reconcile_running_cell(&|run_dir, _root| run_dir.to_str().unwrap().to_string());
}

/// The spelling retrograde writes today. Reading it back has to resolve it
/// against the workspace root, or the manifest is never found.
#[test]
fn a_running_cell_with_a_relative_run_dir_is_reconciled_to_done() {
    reconcile_running_cell(&|run_dir, root| {
        run_dir.strip_prefix(root).unwrap().to_str().unwrap().to_string()
    });
}

/// Body of the two tests above: `run_dir_field` renders the `run_dir` the
/// interrupted sweep is pretended to have left behind.
fn reconcile_running_cell(run_dir_field: &dyn Fn(&Path, &Path) -> String) {
    let f = fixture(&[("kan", &[7])]);

    // A crash (or Ctrl-C) left `running` behind, but the run itself finished.
    let run_id = "2026-09-15T00-00-00Z-train-and-test";
    let run_dir = f.workspace.join("runs").join(run_id);
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("manifest.json"),
        serde_json::json!({
            "run_id": run_id,
            "status": "ok",
            "started_at": "2026-09-15T00:00:00Z",
            "finished_at": "2026-09-15T00:10:00Z",
            "git": { "sha": "feedface", "dirty": false, "branch": "master" },
            "metrics": {
                "median_nse_finite": 0.5,
                "median_kge_finite": 0.6,
                "mean_nse_finite": 0.4,
                "n_gauges_finite_nse": 3,
                "n_gauges_total": 4
            }
        })
        .to_string(),
    )
    .unwrap();

    let cell = f.cell("kan", 7);
    fs::create_dir_all(&cell).unwrap();
    fs::write(
        cell.join("status.json"),
        serde_json::json!({
            "status": "running",
            "run_id": run_id,
            "run_dir": run_dir_field(&run_dir, &f.root),
            "log": null,
            "started_at": "2026-09-15T00:00:00Z",
            "finished_at": null,
            "exit_code": null,
            "error": null
        })
        .to_string(),
    )
    .unwrap();

    let out = sweep(&f.opts()).unwrap();
    assert!(out.all_done);
    assert!(f.run_invocations().is_empty(), "reconciled, not rerun");

    let st = f.status("kan", 7);
    assert_eq!(st["status"], "done");
    assert_eq!(st["run_id"].as_str(), Some(run_id));
    assert!(cell.join("manifest.json").is_file(), "manifest pulled into the cell");

    let csv = fs::read_to_string(f.exp_dir.join("results/summary.csv")).unwrap();
    let row: Vec<&str> = csv.lines().nth(1).unwrap().split(',').collect();
    assert_eq!(&row[0..3], &["kan", "7", "done"]);
    assert_eq!(row[9], "feedface");
}

#[test]
fn a_running_cell_without_a_finished_run_is_treated_as_failed() {
    let f = fixture(&[("kan", &[7])]);
    let cell = f.cell("kan", 7);
    fs::create_dir_all(&cell).unwrap();
    fs::write(
        cell.join("status.json"),
        serde_json::json!({ "status": "running", "run_dir": null }).to_string(),
    )
    .unwrap();

    // A crash left `running` with no run dir: the cell is failed, so a plain
    // sweep reruns it (and here the fake then succeeds).
    let out = sweep(&f.opts()).unwrap();
    assert!(out.all_done);
    assert_eq!(f.run_invocations().len(), 1);
}

#[test]
fn a_missing_ddrs_binary_names_the_build_command() {
    let f = fixture(&[("kan", &[1])]);
    let err = sweep(&SweepOptions {
        ddrs: Some(f.root.join("target/release/ddrs")),
        ..f.opts()
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("cargo build --release -p ddrs --bin ddrs"), "got: {err}");
}

/// The lock a sweep commits must be the one ddrs wrote during THIS sweep's
/// `plan`, not whatever the workspace happened to hold beforehand. Copying it
/// before the first cell captures the previous run's pins, so the committed
/// record would claim the arms ran against data they did not.
#[test]
fn the_committed_lock_is_the_one_this_sweep_planned_against() {
    let f = fixture(&[("kan", &[1])]);
    fs::create_dir_all(&f.workspace).unwrap();
    fs::write(f.workspace.join("sources.lock"), "STALE-from-a-previous-plan\n").unwrap();

    let out = sweep(&f.opts()).unwrap();
    assert!(out.all_done);

    let committed = fs::read_to_string(f.exp_dir.join("sources.lock")).unwrap();
    assert_eq!(
        committed, "sources-lock-contents\n",
        "the committed lock must be what plan wrote, not the stale one",
    );
}

/// A sweep that runs nothing leaves the committed lock alone: the record of
/// what the original arms ran against is not to be overwritten by a no-op.
#[test]
fn a_sweep_that_runs_nothing_leaves_the_committed_lock_alone() {
    let f = fixture(&[("kan", &[1])]);
    sweep(&f.opts()).unwrap();
    fs::write(f.exp_dir.join("sources.lock"), "ORIGINAL\n").unwrap();
    fs::write(f.workspace.join("sources.lock"), "DIFFERENT\n").unwrap();

    sweep(&f.opts()).unwrap();

    assert_eq!(
        fs::read_to_string(f.exp_dir.join("sources.lock")).unwrap(),
        "ORIGINAL\n",
        "a no-op sweep must not rewrite the record's lock",
    );
}
