//! `retrograde view` driven through the axum router, no socket.
//!
//! Every test builds a tempdir holding an experiment directory (the
//! `tests/sweep.rs` fixture shape: `experiment.yaml` plus a `results/` tree
//! written by hand) and a ddrs workspace holding two runs — one finished
//! with metrics and a log, one still going — then calls the router with
//! `tower::ServiceExt::oneshot`. Nothing binds a port and nothing launches
//! ddrs.

use std::fs;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

const DONE_RUN: &str = "2026-09-15T00-00-01Z-train-and-test";
const LIVE_RUN: &str = "2026-09-15T06-00-00Z-train";

const LAST_LOG_LINE: &str = "[2026-09-15T00:10:00Z] run complete, wrote eval/predictions.zarr";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    /// An experiments tree with one experiment (`juniata-repro`, one arm,
    /// one seed, done) and a workspace with two runs.
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let workspace = root.join("crates/ddrs/.ddrs");

        let exp_dir = root.join("experiments/juniata-repro");
        write(
            &exp_dir.join("experiment.yaml"),
            r#"name: juniata-repro
question: >
  Does the committed Juniata bundle reproduce its documented result?
arms:
  - name: kan
    config: arms/kan/ddrs.yaml
    seeds: [42]
expected:
  kan:
    median_nse_finite: 0.79
    tolerance: 0.04
"#,
        );
        write(
            &exp_dir.join("results/kan/seed-42/status.json"),
            &format!(r#"{{"status": "done", "run_id": "{DONE_RUN}"}}"#),
        );
        write(
            &exp_dir.join("results/kan/seed-42/manifest.json"),
            &done_manifest(),
        );

        // The finished run: manifest, config snapshot, log.
        let done = workspace.join("runs").join(DONE_RUN);
        write(&done.join("manifest.json"), &done_manifest());
        write(
            &done.join("config.yaml"),
            "mode: training\nworkflow: train-and-test\nseed: 42\n",
        );
        write(
            &done.join("run.log"),
            &format!(
                "[2026-09-15T00:00:00Z] backend: cpu\n\
                 [2026-09-15T00:00:01Z] epoch 1 lr=0.001\n\
                 [2026-09-15T00:05:00Z] epoch 30 lr=0.001\n\
                 {LAST_LOG_LINE}\n"
            ),
        );

        // The run still going: finished_at null, no metrics.
        write(
            &workspace.join("runs").join(LIVE_RUN).join("manifest.json"),
            &format!(
                r#"{{
  "run_id": "{LIVE_RUN}",
  "ddrs_version": "0.1.0",
  "git": {{ "sha": "beef5678deadbeef", "dirty": true, "branch": "phase1c-view" }},
  "workflow": "train",
  "started_at": "2026-09-15T06:00:00Z",
  "finished_at": null,
  "status": "running",
  "sources": {{}},
  "outputs": {{ "checkpoints": [] }},
  "metrics": {{}}
}}"#
            ),
        );

        // The live lock: `attributes` matches the finished run's manifest,
        // `streamflow` has been re-fingerprinted since, and `gages` is
        // absent from the lock entirely.
        write(
            &workspace.join("sources.lock"),
            r#"{
  "ddrs_version": "0.1.0",
  "created_at": "2026-09-15T00:00:00Z",
  "sources": {
    "attributes": {
      "path": "data/attrs.nc", "mtime": "2026-09-14T22:47:00Z", "size": 32863,
      "fp": "blake3:1111111111111111111111111111111111111111111111111111111111111111"
    },
    "streamflow": {
      "path": "data/qprime.ic", "mtime": "2026-09-15T09:00:00Z", "size": 104,
      "fp": "blake3:9999999999999999999999999999999999999999999999999999999999999999"
    }
  }
}"#,
        );

        Fixture {
            _tmp: tmp,
            root,
            workspace,
        }
    }

    fn app(&self) -> Router {
        retrograde::view::app(&self.root, &self.workspace)
    }
}

/// The finished run's manifest: three sources (one matching the lock, one
/// drifted, one the lock has never heard of) and two metrics.
fn done_manifest() -> String {
    format!(
        r#"{{
  "run_id": "{DONE_RUN}",
  "ddrs_version": "0.1.0",
  "git": {{ "sha": "05ab47fa03f6584afad6f6310fec1c5680e2d04c", "dirty": false, "branch": "master" }},
  "workflow": "train-and-test",
  "started_at": "2026-09-15T00:00:00Z",
  "finished_at": "2026-09-15T00:10:00Z",
  "status": "ok",
  "exit_reason": null,
  "sources": {{
    "attributes": {{
      "path": "data/attrs.nc", "mtime": "2026-09-14T22:47:00Z", "size": 32863,
      "fp": "blake3:1111111111111111111111111111111111111111111111111111111111111111"
    }},
    "gages": {{
      "path": "data/gage.csv", "mtime": "2026-09-14T22:47:00Z", "size": 260,
      "fp": "blake3:2222222222222222222222222222222222222222222222222222222222222222"
    }},
    "streamflow": {{
      "path": "data/qprime.ic", "mtime": "2026-09-14T22:47:00Z", "size": 102,
      "fp": "blake3:3333333333333333333333333333333333333333333333333333333333333333"
    }}
  }},
  "outputs": {{ "checkpoints": [], "run_log": "run.log" }},
  "metrics": {{
    "median_nse_finite": 0.7903451323509216,
    "median_kge_finite": 0.8809698224067688,
    "n_gauges_total": 1
  }}
}}"#
    )
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, contents).expect("write");
}

