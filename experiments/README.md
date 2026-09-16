# experiments

One directory per experiment that a paper, talk, or abstract will cite. The
directory is the reproducible unit: it holds the question, the exact config
of every arm, the data pins, the procedure to obtain the data, and the small
committed results. It never holds data, checkpoints, or stores.

```
experiments/<name>/
├── experiment.yaml        question, arms, expected metrics + tolerance, judgement
├── AGENTS.md              how to obtain the data (Zenodo DOIs, paths, checks)
├── CLAUDE.md              one line pointing at AGENTS.md
├── sources.lock           data fingerprints the arms ran against (copied from .ddrs/)
├── arms/<arm>/ddrs.yaml   exact config snapshot per arm, never a reference
└── results/               written by `retrograde sweep`, committed
    ├── summary.csv        one row per arm x seed
    └── <arm>/seed-<s>/    one cell: config.yaml, manifest.json, status.json
```

## Running one

`crates/retrograde` owns the verbs. Build both binaries once, then sweep
and check:

```bash
cargo build --release -p ddrs --bin ddrs
cargo build --release -p retrograde
target/release/retrograde sweep experiments/<name>/experiment.yaml --backend cpu
target/release/retrograde check experiments/<name>/experiment.yaml
```

`sweep` enumerates every arm x seed, derives a config per cell, and runs
`ddrs plan` then `ddrs run --workflow train-and-test --strict` for each one.
It resolves the workspace root from the nearest `Cargo.toml` with
`[workspace]`, defaults `--ddrs` to `target/release/ddrs` and the ddrs
workspace to `crates/ddrs/.ddrs`, and gives the child `crates/ddrs/` as its
working directory. An arm config may therefore use the repo-relative paths
ddrs itself uses. `--backend` is passed through to `ddrs run`; `--root`,
`--ddrs`, and `--workspace` override the defaults; `--only-failed` runs only
cells that are `failed` or `pending`.

A sweep is resumable. A `done` cell is skipped, so a second sweep over a
finished experiment spawns no ddrs run and returns in under a second. A cell
interrupted mid-run is left `running` and is reconciled on the next sweep
against its run directory. Exit 0 means every cell is `done`, 1 means at
least one is not, 2 is a hard error.

`check` reads only the committed files. It compares each `done` cell's
`manifest.json` against the arm's `expected:` block within that arm's
absolute `tolerance`, one line per arm x seed x metric, and ends with
`CHECK PASS` or `CHECK FAIL`. It needs no ddrs binary, no ddrs workspace, and
no arm config, so a fresh checkout can verify a committed result. An arm with
no `expected` entry is skipped; an arm in `expected` with no `done` cell
fails.

## What a cell holds

`results/<arm>/seed-<s>/` is three files:

| File | What it is |
|---|---|
| `config.yaml` | the arm config with `seed` and `np_seed` set to this cell's seed |
| `manifest.json` | copied from the ddrs run directory |
| `status.json` | `status`, `run_id`, `run_dir`, `log`, `started_at`, `finished_at`, `exit_code`, `error` |

`status.json`'s `run_dir` and `log` are recorded relative to the workspace
root (`crates/ddrs/.ddrs/runs/<run-id>`), so a committed cell points at the
same place on any clone; a run outside the root is recorded absolute.

`config.yaml` goes through a YAML round-trip, so it loses comments and
reformats flow sequences and float literals. It is the arm config plus the two
seeds and nothing else; a textual diff against `arms/<arm>/ddrs.yaml` shows
changes that are not substantive.

`results/summary.csv` carries one row per cell, sorted by arm then seed, with
these columns:

```
arm,seed,status,run_id,median_nse_finite,median_kge_finite,mean_nse_finite,n_gauges_finite_nse,n_gauges_total,ddrs_sha
```

Metric columns are empty for a cell that is not `done`.

## View, plot, notes

Three more verbs read what a sweep produced. They write nothing under
`experiments/` except an explicit `notes export`.

```bash
target/release/retrograde view
target/release/retrograde plot <run-id>
target/release/retrograde notes list <run-id>
target/release/retrograde notes export <run-id>
```

