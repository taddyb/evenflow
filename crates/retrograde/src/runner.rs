//! Invoking `ddrs` as a child process.
//!
//! The argument shape and the cwd are ddrs's requirements, not choices:
//! the child runs with cwd `<root>/crates/ddrs` so the relative
//! `data_sources` paths in a config resolve, and always gets an explicit
//! `--workspace` and `--config` (without `--workspace`, ddrs creates
//! `.ddrs` beside the config).
//!
//! ## How the run id is learned
//!
//! `ddrs run` accepts `--json` but discards it (`json: _` in
//! `crates/ddrs/src/bin/ddrs.rs`), so stdout carries nothing. The run
//! directory is announced on **stderr** twice by ddrs:
//!
//! * `run output → <dir>` — `cli::run::run`, printed right after the run
//!   directory is created and *before* the fd-level tee starts, so it
//!   appears even when the run later fails; and
//! * `run complete → <dir>` — `bin/ddrs.rs`, on success only.
//!
//! stderr has to be captured anyway (a failed cell records its last stderr
//! line), so the marker is read out of that capture and each line is
//! forwarded to our own stderr as it arrives. If neither marker is seen,
//! `newest_run_dir_since` falls back to the newest `<workspace>/runs/*`
//! directory touched since the child was launched.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use crate::Error;

/// ddrs writes these with a U+2192 arrow.
const MARKERS: [&str; 2] = ["run output → ", "run complete → "];

#[derive(Debug, Clone)]
pub struct Outcome {
    pub code: Option<i32>,
    pub run_dir: Option<PathBuf>,
    pub last_stderr: Option<String>,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

#[derive(Debug, Clone)]
pub struct Ddrs {
    pub bin: PathBuf,
    /// `<root>/crates/ddrs`
    pub cwd: PathBuf,
    pub workspace: PathBuf,
    pub backend: Option<String>,
}

impl Ddrs {
    /// `ddrs --workspace W --config C plan --workflow train-and-test`.
    /// Locks sources and caches the baseline; cached thereafter.
    pub fn plan(&self, config: &Path) -> Result<Outcome, Error> {
        self.exec(config, "plan", &[])
    }

    /// `ddrs --workspace W --config C run --workflow train-and-test --strict
    /// [--backend B]`. `plan` has no `--backend`, so it is passed here only.
    pub fn run(&self, config: &Path) -> Result<Outcome, Error> {
        let mut extra = vec!["--strict".to_string()];
        if let Some(b) = &self.backend {
            extra.push("--backend".to_string());
            extra.push(b.clone());
        }
        self.exec(config, "run", &extra)
    }

    fn exec(&self, config: &Path, sub: &str, extra: &[String]) -> Result<Outcome, Error> {
        let mut cmd = Command::new(&self.bin);
        cmd.current_dir(&self.cwd)
            .arg("--workspace")
            .arg(&self.workspace)
            .arg("--config")
            .arg(config)
            .arg(sub)
            .arg("--workflow")
            .arg("train-and-test")
            .args(extra)
            // stdout goes straight to the terminal; stderr is teed so the
            // run-dir marker and the last line can be read out of it.
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped());

        let spawn_err = |source| Error::Spawn {
            bin: self.bin.clone(),
            source,
        };
        let mut child = cmd.spawn().map_err(spawn_err)?;
        let stderr = child
            .stderr
            .take()
            .expect("stderr was configured as a pipe");

        let mut run_dir = None;
        let mut last_stderr = None;
        for line in BufReader::new(stderr).lines() {
            let line = line.map_err(spawn_err)?;
            eprintln!("{line}");
            if let Some(dir) = parse_run_dir(&line) {
                run_dir = Some(PathBuf::from(dir));
            }
            if !line.trim().is_empty() {
                last_stderr = Some(line.trim().to_string());
            }
        }
        let status = child.wait().map_err(spawn_err)?;

        Ok(Outcome {
            code: status.code(),
            run_dir,
            last_stderr,
        })
    }
}

/// Pull the run directory out of one of ddrs's stderr markers. ddrs's tee
/// stamps lines it wraps with `[<ts>] `, so the marker is searched for
/// anywhere in the line rather than at its start.
fn parse_run_dir(line: &str) -> Option<&str> {
    for marker in MARKERS {
        if let Some(i) = line.find(marker) {
            let rest = line[i + marker.len()..].trim();
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

/// Fallback when no marker was seen: the newest `<workspace>/runs/*`
/// directory whose manifest `started_at` is at or after `since`.
///
/// `started_at` rather than the directory mtime deliberately. mtime is
/// filesystem-granular and a *previous* cell's run directory can fall
/// inside any backwards slack, which attributes one cell's run to another —
/// exactly the false provenance this crate exists to prevent. A run whose
/// manifest cannot be read is skipped: no run id beats a wrong one.
pub fn newest_run_dir_since(workspace: &Path, since: SystemTime) -> Option<PathBuf> {
    // ddrs stamps started_at truncated to whole seconds, so a run launched
    // at .9s into a second reads back as that second.
    let floor = chrono::DateTime::<chrono::Utc>::from(since) - chrono::Duration::seconds(1);
    let mut best: Option<(String, PathBuf)> = None;
    for entry in std::fs::read_dir(workspace.join("runs")).ok()?.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if !started_at_or_after(&entry.path().join("manifest.json"), floor) {
            continue;
        }
        // Run ids are sortable UTC timestamps, so "newest" is the max.
        let name = entry.file_name().to_string_lossy().into_owned();
        if best.as_ref().map(|(b, _)| name > *b).unwrap_or(true) {
            best = Some((name, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

fn started_at_or_after(manifest: &Path, floor: chrono::DateTime<chrono::Utc>) -> bool {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value
        .get("started_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&chrono::Utc) >= floor)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::parse_run_dir;

    #[test]
    fn reads_both_of_ddrs_stderr_markers_stamped_or_not() {
        assert_eq!(parse_run_dir("run output → /w/runs/abc"), Some("/w/runs/abc"));
        assert_eq!(
            parse_run_dir("run complete → /w/runs/abc"),
            Some("/w/runs/abc")
        );
        assert_eq!(
            parse_run_dir("[2026-09-15T00:00:00Z] run output → /w/runs/abc"),
            Some("/w/runs/abc"),
        );
        assert_eq!(parse_run_dir("training epoch 1"), None);
    }
}