/// GET `uri`, returning the status and the body as a string.
async fn get(app: Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// GET `uri`, returning the status and the `content-type` header.
async fn content_type(app: Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let ct = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    (status, ct)
}

/// The feed's runs table, so a row lookup cannot land in the experiments
/// section above it.
fn runs_table(html: &str) -> &str {
    let at = html.find(r#"id="runs""#).expect("no runs section");
    &html[at..]
}

/// The `<tr>` of a table that mentions `needle` — so a badge can be
/// asserted against the row it belongs to, not the page as a whole.
fn row_containing<'a>(html: &'a str, needle: &str) -> &'a str {
    let at = html
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is nowhere in the page"));
    let start = html[..at]
        .rfind("<tr")
        .unwrap_or_else(|| panic!("no <tr> before {needle:?}"));
    let end = html[at..]
        .find("</tr>")
        .map(|e| at + e)
        .unwrap_or_else(|| panic!("no </tr> after {needle:?}"));
    &html[start..end]
}

#[tokio::test]
async fn feed_lists_both_runs_and_marks_the_unfinished_one() {
    let fx = Fixture::new();
    let (status, body) = get(fx.app(), "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(DONE_RUN), "feed is missing the finished run");
    assert!(body.contains(LIVE_RUN), "feed is missing the running run");
    assert!(
        body.contains(&format!("/run/{DONE_RUN}")),
        "the run id should link to its profile"
    );

    // The badge belongs to the unfinished run's row only.
    let runs = runs_table(&body);
    let live = row_containing(runs, LIVE_RUN);
    assert!(live.contains("in progress"), "no in-progress badge: {live}");
    let done = row_containing(runs, DONE_RUN);
    assert!(
        !done.contains("in progress"),
        "the finished run must not be marked in progress: {done}"
    );

    // The finished run's table row carries its metrics and short sha.
    assert!(
        done.contains("0.790"),
        "no median NSE in the run row: {done}"
    );
    assert!(done.contains("05ab47f"), "no 7-char ddrs sha: {done}");
    assert!(
        !done.contains("05ab47fa03f6"),
        "the sha should be abbreviated to 7 chars: {done}"
    );
}

#[tokio::test]
async fn feed_shows_the_experiment_card_with_its_cell() {
    let fx = Fixture::new();
    let (status, body) = get(fx.app(), "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("juniata-repro"), "no experiment card");
    assert!(
        body.contains("Does the committed Juniata bundle reproduce"),
        "the card should show the experiment's question"
    );
    assert!(body.contains("kan"), "no arm name on the card");
    assert!(body.contains("seed-42"), "no cell on the card");
    assert!(
        body.contains(&format!("/run/{DONE_RUN}")),
        "the cell should link to the run profile"
    );
    // `expected` is non-empty and the cell's metric is inside tolerance.
    assert!(body.contains("CHECK PASS"), "no check badge on the card");
}

#[tokio::test]
async fn profile_shows_metrics_drift_log_and_the_no_plots_line() {
    let fx = Fixture::new();
    let (status, body) = get(fx.app(), &format!("/run/{DONE_RUN}")).await;

    assert_eq!(status, StatusCode::OK);

    // Header.
    assert!(body.contains(DONE_RUN));
    assert!(body.contains("train-and-test"));
    assert!(body.contains("juniata-repro"), "no experiment attribution");
    assert!(body.contains("master"), "no git branch");

    // Metrics: every key of manifest.metrics, with its value.
    assert!(body.contains("median_nse_finite"));
    assert!(body.contains("0.7903451323509216"), "metric value missing");
    assert!(body.contains("median_kge_finite"));
    assert!(body.contains("0.8809698224067688"), "metric value missing");
    assert!(body.contains("n_gauges_total"));

    // Sources: drift only on the source whose fp moved, unknown only on
    // the one the lock does not carry.
    let attributes = row_containing(&body, "attributes");
    assert!(
        !attributes.contains("drift"),
        "attributes has not drifted: {attributes}"
    );
    let streamflow = row_containing(&body, "streamflow");
    assert!(
        streamflow.contains("drift"),
        "streamflow drifted: {streamflow}"
    );
    let gages = row_containing(&body, "gages");
    assert!(gages.contains("unknown"), "the lock lacks gages: {gages}");

    // Config snapshot.
    assert!(
        body.contains("workflow: train-and-test"),
        "no config snapshot"
    );

    // Plots: none on disk.
    assert!(
        body.contains(&format!("no plots yet: retrograde plot {DONE_RUN}")),
        "no no-plots line"
    );

    // Log: the tail, and a link to the whole thing.
    assert!(
        body.contains("run complete, wrote eval/predictions.zarr"),
        "no log tail"
    );
    assert!(
        body.contains(&format!("/run/{DONE_RUN}/log")),
        "no full-log link"
    );

    // Notes: the Task 2 placeholder.
    assert!(body.contains("Notes"), "no notes card");
}

#[tokio::test]
async fn unknown_run_is_404() {
    let fx = Fixture::new();
    let (status, _) = get(fx.app(), "/run/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn traversal_out_of_the_runs_directory_is_rejected() {
    let fx = Fixture::new();
    for uri in [
        "/run/../etc",
        "/run/..%2fetc",
        "/run/%2e%2e",
        &format!("/run/{DONE_RUN}/plots/..%2f..%2fmanifest.json"),
    ] {
        let (status, _) = get(fx.app(), uri).await;
        assert_ne!(status, StatusCode::OK, "{uri} must not be served");
    }
}

#[tokio::test]
async fn plots_are_served_and_listed() {
    let fx = Fixture::new();
    let plots = fx.workspace.join("runs").join(DONE_RUN).join("plots");
    fs::create_dir_all(&plots).expect("mkdir plots");
    // A one-pixel PNG is enough: the route only has to hand back the bytes.
    fs::write(plots.join("hydrograph.png"), b"\x89PNG\r\n\x1a\n-fake-").expect("write png");

    let (status, body) = get(fx.app(), &format!("/run/{DONE_RUN}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!("/run/{DONE_RUN}/plots/hydrograph.png")),
        "the plot should be an <img> on the profile"
    );
    assert!(
        !body.contains("no plots yet"),
        "the no-plots line should be gone"
    );

    let (status, ct) =
        content_type(fx.app(), &format!("/run/{DONE_RUN}/plots/hydrograph.png")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("image/png"), "plot served as {ct}");
}

#[tokio::test]
async fn full_log_is_served_as_plain_text() {
    let fx = Fixture::new();
    let (status, body) = get(fx.app(), &format!("/run/{DONE_RUN}/log")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("backend: cpu"),
        "the whole log, from its first line"
    );
    assert!(body.contains(LAST_LOG_LINE));

    let (_, ct) = content_type(fx.app(), &format!("/run/{DONE_RUN}/log")).await;
    assert!(ct.contains("text/plain"), "log served as {ct}");
}

#[tokio::test]
async fn bootstrap_css_is_served_as_text_css() {
    let fx = Fixture::new();
    let (status, ct) = content_type(fx.app(), "/assets/bootstrap.min.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ct.contains("text/css"), "bootstrap served as {ct}");

    let (_, body) = get(fx.app(), "/assets/bootstrap.min.css").await;
    assert!(body.contains("Bootstrap"), "that is not bootstrap");
}

#[tokio::test]
async fn no_page_references_a_cdn() {
    let fx = Fixture::new();
    for uri in ["/", &format!("/run/{DONE_RUN}")] {
        let (_, body) = get(fx.app(), uri).await;
        assert!(!body.contains("//cdn."), "{uri} references a CDN");
        assert!(!body.contains("jsdelivr"), "{uri} references a CDN");
        assert!(
            body.contains("/assets/bootstrap.min.css"),
            "{uri} does not use the vendored css"
        );
    }
}

#[tokio::test]
async fn a_template_directory_is_not_an_experiment() {
    let fx = Fixture::new();
    // A perfectly valid experiment.yaml — it is the leading underscore on
    // the directory that keeps it off the feed.
    write(
        &fx.root.join("experiments/_template/experiment.yaml"),
        "name: template-placeholder\narms: []\nexpected: {}\n",
    );

    let (status, body) = get(fx.app(), "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("template-placeholder"),
        "a _-prefixed directory is a template, not an experiment"
    );
    assert!(
        !body.contains("_template"),
        "the template directory is named"
    );
    assert!(
        body.contains("juniata-repro"),
        "the real experiment must still render"
    );
}

#[tokio::test]
async fn a_broken_experiment_yaml_degrades_to_one_card() {
    let fx = Fixture::new();
    let bad = fx.root.join("experiments/broken/experiment.yaml");
    write(&bad, "name: broken\narms: [\n");

    let (status, body) = get(fx.app(), "/").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "one bad file must not fail the feed"
    );

    // The card names the directory and carries the load error verbatim.
    assert!(body.contains("broken"), "the bad directory is not named");
    let error = retrograde::experiment::Experiment::load(&bad)
        .expect_err("that yaml does not parse")
        .to_string();
    assert!(
        body.contains(&retrograde::view::html::escape(&error)),
        "the card should carry the load error, escaped: {error}"
    );
    assert!(
        body.contains("text-danger"),
        "the error is not marked as one"
    );

    // Everything else still gathered.
    assert!(body.contains("juniata-repro"), "the good card is gone");
    assert!(
        body.contains("CHECK PASS"),
        "the good card lost its verdict"
    );
    assert!(body.contains(DONE_RUN), "the runs table is gone");
}
