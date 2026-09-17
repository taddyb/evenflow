"""`plot_run.py` against a synthetic two-gauge run directory.

The fixture is written the way ddrs writes a real run — a zarr v3 group of
`float64 (n_gauges, n_days)` predictions/observations beside a `uint8
(n_gauges, 8)` character matrix of gage ids and an `int64` nanoseconds-since-
epoch time axis, plus raw little-endian `f32` baseline arrays and the two
manifests — so a change in that layout fails here and not only on the
workstation.
"""

import json
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest
import zarr

import plot_run

GAGE_IDS = ["01567000", "01555500"]
N_DAYS = 400
EVAL_START = np.datetime64("1995-10-02", "ns")
BASE_START = np.datetime64("1995-10-01", "ns")


def synthetic_series(n_gauges: int, n_days: int, offset: float) -> np.ndarray:
    """Smooth, strictly positive hydrograph-ish series, one row per gauge."""
    t = np.arange(n_days, dtype=np.float64)
    rows = [
        10.0 * (g + 1) + offset + 5.0 * np.sin(t / 20.0 + g)
        for g in range(n_gauges)
    ]
    return np.vstack(rows)


def write_run(run_dir: Path, gage_ids=GAGE_IDS, n_days: int = N_DAYS) -> None:
    n = len(gage_ids)
    obs = synthetic_series(n, n_days, 0.0)
    pred = synthetic_series(n, n_days, 0.6)

    zarr_path = run_dir / "eval" / "predictions.zarr"
    zarr_path.parent.mkdir(parents=True, exist_ok=True)
    group = zarr.open_group(str(zarr_path), mode="w", zarr_format=3)
    group.attrs.update(
        {
            "description": "Predictions and obs for time period",
            "start time": "1995/10/01",
            "end time": "1996/11/03",
        }
    )
    for name, values in (("predictions", pred), ("observations", obs)):
        arr = group.create_array(name, shape=values.shape, dtype="float64")
        arr[:] = values
        arr.attrs["units"] = "m3/s"

    width = max(len(g) for g in gage_ids)
    chars = np.zeros((n, width), dtype=np.uint8)
    for i, gid in enumerate(gage_ids):
        raw = gid.encode("ascii")
        chars[i, : len(raw)] = np.frombuffer(raw, dtype=np.uint8)
    ids = group.create_array("gage_ids", shape=chars.shape, dtype="uint8")
    ids[:] = chars
    ids.attrs["_dtype_hint"] = f"|S{width}"

    stamps = (EVAL_START + np.arange(n_days) * np.timedelta64(1, "D")).astype("int64")
    time = group.create_array("time", shape=stamps.shape, dtype="int64")
    time[:] = stamps
    time.attrs["units"] = "nanoseconds since 1970-01-01"

    # Baseline: two more days than the eval window, as the real run has.
    base_days = n_days + 2
    base_obs = synthetic_series(n, base_days, 0.0).astype(np.float32)
    base_pred = synthetic_series(n, base_days, -1.4).astype(np.float32)
    baseline = run_dir / "baseline"
    baseline.mkdir(parents=True, exist_ok=True)
    base_pred.tofile(baseline / "predictions.f32")
    base_obs.tofile(baseline / "observations.f32")
    days = BASE_START + np.arange(base_days) * np.timedelta64(1, "D")
    (baseline / "manifest.json").write_text(
        json.dumps(
            {
                "n_gauges": n,
                "n_days": base_days,
                "gage_ids": list(gage_ids),
                "time_range_daily": [str(d.astype("datetime64[D]")) for d in days],
                "metrics": {
                    "nse": [0.69 + 0.01 * i for i in range(n)],
                    "kge": [0.81 + 0.01 * i for i in range(n)],
                },
            }
        )
    )

    (run_dir / "manifest.json").write_text(
        json.dumps(
            {
                "run_id": "2026-09-16T01-21-34Z-train-and-test",
                "status": "ok",
                "metrics": {
                    "median_nse_finite": 0.7903451323509216,
                    "median_kge_finite": 0.8809698224067688,
                },
            }
        )
    )


@pytest.fixture
def run_dir(tmp_path: Path) -> Path:
    d = tmp_path / "2026-09-16T01-21-34Z-train-and-test"
    d.mkdir()
    write_run(d)
    return d


def pngs(run_dir: Path) -> list[str]:
    return sorted(p.name for p in (run_dir / "plots").glob("*.png"))


def test_writes_a_hydrograph_per_gauge_and_a_metrics_chart(run_dir: Path) -> None:
    assert plot_run.main([str(run_dir)]) == 0
    assert pngs(run_dir) == [
        "hydrograph-01555500.png",
        "hydrograph-01567000.png",
        "metrics.png",
    ]
    for name in pngs(run_dir):
        png = run_dir / "plots" / name
        assert png.stat().st_size > 0
        # A real PNG, not an empty file with the right name.
        assert png.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"


