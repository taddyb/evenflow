//! `retrograde check`: does what ran match what the experiment claims?
//!
//! Every `done` cell's copied `manifest.json` is compared against
//! `expected.<arm>` within that arm's absolute `tolerance`, one report line
//! per arm x seed x metric, then a final `CHECK PASS` or `CHECK FAIL`.
//! `check` reads only; it never writes under the experiment directory and
//! never rewrites `experiment.yaml`.

use std::path::{Path, PathBuf};

use crate::cell::{Cell, CellStatus, Status};
use crate::experiment::{Expectation, Experiment};
use crate::Error;

#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// Path to `experiments/<name>/experiment.yaml`.
    pub experiment: PathBuf,
}

#[derive(Debug)]
pub struct CheckOutcome {
    /// True when every compared metric passed and every arm in `expected`
    /// had at least one `done` cell — the caller's exit code.
    pub passed: bool,
    /// The rendered report, ending in `CHECK PASS` / `CHECK FAIL`.
    pub report: String,
}

/// A report line's verdict, in `check`'s reports and in `reproduce`'s.
/// `Missing` is only ever reached by `reproduce`, where a metric can be in
/// one manifest and not the other; `check` compares against a written
/// expectation, so a metric it cannot find is a `Fail` with a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Pass,
    Fail,
    Skip,
    Missing,
}

impl Verdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Skip => "SKIP",
            Verdict::Missing => "MISSING",
        }
    }
}

/// One report line. `None` renders as `-`: a line that is not a metric
/// comparison (a skipped arm, an arm with nothing to compare) still holds
/// its column positions.
#[derive(Debug, Clone)]
struct Line {
    arm: String,
    seed: Option<i64>,
    metric: Option<String>,
    expected: Option<f64>,
    actual: Option<f64>,
    diff: Option<f64>,
    verdict: Verdict,
    reason: Option<String>,
}

impl Line {
    fn note(arm: &str, seed: Option<i64>, verdict: Verdict, reason: &str) -> Self {
        Line {
            arm: arm.to_string(),
            seed,
            metric: None,
            expected: None,
            actual: None,
            diff: None,
            verdict,
            reason: Some(reason.to_string()),
        }
    }

    fn cells(&self) -> [String; 7] {
        [
            self.arm.clone(),
            self.seed.map(|s| s.to_string()).unwrap_or_else(dash),
            self.metric.clone().unwrap_or_else(dash),
            self.expected.map(num).unwrap_or_else(dash),
            self.actual.map(num).unwrap_or_else(dash),
            self.diff.map(num).unwrap_or_else(dash),
            self.verdict.as_str().to_string(),
        ]
    }
}

fn dash() -> String {
    "-".to_string()
}

/// Shortest faithful rendering at six decimals: `0.79`, `0.0012`, `3211`.
/// Six decimals is far below any tolerance worth writing down, and it keeps
/// float noise (`0.0011999999999999789`) out of the report.
pub(crate) fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn check(opts: &CheckOptions) -> Result<CheckOutcome, Error> {
    let exp_path = crate::canonical(&opts.experiment)?;
    let exp_dir = exp_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let experiment = Experiment::load(&exp_path)?;
    let expectations = experiment.expectations()?;
    let cells = crate::cell::cells(&exp_dir, &experiment)?;

    let mut lines = Vec::new();

    // Arms in `experiment.yaml` order; each compared, or SKIP if `expected`
    // says nothing about it.
    for arm in &experiment.arms {
        let cells: Vec<&Cell> = cells.iter().filter(|c| c.arm == arm.name).collect();
        match expectations.iter().find(|e| e.arm == arm.name) {
            None => lines.push(Line::note(
                &arm.name,
                None,
                Verdict::Skip,
                "no expected entry",
            )),
            Some(expectation) => lines.extend(check_arm(expectation, &cells)?),
        }
    }

    // An `expected` key naming an arm that `arms:` does not define has no
    // cell and therefore no `done` cell.
    for expectation in &expectations {
        if !experiment.arms.iter().any(|a| a.name == expectation.arm) {
            lines.push(Line::note(
                &expectation.arm,
                None,
                Verdict::Fail,
                "no done cell: no such arm in arms:",
            ));
        }
    }

    let passed = !lines.iter().any(|l| l.verdict == Verdict::Fail);
    Ok(CheckOutcome {
        passed,
        report: render(&lines, passed),
    })
}

