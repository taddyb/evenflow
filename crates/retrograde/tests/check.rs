//! `retrograde check` against hand-written `results/` fixtures.
//!
//! No test here launches ddrs, or retrograde's sweep: a cell is just the
//! three files a sweep would have left behind, written by hand into a
//! tempdir. The point is to pin `check`'s verdicts and its output format.

use std::fs;
use std::path::PathBuf;

use retrograde::{check, CheckOptions};

struct Fixture {
    _tmp: tempfile::TempDir,
    exp_yaml: PathBuf,
    exp_dir: PathBuf,
}

impl Fixture {
    /// An experiment directory holding `experiment.yaml` with `body` as its
    /// `arms:`/`expected:` text.
    fn new(body: &str) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let exp_dir = tmp.path().join("experiments/x");
        fs::create_dir_all(&exp_dir).expect("mkdir");
        let exp_yaml = exp_dir.join("experiment.yaml");
        fs::write(&exp_yaml, format!("name: x\n{body}")).expect("write experiment.yaml");
        Fixture {
            _tmp: tmp,
            exp_yaml,
            exp_dir,
        }
    }

    /// A `done` cell: `status.json` plus a `manifest.json` carrying `metrics`.
    fn done_cell(&self, arm: &str, seed: i64, metrics: &str) -> &Self {
        let dir = self.cell_dir(arm, seed);
        fs::create_dir_all(&dir).expect("mkdir cell");
        fs::write(
            dir.join("status.json"),
            r#"{"status": "done", "run_id": "2026-09-15T00-00-00Z-train-and-test"}"#,
        )
        .expect("write status.json");
        fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{"run_id": "2026-09-15T00-00-00Z-train-and-test",
                     "status": "ok",
                     "metrics": {metrics}}}"#
            ),
        )
        .expect("write manifest.json");
        self
    }

    /// A cell that ran and failed: `status.json` only, no manifest.
    fn failed_cell(&self, arm: &str, seed: i64) -> &Self {
        let dir = self.cell_dir(arm, seed);
        fs::create_dir_all(&dir).expect("mkdir cell");
        fs::write(
            dir.join("status.json"),
            r#"{"status": "failed", "exit_code": 3, "error": "boom"}"#,
        )
        .expect("write status.json");
        self
    }

    fn cell_dir(&self, arm: &str, seed: i64) -> PathBuf {
        self.exp_dir
            .join("results")
            .join(arm)
            .join(format!("seed-{seed}"))
    }

    fn check(&self) -> retrograde::CheckOutcome {
        check(&CheckOptions {
            experiment: self.exp_yaml.clone(),
        })
        .expect("check")
    }
}

/// The whitespace-separated fields of the report line for `arm`/`metric`.
fn line<'a>(report: &'a str, arm: &str, metric: &str) -> Vec<&'a str> {
    let hit: Vec<&str> = report
        .lines()
        .filter(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            f.first() == Some(&arm) && f.get(2) == Some(&metric)
        })
        .collect();
    assert_eq!(
        hit.len(),
        1,
        "expected exactly one {arm}/{metric} line in:\n{report}"
    );
    hit[0].split_whitespace().collect()
}

const EXPECTED_KAN: &str = "\
arms:
  - name: kan
    config: arms/kan/ddrs.yaml
    seeds: [42]
expected:
  kan:
    median_nse_finite: 0.790
    median_kge_finite: 0.881
    tolerance: 0.04
";

#[test]
fn every_metric_within_tolerance_passes() {
    let f = Fixture::new(EXPECTED_KAN);
    f.done_cell(
        "kan",
        42,
        r#"{"median_nse_finite": 0.7912, "median_kge_finite": 0.8801}"#,
    );

    let out = f.check();
    assert!(out.passed, "report:\n{}", out.report);
    assert!(
        out.report.trim_end().ends_with("CHECK PASS"),
        "report:\n{}",
        out.report
    );

    // arm seed metric expected actual |diff| verdict
    let nse = line(&out.report, "kan", "median_nse_finite");
    assert_eq!(nse[0], "kan");
    assert_eq!(nse[1], "42");
    assert_eq!(nse[3], "0.79");
    assert_eq!(nse[4], "0.7912");
    assert_eq!(nse[5], "0.0012");
    assert_eq!(nse[6], "PASS");

    assert_eq!(line(&out.report, "kan", "median_kge_finite")[6], "PASS");
}

#[test]
fn one_metric_outside_tolerance_fails_the_whole_check() {
    let f = Fixture::new(EXPECTED_KAN);
    // NSE is fine; KGE is 0.05 off with a tolerance of 0.04.
    f.done_cell(
        "kan",
        42,
        r#"{"median_nse_finite": 0.79, "median_kge_finite": 0.831}"#,
    );

    let out = f.check();
    assert!(!out.passed, "report:\n{}", out.report);
    assert!(
        out.report.trim_end().ends_with("CHECK FAIL"),
        "report:\n{}",
        out.report
    );
    assert_eq!(line(&out.report, "kan", "median_nse_finite")[6], "PASS");

    let kge = line(&out.report, "kan", "median_kge_finite");
    assert_eq!(kge[3], "0.881");
    assert_eq!(kge[4], "0.831");
    assert_eq!(kge[5], "0.05");
    assert_eq!(kge[6], "FAIL");
}

