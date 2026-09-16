# Obtaining the data for juniata-repro

Machine-readable pins are in `sources.lock` beside this file. This document
names where the bytes come from and how to check them; it never repeats a
hash.

## 1. Records

There is no Zenodo record for this experiment. Its data is the committed
ddrs test bundle under `crates/ddrs/examples/juniata/data/` (8.9 MB, in git
inside the `crates/ddrs` submodule): attributes NetCDF, the two adjacency
zarr stores, the Q' and observation icechunk stores, the one-row gauge CSV,
and the attribute normalization statistics.

| Dataset | Source | Files | Unpack to |
|---|---|---|---|
| Juniata single-catchment bundle | `crates/ddrs` submodule (git) | `examples/juniata/data/` | already in place |

## 2. Unpack

Nothing to unpack. Check out the submodule:

```bash
git submodule update --init --recursive
```

## 3. Select the source group

Not applicable. The arm config names the bundle's paths directly
(`examples/juniata/data/...`, relative to `crates/ddrs`), so no
`ddrs sources use` step is required.

## 4. Check the pins

```bash
cd crates/ddrs
cp ../../experiments/juniata-repro/sources.lock .ddrs/sources.lock
../../target/release/ddrs --workspace .ddrs \
  --config ../../experiments/juniata-repro/arms/kan/ddrs.yaml \
  plan --strict --workflow train-and-test
```

Drift means the wrong bytes. Stop and compare against the submodule commit
this experiment was run at (`git -C crates/ddrs rev-parse HEAD`).

## 5. Run an arm

```bash
cd crates/ddrs && ../../target/release/ddrs --workspace .ddrs \
  --config ../../experiments/juniata-repro/arms/kan/ddrs.yaml \
  run --workflow train-and-test --backend cpu
```

Or, the way the committed results were produced, from the workspace root:

```bash
target/release/retrograde sweep experiments/juniata-repro/experiment.yaml --backend cpu
target/release/retrograde check experiments/juniata-repro/experiment.yaml
```
