# CLAUDE.md — evenflow workspace

This repository is a cargo workspace. Each member crate carries its own
`CLAUDE.md`, skills, research notes, and tests; read the crate's file before
working in it:

- `crates/ddrs/CLAUDE.md` — routing, CLI, invariants, gates. Its four skills
  live at `crates/ddrs/.claude/skills/` and are listed with a `crates/ddrs:`
  prefix. "The repo's skills" in ddrs documentation means that directory.
- `crates/corduroy/README.md` — precipitation mapping.
- `experiments/README.md` — `crates/retrograde`, the experiment operator
  (`sweep`, `check`, `view`, `plot`, `notes`, `reproduce`).

Workspace rules:

- `experiments/<name>/` holds paper experiments (see `experiments/README.md`):
  configs, pins, `AGENTS.md` data procedure, small results. Never data,
  checkpoints, or stores. Runs go through `crates/ddrs/` with an explicit
  `--workspace crates/ddrs/.ddrs`.

- `crates/retrograde` owns the experiment verbs. `retrograde sweep` runs an
  experiment's arms x seeds through the ddrs CLI; `retrograde check` compares
  the results against the experiment's `expected:` block. They write only
  under `experiments/<name>/results/` and that experiment's `sources.lock`.
  Never inside a submodule, a crate, or `target/`.

- Three more verbs read a run rather than produce one. `retrograde view`
  serves the experiment feed and a per-run profile page; it binds
  `127.0.0.1` only, never another interface, and Bootstrap is vendored under
  `crates/retrograde/assets/` so no page fetches a CDN. `retrograde plot
  <run-id>` writes PNGs into `<run-dir>/plots/` through the uv project at
  `crates/retrograde/py/`, and writes nowhere else. `retrograde notes` keeps
  per-run notes in `.retrograde/notes.sqlite` at the workspace root, which is
  gitignored and stays local: `retrograde notes export <run-id>` is the only
  thing that puts a note into git, as `notes.md` in the cell that claims the
  run. A note typed while browsing is not a research note; export the ones
  that are.

- `retrograde reproduce <target>` re-runs a past run from its record and
  compares the metrics. Everything it writes goes under
  `<workspace>/reproductions/<original-run-id>/`: the copied `config.yaml`,
  the `report.txt`, and the new run's `manifest.json`. It never writes next to
  the original record, so reproducing a committed experiment cell leaves git
  untouched. The new ddrs run itself lands in `<workspace>/runs/` like any
  other. Pass `--backend` matching the original run; ddrs does not record the
  device, and CPU and CUDA give different numbers.

- `[patch.crates-io]` and `[profile.release]` live only in the root
  `Cargo.toml`. Never add them to a member crate; cargo ignores them there.
- One `target/` at the root. `cargo test -p ddrs` runs with `crates/ddrs/` as
  the working directory, so crate-relative fixture paths keep working; run
  ddrs's release gates and scripts from `crates/ddrs/`.
- The research journal hooks in `.claude/settings.json` point at
  `crates/ddrs/scripts/journal.py`, which resolves `crates/ddrs/` as its root.
- Do not hoist `research/`, `docs/`, or skills to the root.
- Design and brainstorming documents (superpowers specs and plans) are local
  working notes under the gitignored `docs/superpowers/`; never commit them.
- Submodules: `crates/ddrs` and `crates/corduroy` are git submodules of
  `taddyb/ddrs` and `DeepGroundwater/corduroy`. `.gitmodules` records each path
  and URL, and the gitlink pins the exact commit; `git submodule status` shows
  the pins, and `git submodule update --init --recursive` checks them out after
  a clone. Each submodule keeps its own repository, history, blame, CI, and
  tags; nothing is archived. Never edit files inside a submodule from this
  repo; change them upstream and bump the pointer.