#[test]
fn an_expected_arm_with_no_done_cell_fails() {
    let f = Fixture::new(EXPECTED_KAN);
    f.failed_cell("kan", 42);

    let out = f.check();
    assert!(!out.passed, "report:\n{}", out.report);
    assert!(
        out.report.trim_end().ends_with("CHECK FAIL"),
        "report:\n{}",
        out.report
    );
    let fail: Vec<&str> = out
        .report
        .lines()
        .filter(|l| l.starts_with("kan ") && l.contains("FAIL"))
        .collect();
    assert_eq!(fail.len(), 1, "report:\n{}", out.report);
    assert!(
        fail[0].contains("no done cell"),
        "the FAIL must say why: {}",
        fail[0]
    );
}

#[test]
fn an_arm_with_no_expected_entry_is_skipped_and_does_not_fail() {
    let f = Fixture::new(
        "\
arms:
  - name: kan
    config: arms/kan/ddrs.yaml
    seeds: [42]
  - name: mlp
    config: arms/mlp/ddrs.yaml
    seeds: [42]
expected:
  kan:
    median_nse_finite: 0.790
    tolerance: 0.04
",
    );
    f.done_cell("kan", 42, r#"{"median_nse_finite": 0.79}"#);
    f.done_cell("mlp", 42, r#"{"median_nse_finite": 0.1}"#);

    let out = f.check();
    assert!(out.passed, "report:\n{}", out.report);
    assert!(
        out.report.trim_end().ends_with("CHECK PASS"),
        "report:\n{}",
        out.report
    );
    let mlp: Vec<&str> = out
        .report
        .lines()
        .filter(|l| l.starts_with("mlp "))
        .collect();
    assert_eq!(mlp.len(), 1, "report:\n{}", out.report);
    assert!(mlp[0].contains("SKIP"), "{}", mlp[0]);
    assert!(!out.report.contains("FAIL"), "report:\n{}", out.report);
}

#[test]
fn a_metric_missing_from_the_manifest_fails_with_a_reason() {
    let f = Fixture::new(EXPECTED_KAN);
    f.done_cell("kan", 42, r#"{"median_nse_finite": 0.79}"#);

    let out = f.check();
    assert!(!out.passed, "report:\n{}", out.report);
    let kge = line(&out.report, "kan", "median_kge_finite");
    assert_eq!(kge[6], "FAIL");
    assert!(
        kge.join(" ").contains("missing"),
        "the FAIL must say why: {}",
        kge.join(" ")
    );
    assert_eq!(line(&out.report, "kan", "median_nse_finite")[6], "PASS");
}

#[test]
fn every_seed_of_an_arm_is_compared() {
    let f = Fixture::new(
        "\
arms:
  - name: kan
    config: arms/kan/ddrs.yaml
    seeds: [1, 2]
expected:
  kan:
    median_nse_finite: 0.790
    tolerance: 0.04
",
    );
    f.done_cell("kan", 1, r#"{"median_nse_finite": 0.79}"#);
    f.done_cell("kan", 2, r#"{"median_nse_finite": 0.60}"#);

    let out = f.check();
    assert!(!out.passed, "report:\n{}", out.report);
    let seeds: Vec<Vec<&str>> = out
        .report
        .lines()
        .filter(|l| l.contains("median_nse_finite"))
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(seeds.len(), 2, "report:\n{}", out.report);
    assert_eq!(seeds[0][1], "1");
    assert_eq!(seeds[0][6], "PASS");
    assert_eq!(seeds[1][1], "2");
    assert_eq!(seeds[1][6], "FAIL");
}

#[test]
fn an_expected_block_without_a_tolerance_is_an_error() {
    let f = Fixture::new(
        "\
arms:
  - name: kan
    config: arms/kan/ddrs.yaml
    seeds: [42]
expected:
  kan:
    median_nse_finite: 0.790
",
    );
    f.done_cell("kan", 42, r#"{"median_nse_finite": 0.79}"#);

    let err = check(&CheckOptions {
        experiment: f.exp_yaml.clone(),
    })
    .expect_err("a missing tolerance must not silently pass");
    let msg = err.to_string();
    assert!(msg.contains("tolerance"), "{msg}");
    assert!(msg.contains("kan"), "{msg}");
}

#[test]
fn unknown_keys_in_experiment_yaml_survive_a_check() {
    let f = Fixture::new(
        "\
question: >
  does it reproduce?
arms:
  - name: kan
    config: arms/kan/ddrs.yaml
    seeds: [42]
expected:
  kan:
    median_nse_finite: 0.790
    tolerance: 0.04
result: \"\"
conclusion: \"\"
",
    );
    f.done_cell("kan", 42, r#"{"median_nse_finite": 0.79}"#);

    let before = fs::read_to_string(&f.exp_yaml).expect("read");
    let out = f.check();
    assert!(out.passed, "report:\n{}", out.report);
    assert_eq!(
        before,
        fs::read_to_string(&f.exp_yaml).expect("read"),
        "check must never rewrite experiment.yaml"
    );
    // ... and it writes nothing at all under the experiment directory.
    assert!(!f.exp_dir.join("results/summary.csv").exists());
}

/// Guard for the doc example in the task report: the rendered table's
/// columns line up and the verdict is the last line.
#[test]
fn the_report_is_a_table_then_a_verdict() {
    let f = Fixture::new(EXPECTED_KAN);
    f.done_cell(
        "kan",
        42,
        r#"{"median_nse_finite": 0.7912, "median_kge_finite": 0.8801}"#,
    );
    let out = f.check();
    let lines: Vec<&str> = out.report.lines().collect();
    assert_eq!(lines.len(), 3, "report:\n{}", out.report);
    assert_eq!(lines[2], "CHECK PASS");
    for l in &lines[..2] {
        assert!(!l.ends_with(' '), "no trailing whitespace: {l:?}");
    }
    assert_eq!(
        lines[0].find("median_nse_finite"),
        lines[1].find("median_kge_finite"),
        "metric column must align:\n{}",
        out.report
    );
}
