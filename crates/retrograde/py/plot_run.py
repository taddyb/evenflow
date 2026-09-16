#!/usr/bin/env python3
"""Plot a ddrs run: one hydrograph per gauge, plus a metrics bar chart.

`retrograde view` serves `<run_dir>/plots/*.png` on a run's profile page;
this is what puts them there. Everything is read out of the run directory
ddrs already wrote, and nothing outside `<run_dir>/plots/` is touched.

What is read, as ddrs writes it (verified against
`.ddrs/runs/2026-09-16T01-21-34Z-train-and-test/`):

    <run_dir>/eval/predictions.zarr     zarr v3 group
        predictions   float (n_gauges, n_days)   routed, m3/s
        observations  float (n_gauges, n_days)   USGS daily, m3/s
        gage_ids      uint8 (n_gauges, width)    a |S<width> char matrix
        time          int64 (n_days,)            ns since 1970-01-01
    <run_dir>/baseline/predictions.f32  raw little-endian f32, row-major
    <run_dir>/baseline/observations.f32 same shape
    <run_dir>/baseline/manifest.json    n_gauges, n_days, gage_ids,
                                        time_range_daily, metrics
    <run_dir>/manifest.json             metrics.median_{nse,kge}_finite

The baseline is the summed-Q' reference: no routing, no learned parameters.
Its window is the full eval period while the routed window drops the warmup
days, so the two series are plotted against their own dates rather than
being forced onto one index. The baseline is optional — a `train`-only run
has none, and the hydrographs then carry two lines instead of three.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path

import matplotlib

matplotlib.use("Agg")  # No display anywhere this runs: headless, file output only.

import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402
import zarr  # noqa: E402

DEFAULT_MAX_GAUGES = 12

ROUTED_COLOR = "#1f4e99"
BASELINE_COLOR = "#e07b39"
OBSERVED_COLOR = "#444444"


class PlotError(Exception):
    """Something the run directory does not have. Reported, not traced."""


@dataclass
class Eval:
    """The routed eval window, straight out of `eval/predictions.zarr`."""

    gage_ids: list[str]
    time: np.ndarray  # datetime64[ns], (n_days,)
    predictions: np.ndarray  # float64, (n_gauges, n_days)
    observations: np.ndarray  # float64, (n_gauges, n_days)


@dataclass
class Baseline:
    """The summed-Q' reference over the full eval period."""

    gage_ids: list[str]
    time: np.ndarray  # datetime64[D], (n_days,)
    predictions: np.ndarray  # float32, (n_gauges, n_days)
    metrics: dict  # per-gauge lists, keyed by metric name


def decode_gage_ids(raw: np.ndarray) -> list[str]:
    """`uint8 (n_gauges, width)` back into strings.

    ddrs stores gage ids as a fixed-width character matrix (`_dtype_hint:
    |S8`) because zarr v3 has no ragged string type here. Trailing NULs pad
    an id shorter than the widest one.
    """
    ids = []
    for row in np.atleast_2d(np.asarray(raw, dtype=np.uint8)):
        ids.append(row.tobytes().rstrip(b"\x00").decode("utf-8", "replace").strip())
    return ids


def read_eval(run_dir: Path) -> Eval:
    path = run_dir / "eval" / "predictions.zarr"
    if not path.is_dir():
        raise PlotError(
            f"no eval/predictions.zarr in {run_dir} — "
            "this run has no eval phase to plot "
            "(`ddrs run --workflow train-and-test` writes one)"
        )
    group = zarr.open_group(str(path), mode="r")
    try:
        predictions = np.asarray(group["predictions"][:], dtype=np.float64)
        observations = np.asarray(group["observations"][:], dtype=np.float64)
        gage_ids = decode_gage_ids(group["gage_ids"][:])
        time = np.asarray(group["time"][:], dtype="int64").astype("datetime64[ns]")
    except KeyError as missing:
        raise PlotError(f"{path}: no such array {missing}") from missing

    predictions = np.atleast_2d(predictions)
    observations = np.atleast_2d(observations)
    if len(gage_ids) != predictions.shape[0]:
        raise PlotError(
            f"{path}: {len(gage_ids)} gage ids but "
            f"{predictions.shape[0]} rows of predictions"
        )
    return Eval(gage_ids, time, predictions, observations)


