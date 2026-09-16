//! The `experiment.yaml` schema retrograde reads (v1).
//!
//! Only the keys retrograde acts on are modelled. Everything else in the
//! file (`question`, `expected`, `result`, `conclusion`, and anything a
//! person adds) is ignored on read and never rewritten: retrograde only
//! ever writes under `results/`, plus `sources.lock`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct Experiment {
    pub name: String,
    #[serde(default)]
    pub arms: Vec<Arm>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Arm {
    pub name: String,
    /// Relative to the experiment directory.
    pub config: PathBuf,
    #[serde(default)]
    seeds: Option<Vec<i64>>,
    #[serde(default)]
    seed: Option<i64>,
}

impl Arm {
    /// `seeds: [1, 2]` is the general form; the scalar `seed: 42` is also
    /// accepted (it is what `experiments/_template/experiment.yaml` uses).
    pub fn seed_list(&self) -> Result<Vec<i64>, Error> {
        match (&self.seeds, self.seed) {
            (Some(s), _) if !s.is_empty() => Ok(s.clone()),
            (_, Some(s)) => Ok(vec![s]),
            _ => Err(Error::Invalid(format!(
                "arm {:?} has no seeds: give it `seeds: [..]` or `seed: <n>`",
                self.name
            ))),
        }
    }
}

impl Experiment {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_yaml::from_str(&text).map_err(|source| Error::Yaml {
            path: path.to_path_buf(),
            source,
        })
    }
}