def test_writes_nothing_outside_plots(run_dir: Path) -> None:
    before = {
        str(p.relative_to(run_dir)): p.stat().st_mtime_ns
        for p in run_dir.rglob("*")
        if p.is_file()
    }
    assert plot_run.main([str(run_dir)]) == 0
    after = {
        str(p.relative_to(run_dir)): p.stat().st_mtime_ns
        for p in run_dir.rglob("*")
        if p.is_file()
    }
    new = set(after) - set(before)
    assert all(n.startswith("plots/") for n in new), new
    assert {k: v for k, v in after.items() if k in before} == before


def test_max_gauges_caps_the_hydrographs(tmp_path: Path) -> None:
    d = tmp_path / "many"
    d.mkdir()
    write_run(d, gage_ids=[f"0150000{i}" for i in range(5)])
    assert plot_run.main([str(d), "--max-gauges", "2"]) == 0
    assert pngs(d) == [
        "hydrograph-01500000.png",
        "hydrograph-01500001.png",
        "metrics.png",
    ]


def test_default_cap_is_twelve(tmp_path: Path) -> None:
    d = tmp_path / "fifteen"
    d.mkdir()
    write_run(d, gage_ids=[f"015000{i:02d}" for i in range(15)], n_days=60)
    assert plot_run.main([str(d)]) == 0
    assert len([n for n in pngs(d) if n.startswith("hydrograph-")]) == 12


def test_a_hydrograph_title_carries_the_gauge_id_and_its_scores(run_dir: Path) -> None:
    # The scores in the title are the ones the reader computes from the eval
    # arrays, so assert against a recomputation rather than a magic number.
    group = zarr.open_group(str(run_dir / "eval" / "predictions.zarr"), mode="r")
    pred = np.asarray(group["predictions"][0], dtype=np.float64)
    obs = np.asarray(group["observations"][0], dtype=np.float64)
    assert plot_run.nse(pred, obs) == pytest.approx(
        1.0 - np.sum((pred - obs) ** 2) / np.sum((obs - obs.mean()) ** 2)
    )
    assert 0.0 < plot_run.kge(pred, obs) <= 1.0

    title = plot_run.hydrograph_title(GAGE_IDS[0], pred, obs)
    assert GAGE_IDS[0] in title
    assert f"NSE {plot_run.nse(pred, obs):.3f}" in title
    assert f"KGE {plot_run.kge(pred, obs):.3f}" in title


def test_a_title_says_n_a_for_a_score_it_cannot_compute() -> None:
    flat = np.ones(10)
    title = plot_run.hydrograph_title("01567000", flat, flat)
    assert "01567000" in title
    assert "NSE n/a" in title and "KGE n/a" in title


def test_scores_are_none_when_nothing_overlaps(run_dir: Path) -> None:
    nan = np.full(10, np.nan)
    assert plot_run.nse(nan, nan) is None
    assert plot_run.kge(nan, nan) is None
    # A flat observation has no variance to explain: NSE is undefined.
    assert plot_run.nse(np.ones(10), np.ones(10)) is None


def test_baseline_is_optional(tmp_path: Path) -> None:
    d = tmp_path / "no-baseline"
    d.mkdir()
    write_run(d)
    for name in ("predictions.f32", "observations.f32", "manifest.json"):
        (d / "baseline" / name).unlink()
    assert plot_run.main([str(d)]) == 0
    assert pngs(d) == [
        "hydrograph-01555500.png",
        "hydrograph-01567000.png",
        "metrics.png",
    ]


def test_a_missing_eval_zarr_is_a_clear_nonzero_exit(tmp_path: Path, capsys) -> None:
    d = tmp_path / "empty"
    d.mkdir()
    assert plot_run.main([str(d)]) != 0
    assert "predictions.zarr" in capsys.readouterr().err


def test_runs_as_a_script(run_dir: Path) -> None:
    script = Path(plot_run.__file__)
    done = subprocess.run(
        [sys.executable, str(script), str(run_dir)],
        capture_output=True,
        text=True,
    )
    assert done.returncode == 0, done.stderr
    assert (run_dir / "plots" / "metrics.png").is_file()


def test_a_short_gage_id_is_not_padded_into_its_filename(tmp_path: Path) -> None:
    # Ragged ids share one fixed-width char matrix, so the short one is
    # NUL-padded on disk and must not carry that padding into a filename.
    d = tmp_path / "ragged"
    d.mkdir()
    write_run(d, gage_ids=["01567000", "0155"])
    assert plot_run.main([str(d)]) == 0
    assert pngs(d) == [
        "hydrograph-0155.png",
        "hydrograph-01567000.png",
        "metrics.png",
    ]