def read_baseline(run_dir: Path) -> Baseline | None:
    """The summed-Q' reference, or None when the run has none."""
    base = run_dir / "baseline"
    manifest_path = base / "manifest.json"
    predictions_path = base / "predictions.f32"
    if not manifest_path.is_file() or not predictions_path.is_file():
        return None

    manifest = json.loads(manifest_path.read_text())
    n_gauges = int(manifest["n_gauges"])
    n_days = int(manifest["n_days"])
    flat = np.fromfile(predictions_path, dtype="<f4")
    if flat.size != n_gauges * n_days:
        raise PlotError(
            f"{predictions_path}: {flat.size} f32 values, but the manifest "
            f"says {n_gauges} x {n_days} = {n_gauges * n_days}"
        )
    time = np.asarray(manifest["time_range_daily"], dtype="datetime64[D]")
    return Baseline(
        gage_ids=[str(g) for g in manifest["gage_ids"]],
        time=time,
        predictions=flat.reshape(n_gauges, n_days),
        metrics=manifest.get("metrics", {}),
    )


def read_metrics(run_dir: Path) -> dict:
    """`manifest.json`'s `metrics` object, or `{}` for a run without one."""
    path = run_dir / "manifest.json"
    if not path.is_file():
        return {}
    return json.loads(path.read_text()).get("metrics", {}) or {}


