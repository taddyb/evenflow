# Obtaining the data for dalton-lecture

Machine-readable pins are in `sources.lock` beside this file. This document
names where the bytes come from and how to check them; it never repeats a
hash.

## 1. Records

| Dataset | Zenodo version DOI | Files | Unpack to |
|---|---|---|---|
| <name> | 10.5281/zenodo.<version> | <file.tar> | <path the source group names> |

## 2. Unpack

```bash
# one block per record; icechunk stores are tarballs of the repository dir
tar -xf <file.tar> -C <parent dir>
```

## 3. Select the source group

```bash
cd crates/ddrs && ../../target/release/ddrs --workspace .ddrs sources use <group>
```

## 4. Check the pins

```bash
cp ../../experiments/dalton-lecture/sources.lock .ddrs/sources.lock
../../target/release/ddrs --workspace .ddrs plan --strict --workflow train-and-test
```

Drift means the wrong bytes. Stop and compare against the Zenodo md5.

## 5. Run an arm

```bash
../../target/release/ddrs --workspace .ddrs \
  --config ../../experiments/dalton-lecture/arms/<arm>/ddrs.yaml \
  run --workflow train-and-test
```