`view` serves a local page on `127.0.0.1:8787` (`--port` to change it). The
feed is one card per experiment, with its question, a `CHECK PASS` /
`CHECK FAIL` badge, and a row per cell, above a table of every run in the
ddrs workspace. `/run/<run-id>` is that run's profile: metrics, the source
fingerprints and whether they have drifted since, the `config.yaml` snapshot,
the plots, the last 200 lines of `run.log`, and the notes. It binds localhost
only and reads files already on disk. Bootstrap is vendored, so the pages
work offline.

`plot <run-id>` fills the Plots card. It hands the run directory to the uv
project at `crates/retrograde/py/`, which writes
`<run-dir>/plots/hydrograph-<gauge>.png` (observed, the summed-Q' baseline,
and the routed prediction, titled with that gauge's routed NSE and KGE) and
`<run-dir>/plots/metrics.png` (median NSE and KGE, routed against baseline).
The numbers come from the manifests `check` already reads, so a chart cannot
disagree with a check. It needs `uv` on PATH; `--max-gauges` defaults to 12.

Notes are typed into the form at the bottom of a profile page and stored in
`.retrograde/notes.sqlite` at the workspace root, which is gitignored. That
store is local until you promote it: `notes export <run-id>` writes
`results/<arm>/seed-<s>/notes.md` in the cell that claims the run, and that
file is what gets committed. A run no cell claims prints the markdown instead
of writing it.

## Reproduce a past run

A run that no experiment claims still has a record: its `manifest.json` and
the `config.yaml` snapshot beside it. `reproduce` reads that record, re-runs
the same config, and compares the new metrics to the recorded ones.

```bash
target/release/retrograde reproduce <run-id> --backend cpu --dry-run
target/release/retrograde reproduce <run-id> --backend cpu
target/release/retrograde reproduce experiments/<name>/results/<arm>/seed-<s>/manifest.json --backend cpu
```

The target is a run id in the ddrs workspace, or a path to a `manifest.json`,
or the directory holding one. A committed cell and a live run directory are
both valid records, so a reproduction can be reached through git alone.

The report has five sections. `record` says what is being reproduced and
refuses a run whose `status` is not `ok`. `code` compares the record's ddrs
sha against the `crates/ddrs` submodule now, and says when they differ that
the question has changed. `sources` runs `ddrs plan` and prints ddrs's own
verdict on whether the data moved; drift stops the reproduction unless
`--allow-drift`. `run` invokes `ddrs run` with the record's workflow. `compare`
lists every metric in both manifests against `--tolerance`, and the last line
is `REPRODUCED` or `NOT REPRODUCED`. Keys ending `_seconds` are printed with
their difference but do not vote, because a run's duration measures the
machine and not the model. Exit 0 means reproduced, 1 means not, 2 is a hard
error, 4 is unallowed drift. `--dry-run` stops after `sources`.

Everything written goes to `<workspace>/reproductions/<original-run-id>/`:
the copied `config.yaml`, the rendered `report.txt`, and the new run's
`manifest.json`. Nothing is written next to the record, so reproducing a
committed cell leaves git untouched. `view` reads that directory and shows
the verdict and the report on the original run's profile page.

### Pass the backend the original run used

ddrs does not record which backend executed a run. The manifest's `system`
probe is blank on a CPU run, and `backend` appears only inside the smoke-test
block, so `reproduce` cannot infer the device from the record. CPU and CUDA
produce different numbers: reproducing the Juniata record on CUDA diverges at
epoch 1, and on CPU every metric returns bit-identical. Pass `--backend`
matching the original run. The fix is for ddrs to record the backend in the
manifest, at which point `reproduce` can default to it and warn on a
mismatch.

## Rules

- Paper data lives on Zenodo; test fixtures live in git; nothing large here.
- `AGENTS.md` names DOIs and paths and points at `sources.lock`; it never
  repeats a hash.
- `expected` in `experiment.yaml` is copied from a cell's `manifest.json`
  when results are committed. `result` and `conclusion` are written by a
  person, never generated.
- `sources.lock` is copied out of the ddrs workspace by the sweep. It
  fingerprints each source path, but for a directory-backed store it does not
  hash the whole tree, so pin such data by a submodule commit or a published
  checksum as well.

## A worked example

`juniata-repro/` reproduces the committed Juniata bundle through this layer.
One arm, one seed, CPU, 21 s wall. `check` reports:

```
kan 42 median_nse_finite 0.79  0.790345 0.000345 PASS
kan 42 median_kge_finite 0.881 0.88097  0.00003  PASS
CHECK PASS
```

Start a new experiment by copying `_template/`.
