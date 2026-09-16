# evenflow

A cargo workspace for differentiable hydrologic prediction. One build, one
lockfile, one CI; crates that train together share one `burn` and therefore
one autograd tape.

| Crate | What it is | Docs |
|---|---|---|
| `crates/ddrs` | Distributed differentiable Muskingum-Cunge routing in Rust (BURN), with the `ddrs` CLI, KAN parameter head, training, evaluation, and paper studies. Git submodule of `taddyb/ddrs`. | `crates/ddrs/README.md`, `crates/ddrs/CLAUDE.md` |
| `crates/ddrs/ddrs-py` | PyO3 bindings for ddrs: read-only CPU inference and parameter export. | `crates/ddrs/ddrs-py/README.md` |
| `crates/corduroy` | Adaptive-mesh precipitation mapping from ERA5 Zarr v3 on GCS, with a small overland-flow and channel model. Git submodule of `DeepGroundwater/corduroy`. | `crates/corduroy/README.md` |
| `crates/retrograde` | The evenflow experiment operator. `sweep` runs an experiment's arms x seeds through ddrs; `check` compares the results against what the experiment claims; `view` serves a local page per experiment and run; `plot` draws a run's hydrographs and metrics; `notes` keeps notes on a run; `reproduce` re-runs a past run from its record. | `experiments/README.md` |

## Build

```bash
cargo build --workspace
cargo test  --workspace --features ddrs/fixtures
# ddrs release gates run from the crate:
cd crates/ddrs && cargo run --release --example compare_ddr_sandbox
```

On a host with CUDA 13.3+, put `CUDARC_CUDA_VERSION = "13020"` under `[env]`
in a gitignored `.cargo/config.toml` at the workspace root before the first
build (see `crates/ddrs/.claude/skills/ddrs-dev`, trap T12).

Python bindings are per-crate `uv` projects; nothing is shared at the root.

## Experiments

`experiments/<name>/` is the reproducible unit a paper cites: arm configs,
data pins, the data procedure in `AGENTS.md`, and small committed results.
See `experiments/README.md`. Data never lives in git.

Run one with `crates/retrograde`. Build the two binaries, then sweep, check,
plot, and view:

```bash
cargo build --release -p ddrs --bin ddrs
cargo build --release -p retrograde
target/release/retrograde sweep experiments/juniata-repro/experiment.yaml --backend cpu
target/release/retrograde check experiments/juniata-repro/experiment.yaml
target/release/retrograde plot <run-id>
target/release/retrograde view
target/release/retrograde reproduce <run-id> --backend cpu
```

`sweep` runs every arm x seed through `ddrs plan` and `ddrs run --workflow
train-and-test --strict`, writing `results/<arm>/seed-<s>/` and
`results/summary.csv`. It skips a cell that is already `done`, so a rerun
costs nothing. `check` compares those results against the experiment's
`expected:` block within its tolerance and prints `CHECK PASS` or
`CHECK FAIL`; it reads only committed files. `experiments/juniata-repro/` is
a worked example: the Juniata bundle at routed NSE 0.790 / KGE 0.881, 21 s on
CPU.

`reproduce` takes a run that no experiment claims and re-runs it from its
record: the `manifest.json` and the `config.yaml` snapshot beside it. It
reports the code and the sources it ran against, compares every metric to the
recorded one, and ends with `REPRODUCED` or `NOT REPRODUCED`. The target can
also be a committed cell's `manifest.json`, so a record in git is enough.
Pass `--backend` matching the original run, because ddrs does not record which
device executed it. Output goes to
`crates/ddrs/.ddrs/reproductions/<original-run-id>/`, never next to the
record.

`plot` writes a run's hydrographs and a routed-against-baseline metrics chart
into `<run-dir>/plots/`, using the uv project at `crates/retrograde/py/`.
`view` then serves those plots, the metrics, the source drift, the config
snapshot, the log tail, and a notes form on `127.0.0.1:8787`. Notes live in a
gitignored `.retrograde/notes.sqlite` until `retrograde notes export <run-id>`
writes them into the experiment cell that claims the run. See
`experiments/README.md`.

## Submodules

`crates/ddrs` and `crates/corduroy` are git submodules. `.gitmodules` records
each path and URL, and the gitlink in this repo pins the exact commit. Run
`git submodule status` to see the pins. After cloning:

```bash
git submodule update --init --recursive
```

Each submodule keeps its own repository, history, blame, CI, and tags. Nothing
is archived. Design notes for the workspace are local working documents and are
not committed.