def _paired(sim: np.ndarray, obs: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """The days where both series are finite."""
    both = np.isfinite(sim) & np.isfinite(obs)
    return sim[both], obs[both]


def nse(sim: np.ndarray, obs: np.ndarray) -> float | None:
    """Nash-Sutcliffe efficiency, or None when it is undefined."""
    sim, obs = _paired(np.asarray(sim, float), np.asarray(obs, float))
    if sim.size < 2:
        return None
    denominator = float(np.sum((obs - obs.mean()) ** 2))
    if denominator == 0.0:
        return None  # A flat observation has no variance to explain.
    return 1.0 - float(np.sum((sim - obs) ** 2)) / denominator


def kge(sim: np.ndarray, obs: np.ndarray) -> float | None:
    """Kling-Gupta efficiency (2009), or None when it is undefined."""
    sim, obs = _paired(np.asarray(sim, float), np.asarray(obs, float))
    if sim.size < 2:
        return None
    sim_std, obs_std = float(sim.std()), float(obs.std())
    obs_mean = float(obs.mean())
    if obs_std == 0.0 or obs_mean == 0.0 or sim_std == 0.0:
        return None
    r = float(np.corrcoef(sim, obs)[0, 1])
    if not np.isfinite(r):
        return None
    alpha = sim_std / obs_std
    beta = float(sim.mean()) / obs_mean
    return 1.0 - float(np.sqrt((r - 1) ** 2 + (alpha - 1) ** 2 + (beta - 1) ** 2))


def safe_name(gage_id: str) -> str:
    """A gage id as one path component.

    Global ids are `Provider__GageId`, which is already safe; this is here
    so that a stray separator cannot aim the output at another directory,
    and because `retrograde view` only serves single-component `.png` names.
    """
    cleaned = "".join(c if (c.isalnum() or c in "-_.") else "_" for c in gage_id)
    return cleaned or "unnamed"


def score_label(sim: np.ndarray, obs: np.ndarray) -> str:
    scores = []
    for name, value in (("NSE", nse(sim, obs)), ("KGE", kge(sim, obs))):
        scores.append(f"{name} {value:.3f}" if value is not None else f"{name} n/a")
    return ", ".join(scores)


def hydrograph_title(gage_id: str, sim: np.ndarray, obs: np.ndarray) -> str:
    """The gauge and how the routed series scored against its observation.

    A gauge whose observation is all-NaN or flat has no score, and says so
    rather than showing a number that is not one.
    """
    return f"gauge {gage_id}: routed {score_label(sim, obs)}"


def hydrograph(
    out: Path,
    gage_id: str,
    evaluation: Eval,
    index: int,
    baseline: Baseline | None,
) -> Path:
    routed = evaluation.predictions[index]
    observed = evaluation.observations[index]

    figure, axes = plt.subplots(figsize=(11, 4), layout="constrained")
    axes.plot(
        evaluation.time, observed, color=OBSERVED_COLOR, lw=0.8, label="observed (USGS)"
    )

    # Baseline under routed: the two track each other closely on a good
    # gauge, and the routed line is the one the page is about.
    if baseline is not None and gage_id in baseline.gage_ids:
        row = baseline.gage_ids.index(gage_id)
        label = "summed Q' baseline"
        scores = []
        for name in ("nse", "kge"):
            values = baseline.metrics.get(name)
            if values is not None and row < len(values) and np.isfinite(values[row]):
                scores.append(f"{name.upper()} {float(values[row]):.3f}")
        if scores:
            label = f"{label} ({', '.join(scores)})"
        axes.plot(
            baseline.time.astype("datetime64[ns]"),
            baseline.predictions[row],
            color=BASELINE_COLOR,
            lw=0.8,
            alpha=0.85,
            label=label,
        )

    axes.plot(evaluation.time, routed, color=ROUTED_COLOR, lw=0.9, label="routed")

    axes.set_title(hydrograph_title(gage_id, routed, observed))
    axes.set_ylabel("discharge (m$^3$/s)")
    axes.grid(alpha=0.25, lw=0.5)
    axes.legend(loc="upper right", fontsize=8, framealpha=0.9)

    path = out / f"hydrograph-{safe_name(gage_id)}.png"
    figure.savefig(path, dpi=130)
    plt.close(figure)
    return path


def median_of(values) -> float | None:
    array = np.asarray(values, dtype=np.float64).ravel()
    array = array[np.isfinite(array)]
    return float(np.median(array)) if array.size else None


def metrics_chart(out: Path, run_metrics: dict, baseline: Baseline | None) -> Path:
    """Median NSE and KGE, routed vs the summed-Q' baseline.

    Both numbers come from the manifests that already computed them — the
    run's `median_*_finite` and the baseline's per-gauge arrays — so this
    chart cannot disagree with `retrograde check`.
    """
    routed = {
        "NSE": median_of(run_metrics.get("median_nse_finite", [])),
        "KGE": median_of(run_metrics.get("median_kge_finite", [])),
    }
    reference = {"NSE": None, "KGE": None}
    if baseline is not None:
        reference = {
            "NSE": median_of(baseline.metrics.get("nse", [])),
            "KGE": median_of(baseline.metrics.get("kge", [])),
        }

    names = ["NSE", "KGE"]
    x = np.arange(len(names), dtype=np.float64)
    figure, axes = plt.subplots(figsize=(5.5, 4), layout="constrained")
    for offset, (label, source, color) in enumerate(
        (
            ("routed", routed, ROUTED_COLOR),
            ("summed Q' baseline", reference, BASELINE_COLOR),
        )
    ):
        heights = [0.0 if source[n] is None else source[n] for n in names]
        bars = axes.bar(x + (offset - 0.5) * 0.36, heights, 0.34, label=label, color=color)
        for bar, name in zip(bars, names):
            value = source[name]
            axes.annotate(
                "n/a" if value is None else f"{value:.3f}",
                (bar.get_x() + bar.get_width() / 2, bar.get_height()),
                textcoords="offset points",
                xytext=(0, 3 if bar.get_height() >= 0 else -11),
                ha="center",
                fontsize=8,
            )

    axes.set_xticks(x, names)
    axes.set_ylabel("median over gauges")
    axes.set_title("median efficiency: routed vs baseline")
    axes.axhline(0.0, color="#888888", lw=0.6)
    axes.grid(axis="y", alpha=0.25, lw=0.5)
    # Below the axes: the bars reach the top of the frame, and a legend in
    # the corner would sit on the value annotations.
    axes.legend(fontsize=8, ncols=2, loc="upper center", bbox_to_anchor=(0.5, -0.07))

    path = out / "metrics.png"
    figure.savefig(path, dpi=130)
    plt.close(figure)
    return path


def plot(run_dir: Path, max_gauges: int) -> list[Path]:
    evaluation = read_eval(run_dir)
    baseline = read_baseline(run_dir)
    run_metrics = read_metrics(run_dir)

    out = run_dir / "plots"
    out.mkdir(parents=True, exist_ok=True)

    written = [
        hydrograph(out, gage_id, evaluation, index, baseline)
        for index, gage_id in enumerate(evaluation.gage_ids[:max_gauges])
    ]
    written.append(metrics_chart(out, run_metrics, baseline))
    return written


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="plot_run.py",
        description="Plot a ddrs run's hydrographs and metrics into <run_dir>/plots/.",
    )
    parser.add_argument("run_dir", type=Path, help="a .ddrs/runs/<id> directory")
    parser.add_argument(
        "--max-gauges",
        type=int,
        default=DEFAULT_MAX_GAUGES,
        help=f"how many hydrographs to draw, in gauge order (default {DEFAULT_MAX_GAUGES})",
    )
    args = parser.parse_args(argv)

    if args.max_gauges < 1:
        print("plot_run: --max-gauges must be at least 1", file=sys.stderr)
        return 2
    if not args.run_dir.is_dir():
        print(f"plot_run: no such run directory: {args.run_dir}", file=sys.stderr)
        return 2

    try:
        written = plot(args.run_dir, args.max_gauges)
    except PlotError as error:
        print(f"plot_run: {error}", file=sys.stderr)
        return 1

    for path in written:
        print(path)
    total = len(written) - 1
    print(f"plot_run: {total} hydrograph(s) + metrics.png in {args.run_dir / 'plots'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
