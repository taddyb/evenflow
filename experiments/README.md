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

`crates/retrograde` owns the two verbs. Build both binaries once, then sweep
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
