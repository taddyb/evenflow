# evenflow

A cargo workspace for differentiable hydrologic prediction. One build, one
lockfile, one CI; crates that train together share one `burn` and therefore
one autograd tape.

| Crate | What it is | Docs |
|---|---|---|
| `crates/ddrs` | Distributed differentiable Muskingum-Cunge routing in Rust (BURN), with the `ddrs` CLI, KAN parameter head, training, evaluation, and paper studies. Git submodule of `taddyb/ddrs`. | `crates/ddrs/README.md`, `crates/ddrs/CLAUDE.md` |
| `crates/ddrs/ddrs-py` | PyO3 bindings for ddrs: read-only CPU inference and parameter export. | `crates/ddrs/ddrs-py/README.md` |
| `crates/corduroy` | Adaptive-mesh precipitation mapping from ERA5 Zarr v3 on GCS, with a small overland-flow and channel model. Git submodule of `DeepGroundwater/corduroy`. | `crates/corduroy/README.md` |
| `crates/retrograde` | Reserved name for the evenflow operator (model-state resources + reconcile). Empty. | `crates/retrograde/src/lib.rs` |

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
