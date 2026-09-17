//! The `experiment.yaml` schema retrograde reads (v1).
//!
//! Only the keys retrograde acts on are modelled. Everything else in the
//! file (`question`, `result`, `conclusion`, and anything a person adds) is
//! ignored on read and never rewritten: retrograde only ever writes under
//! `results/`, plus `sources.lock`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct Experiment {
    pub name: String,
    #[serde(default)]
    pub arms: Vec<Arm>,
    /// `expected:` — an arm name per key, each mapping metric names to the
    /// value `check` compares against, plus a `tolerance`. Held as an
    /// opaque mapping so file order (and therefore report order) survives
    /// and an arm that names a metric retrograde has never heard of is
    /// still compared. Parsed by [`Experiment::expectations`].
    #[serde(default)]
    pub expected: serde_yaml::Mapping,
}

/// The key inside an arm's `expected` block that is the tolerance rather
/// than a metric.
pub const TOLERANCE_KEY: &str = "tolerance";

/// One arm's `expected` entry, parsed.
#[derive(Debug, Clone)]
pub struct Expectation {
    pub arm: String,
    /// Absolute, applied to every metric of this arm.
    pub tolerance: f64,
    /// Metric name -> expected value, in `experiment.yaml` order.
    pub metrics: Vec<(String, f64)>,
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
    /// The `expected:` block, in file order. An arm entry that is not a
    /// mapping, carries no `tolerance`, or gives a non-numeric value is an
    /// error: `check` must never invent a tolerance or skip a metric it
    /// cannot read.
    pub fn expectations(&self) -> Result<Vec<Expectation>, Error> {
        let mut out = Vec::new();
        for (key, value) in &self.expected {
            let arm = key.as_str().ok_or_else(|| {
                Error::Invalid("expected: every key must be an arm name".to_string())
            })?;
            let map = value.as_mapping().ok_or_else(|| {
                Error::Invalid(format!("expected.{arm}: must be a mapping of metric -> value"))
            })?;
            let mut tolerance = None;
            let mut metrics = Vec::new();
            for (mkey, mvalue) in map {
                let name = mkey.as_str().ok_or_else(|| {
                    Error::Invalid(format!("expected.{arm}: every key must be a metric name"))
                })?;
                let number = mvalue.as_f64().ok_or_else(|| {
                    Error::Invalid(format!("expected.{arm}.{name}: must be a number"))
                })?;
                if name == TOLERANCE_KEY {
                    tolerance = Some(number);
                } else {
                    metrics.push((name.to_string(), number));
                }
            }
            let tolerance = tolerance.ok_or_else(|| {
                Error::Invalid(format!(
                    "expected.{arm}: no `{TOLERANCE_KEY}` key — give the arm an absolute tolerance"
                ))
            })?;
            out.push(Expectation {
                arm: arm.to_string(),
                tolerance,
                metrics,
            });
        }
        Ok(out)
    }

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