/// One arm: every `done` cell compared, every other cell noted, and a FAIL
/// if no cell was `done` at all.
fn check_arm(expectation: &Expectation, cells: &[&Cell]) -> Result<Vec<Line>, Error> {
    let mut lines = Vec::new();
    let mut any_done = false;

    for cell in cells {
        let status = Status::read(&cell.status_path())?;
        if status.status != CellStatus::Done {
            lines.push(Line::note(
                &cell.arm,
                Some(cell.seed),
                Verdict::Skip,
                &format!("cell is {}, not compared", status.status.as_str()),
            ));
            continue;
        }
        any_done = true;

        let manifest = cell.manifest_path();
        let metrics = if manifest.is_file() {
            Some(crate::read_json(&manifest)?)
        } else {
            None
        };

        for (name, want) in &expectation.metrics {
            lines.push(compare(cell, expectation.tolerance, name, *want, &metrics));
        }
    }

    if !any_done {
        lines.push(Line::note(
            &expectation.arm,
            None,
            Verdict::Fail,
            "no done cell",
        ));
    }
    Ok(lines)
}

/// One metric of one cell. A metric the manifest does not carry is a FAIL
/// with the reason, never a silent pass.
fn compare(
    cell: &Cell,
    tolerance: f64,
    metric: &str,
    want: f64,
    manifest: &Option<serde_json::Value>,
) -> Line {
    let line = Line {
        arm: cell.arm.clone(),
        seed: Some(cell.seed),
        metric: Some(metric.to_string()),
        expected: Some(want),
        actual: None,
        diff: None,
        verdict: Verdict::Fail,
        reason: None,
    };

    let Some(manifest) = manifest else {
        return Line {
            reason: Some("no manifest.json in the cell".to_string()),
            ..line
        };
    };
    let value = manifest.get("metrics").and_then(|m| m.get(metric));
    let Some(value) = value else {
        return Line {
            reason: Some("metric missing from the manifest".to_string()),
            ..line
        };
    };
    let Some(got) = value.as_f64() else {
        return Line {
            reason: Some("metric is not a number in the manifest".to_string()),
            ..line
        };
    };

    let diff = (got - want).abs();
    Line {
        actual: Some(got),
        diff: Some(diff),
        verdict: if diff <= tolerance {
            Verdict::Pass
        } else {
            Verdict::Fail
        },
        reason: if diff <= tolerance {
            None
        } else {
            Some(format!("outside tolerance {}", num(tolerance)))
        },
        ..line
    }
}

/// One rendered row: its columns, and an optional reason that follows the
/// last of them in parentheses. `reproduce` renders its comparison through
/// [`render_rows`] too, so both verbs' tables line their columns up the
/// same way and there is one padding routine in the crate.
#[derive(Debug, Clone)]
pub(crate) struct TableRow {
    pub cells: Vec<String>,
    pub reason: Option<String>,
}

/// Columns padded to their widest cell, reason in parentheses after the
/// last column. The last column is not padded to (nothing follows it but a
/// reason, and a reason is never padded to).
pub(crate) fn render_rows(rows: &[TableRow]) -> String {
    let columns = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let mut width = vec![0usize; columns];
    for row in rows {
        for (w, field) in width.iter_mut().zip(&row.cells) {
            *w = (*w).max(field.chars().count());
        }
    }

    let mut out = String::new();
    for row in rows {
        let mut rendered = String::new();
        for (i, field) in row.cells.iter().enumerate() {
            if i > 0 {
                rendered.push(' ');
            }
            rendered.push_str(field);
            if i + 1 < columns {
                let pad = width[i] - field.chars().count();
                rendered.extend(std::iter::repeat_n(' ', pad));
            }
        }
        if let Some(reason) = &row.reason {
            rendered.push_str(&format!(" ({reason})"));
        }
        out.push_str(rendered.trim_end());
        out.push('\n');
    }
    out
}

/// Columns padded to their widest cell, verdict last, reason in parentheses
/// after it. The verdict line is the last line of the report.
fn render(lines: &[Line], passed: bool) -> String {
    let rows: Vec<TableRow> = lines
        .iter()
        .map(|l| TableRow {
            cells: l.cells().to_vec(),
            reason: l.reason.clone(),
        })
        .collect();
    let mut out = render_rows(&rows);
    out.push_str(if passed { "CHECK PASS" } else { "CHECK FAIL" });
    out.push('\n');
    out
}
