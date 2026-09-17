//! `results/summary.csv`: one row per cell, sorted by arm then seed.

use std::path::Path;

use crate::cell::CellStatus;
use crate::Error;

pub const HEADER: &str = "arm,seed,status,run_id,median_nse_finite,median_kge_finite,\
                          mean_nse_finite,n_gauges_finite_nse,n_gauges_total,ddrs_sha";

/// The manifest metric keys that become columns, in column order.
const METRIC_KEYS: [&str; 5] = [
    "median_nse_finite",
    "median_kge_finite",
    "mean_nse_finite",
    "n_gauges_finite_nse",
    "n_gauges_total",
];

#[derive(Debug, Clone)]
pub struct Row {
    pub arm: String,
    pub seed: i64,
    pub status: CellStatus,
    pub run_id: String,
    /// In `METRIC_KEYS` order; empty for a cell that is not `done`.
    pub metrics: [String; 5],
    pub ddrs_sha: String,
}

impl Row {
    /// A cell that is not `done` has empty metric fields.
    pub fn empty(arm: &str, seed: i64, status: CellStatus, run_id: Option<&str>) -> Self {
        Row {
            arm: arm.to_string(),
            seed,
            status,
            run_id: run_id.unwrap_or_default().to_string(),
            metrics: Default::default(),
            ddrs_sha: String::new(),
        }
    }

    /// Fill the metric columns from a cell's copied `manifest.json`.
    pub fn with_manifest(mut self, manifest: &serde_json::Value) -> Self {
        for (slot, key) in self.metrics.iter_mut().zip(METRIC_KEYS) {
            *slot = match manifest.get("metrics").and_then(|m| m.get(key)) {
                Some(v) if v.is_number() => v.to_string(),
                Some(v) if v.is_string() => v.as_str().unwrap_or_default().to_string(),
                _ => String::new(),
            };
        }
        self.ddrs_sha = manifest
            .get("git")
            .and_then(|g| g.get("sha"))
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        self
    }

    fn to_csv(&self) -> String {
        let mut fields = vec![
            self.arm.clone(),
            self.seed.to_string(),
            self.status.as_str().to_string(),
            self.run_id.clone(),
        ];
        fields.extend(self.metrics.iter().cloned());
        fields.push(self.ddrs_sha.clone());
        fields.join(",")
    }
}

pub fn render(rows: &[Row]) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for row in rows {
        out.push_str(&row.to_csv());
        out.push('\n');
    }
    out
}

pub fn write(path: &Path, rows: &[Row]) -> Result<String, Error> {
    let table = render(rows);
    crate::write_file(path, &table)?;
    Ok(table)
}
