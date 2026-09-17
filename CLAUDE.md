# CLAUDE.md — evenflow workspace

This repository is a cargo workspace. Each member crate carries its own
`CLAUDE.md`, skills, research notes, and tests; read the crate's file before
working in it:

- `crates/ddrs/CLAUDE.md` — routing, CLI, invariants, gates. Its four skills
  live at `crates/ddrs/.claude/skills/` and are listed with a `crates/ddrs:`
  prefix. "The repo's skills" in ddrs documentation means that directory.
- `crates/corduroy/README.md` — precipitation mapping.
- `crates/retrograde/src/lib.rs` — reserved, empty.

Workspace rules:

- `experiments/<name>/` holds paper experiments (see `experiments/README.md`):
  configs, pins, `AGENTS.md` data procedure, small results. Never data,
  checkpoints, or stores. Runs go through `crates/ddrs/` with an explicit
  `--workspace crates/ddrs/.ddrs`.

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
