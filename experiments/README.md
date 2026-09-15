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
└── results/<arm>/         manifest.json + metrics.json + a few small figures
```

Rules:

- Paper data lives on Zenodo; test fixtures live in git; nothing large here.
- `AGENTS.md` names DOIs and paths and points at `sources.lock`; it never
  repeats a hash.
- `expected` in `experiment.yaml` is copied from `results/<arm>/manifest.json`
  when results are committed. `result` and `conclusion` are written by a
  person, never generated.
- Runs execute through the ddrs CLI from `crates/ddrs/` with an explicit
  `--workspace crates/ddrs/.ddrs` (gitignored inside the submodule) and
  `--config experiments/<name>/arms/<arm>/ddrs.yaml`.

Start a new experiment by copying `_template/`.
