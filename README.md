# evenflow

A cargo workspace for differentiable hydrologic prediction. One build, one
lockfile, one CI; crates that train together share one `burn` and therefore
one autograd tape.

| Crate | What it is | Docs |
|---|---|---|
| `crates/ddrs` | Distributed differentiable Muskingum-Cunge routing in Rust (BURN), with the `ddrs` CLI, KAN parameter head, training, evaluation, and paper studies. Imported from `taddyb/ddrs@d86f8c7`. | `crates/ddrs/README.md`, `crates/ddrs/CLAUDE.md` |
| `crates/ddrs/ddrs-py` | PyO3 bindings for ddrs: read-only CPU inference and parameter export. | `crates/ddrs/ddrs-py/README.md` |
| `crates/corduroy` | Adaptive-mesh precipitation mapping from ERA5 Zarr v3 on GCS, with a small overland-flow and channel model. Imported from `DeepGroundwater/corduroy@bb36dd9`. | `crates/corduroy/README.md` |
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

## Provenance

ddrs was imported from `taddyb/ddrs@d86f8c7` and corduroy from
`DeepGroundwater/corduroy@bb36dd9` on 2026-09-13. Design notes for the
workspace are local working documents and are not committed.
