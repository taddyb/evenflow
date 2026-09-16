//! A cell is one arm x seed: its directory, its derived config, its status.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::experiment::Experiment;
use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellStatus {
    Pending,
    Running,
    Done,
    Failed,
}

impl CellStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CellStatus::Pending => "pending",
            CellStatus::Running => "running",
            CellStatus::Done => "done",
            CellStatus::Failed => "failed",
        }
    }
}

/// `results/<arm>/seed-<s>/status.json`. Every field but `status` is
/// nullable; a hand-written or partial file reads back with the rest null.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub status: CellStatus,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub run_dir: Option<PathBuf>,
    #[serde(default)]
    pub log: Option<PathBuf>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            status: CellStatus::Pending,
            run_id: None,
            run_dir: None,
            log: None,
            started_at: None,
            finished_at: None,
            exit_code: None,
            error: None,
        }
    }
}

impl Status {
    /// A cell with no status file is `pending`.
    pub fn read(path: &Path) -> Result<Self, Error> {
        if !path.is_file() {
            return Ok(Status::default());
        }
        let text = fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| Error::Json {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn write(&self, path: &Path) -> Result<(), Error> {
        let text = serde_json::to_string_pretty(self).map_err(|source| Error::Json {
            path: path.to_path_buf(),
            source,
        })?;
        crate::write_file(path, &(text + "\n"))
    }
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub arm: String,
    pub seed: i64,
    /// `<experiment dir>/results/<arm>/seed-<s>/`
    pub dir: PathBuf,
    /// The arm's source config, absolute.
    pub arm_config: PathBuf,
}

impl Cell {
    pub fn status_path(&self) -> PathBuf {
        self.dir.join("status.json")
    }

    /// The derived config the ddrs child is pointed at.
    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config.yaml")
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json")
    }

    pub fn label(&self) -> String {
        format!("{}/seed-{}", self.arm, self.seed)
    }
}

/// Every arm x seed of an experiment, sorted by arm then seed — the order a
/// sweep runs them in and the order `summary.csv` and `check` list them in.
pub fn cells(exp_dir: &Path, experiment: &Experiment) -> Result<Vec<Cell>, Error> {
    let mut cells = Vec::new();
    for arm in &experiment.arms {
        for seed in arm.seed_list()? {
            cells.push(Cell {
                arm: arm.name.clone(),
                seed,
                dir: exp_dir
                    .join("results")
                    .join(&arm.name)
                    .join(format!("seed-{seed}")),
                arm_config: exp_dir.join(&arm.config),
            });
        }
    }
    cells.sort_by(|a, b| (a.arm.as_str(), a.seed).cmp(&(b.arm.as_str(), b.seed)));
    Ok(cells)
}

/// Load the arm config as an opaque YAML mapping, set the two top-level seed
/// keys, and render it back. Comments are lost; the arm config keeps them.
pub fn derive_config(arm_config: &Path, seed: i64) -> Result<String, Error> {
    let text = fs::read_to_string(arm_config).map_err(|source| Error::Io {
        path: arm_config.to_path_buf(),
        source,
    })?;
    let mut value: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|source| Error::Yaml {
            path: arm_config.to_path_buf(),
            source,
        })?;
    let map = value.as_mapping_mut().ok_or_else(|| {
        Error::Invalid(format!(
            "{}: an arm config must be a YAML mapping",
            arm_config.display()
        ))
    })?;
    // `insert` on an existing key keeps its position, so the rest of the
    // file's key order is untouched.
    map.insert(
        serde_yaml::Value::from("seed"),
        serde_yaml::Value::from(seed),
    );
    map.insert(
        serde_yaml::Value::from("np_seed"),
        serde_yaml::Value::from(seed),
    );
    serde_yaml::to_string(&value).map_err(|source| Error::Yaml {
        path: arm_config.to_path_buf(),
        source,
    })
}
