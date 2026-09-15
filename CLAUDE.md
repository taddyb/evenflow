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
- Provenance: ddrs was imported from `taddyb/ddrs@d86f8c7`, corduroy from
  `DeepGroundwater/corduroy@bb36dd9`. `git blame` starts at the import
  commit; the archived source repos hold earlier history.
