//! `retrograde notes`: the SQLite store, the profile card's form, and the
//! two subcommands.
//!
//! The fixture is the `tests/view.rs` shape trimmed to what notes need: an
//! experiments tree whose one cell claims `CLAIMED_RUN`, and a workspace
//! holding that run plus `UNCLAIMED_RUN`, which no cell references. The
//! experiment *directory* is deliberately not its `name:`, so an export
//! that used the name instead of the directory would miss.

use std::fs;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use tower::ServiceExt;

use retrograde::notes::{self, Export, Notes, NotesOptions};

const CLAIMED_RUN: &str = "2026-09-15T00-00-01Z-train-and-test";
const UNCLAIMED_RUN: &str = "2026-09-15T06-00-00Z-train";

/// The cell that claims `CLAIMED_RUN`, relative to the root.
const CELL: &str = "experiments/juniata-repro/results/kan/seed-42";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let workspace = root.join("crates/ddrs/.ddrs");

        // The experiment's directory is `juniata-repro`; its name is not.
        write(
            &root.join("experiments/juniata-repro/experiment.yaml"),
            "name: juniata reproduction\n\
             question: Does the bundle reproduce?\n\
             arms:\n\
             \x20 - name: kan\n\
             \x20   config: arms/kan/ddrs.yaml\n\
             \x20   seeds: [42]\n",
        );
        write(
            &root.join(CELL).join("status.json"),
            &format!(r#"{{"status": "done", "run_id": "{CLAIMED_RUN}"}}"#),
        );

        for id in [CLAIMED_RUN, UNCLAIMED_RUN] {
            write(
                &workspace.join("runs").join(id).join("manifest.json"),
                &manifest(id),
            );
        }

        Fixture {
            _tmp: tmp,
            root,
            workspace,
        }
    }

    fn app(&self) -> Router {
        retrograde::view::app(&self.root, &self.workspace)
    }

    fn opts(&self, run_id: &str) -> NotesOptions {
        NotesOptions {
            run_id: run_id.to_string(),
            root: Some(self.root.clone()),
        }
    }
}

fn manifest(run_id: &str) -> String {
    format!(
        r#"{{
  "run_id": "{run_id}",
  "git": {{ "sha": "05ab47fa03f6584afad6f6310fec1c5680e2d04c", "dirty": false, "branch": "master" }},
  "workflow": "train-and-test",
  "started_at": "2026-09-15T00:00:00Z",
  "finished_at": "2026-09-15T00:10:00Z",
  "status": "ok",
  "sources": {{}},
  "outputs": {{}},
  "metrics": {{ "median_nse_finite": 0.79 }}
}}"#
    )
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, contents).expect("write");
}

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

/// POST a form-encoded body, returning the status and the `location`
/// header (empty when there is none).
async fn post_form(app: Router, uri: &str, form: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    (status, location)
}

// ---------------------------------------------------------------- the store

#[test]
fn insert_and_list_round_trips_through_the_database() {
    let fx = Fixture::new();
    let db = Notes::open(&fx.root).expect("open");

    db.add(CLAIMED_RUN, "first: the loss curve is flat after epoch 20")
        .expect("add");
    db.add(CLAIMED_RUN, "second: rerun with a lower lr")
        .expect("add");
    db.add(UNCLAIMED_RUN, "a note about a different run")
        .expect("add");

    assert!(
        notes::db_path(&fx.root).is_file(),
        "open should create {}",
        notes::db_path(&fx.root).display()
    );

    let listed = db.list(CLAIMED_RUN).expect("list");
    assert_eq!(listed.len(), 2, "a note leaked across run ids: {listed:?}");
    // Newest first.
    assert_eq!(listed[0].body, "second: rerun with a lower lr");
    assert_eq!(
        listed[1].body,
        "first: the loss curve is flat after epoch 20"
    );
    assert!(
        !listed[0].created_at.is_empty(),
        "a note must carry a timestamp"
    );

    // Reopening reads the same rows back: the rows are on disk, not in RAM.
    let reopened = Notes::open(&fx.root).expect("reopen");
    assert_eq!(reopened.list(CLAIMED_RUN).expect("list").len(), 2);
}

#[test]
fn a_whitespace_only_note_is_refused_by_the_store() {
    let fx = Fixture::new();
    let db = Notes::open(&fx.root).expect("open");
    assert!(db.add(CLAIMED_RUN, "   \n\t ").is_err());
    assert!(db.add(CLAIMED_RUN, "").is_err());
    assert!(db.list(CLAIMED_RUN).expect("list").is_empty());
}

// ----------------------------------------------------------------- the page

#[tokio::test]
async fn posting_a_note_redirects_and_the_profile_shows_it() {
    let fx = Fixture::new();

    let (status, body) = get(fx.app(), &format!("/run/{CLAIMED_RUN}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(&format!(r#"action="/run/{CLAIMED_RUN}/notes""#)),
        "the Notes card should carry a form posting to the run's notes route"
    );
    assert!(
        body.contains(r#"name="body""#),
        "the form needs a `body` textarea"
    );

    let (status, location) = post_form(
        fx.app(),
        &format!("/run/{CLAIMED_RUN}/notes"),
        "body=peaks+are+over-attenuated+%3C4+m3%2Fs",
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "POST should redirect 303");
    assert_eq!(location, format!("/run/{CLAIMED_RUN}"));

    let (status, body) = get(fx.app(), &format!("/run/{CLAIMED_RUN}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("peaks are over-attenuated &lt;4 m3/s"),
        "the note should be on the page, escaped: {body}"
    );
    assert!(
        !body.contains("<4 m3/s"),
        "a note body must never reach the page unescaped"
    );

    // It belongs to that run only.
    let (_, other) = get(fx.app(), &format!("/run/{UNCLAIMED_RUN}")).await;
    assert!(
        !other.contains("over-attenuated"),
        "a note leaked onto another run's profile"
    );
}

#[tokio::test]
async fn an_empty_note_is_rejected_with_400() {
    let fx = Fixture::new();

    for form in ["body=", "body=+%20%0A", ""] {
        let (status, _) = post_form(fx.app(), &format!("/run/{CLAIMED_RUN}/notes"), form).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "an empty note ({form:?}) must be a 400"
        );
    }

    assert!(
        Notes::open(&fx.root)
            .expect("open")
            .list(CLAIMED_RUN)
            .expect("list")
            .is_empty(),
        "a rejected note must not be stored"
    );
}

#[tokio::test]
async fn a_note_on_an_unknown_run_is_404() {
    let fx = Fixture::new();
    let (status, _) = post_form(fx.app(), "/run/does-not-exist/notes", "body=hello").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = post_form(fx.app(), "/run/..%2fetc/notes", "body=hello").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unopenable_database_degrades_to_a_message_in_the_card() {
    let fx = Fixture::new();
    // `.retrograde` is a regular file, so the directory cannot be created.
    fs::write(fx.root.join(".retrograde"), "not a directory").expect("write");

    let (status, body) = get(fx.app(), &format!("/run/{CLAIMED_RUN}")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "browsing must not fail because notes could not be opened"
    );
    assert!(
        body.contains("text-danger"),
        "the Notes card should report the error: {body}"
    );
}

// ------------------------------------------------------------------ the CLI

#[test]
fn notes_list_prints_every_note_newest_first() {
    let fx = Fixture::new();
    let db = Notes::open(&fx.root).expect("open");
    db.add(CLAIMED_RUN, "older").expect("add");
    db.add(CLAIMED_RUN, "newer").expect("add");
    drop(db);

    let report = notes::list(&fx.opts(CLAIMED_RUN)).expect("list");
    let newer = report.find("newer").expect("no newer note in the report");
    let older = report.find("older").expect("no older note in the report");
    assert!(newer < older, "notes should print newest first: {report}");

    let empty = notes::list(&fx.opts(UNCLAIMED_RUN)).expect("list");
    assert!(
        empty.contains("no notes"),
        "a run with no notes should say so: {empty}"
    );
}

#[test]
fn export_writes_notes_md_into_the_claiming_cell() {
    let fx = Fixture::new();
    let db = Notes::open(&fx.root).expect("open");
    db.add(CLAIMED_RUN, "the peaks are attenuated")
        .expect("add");
    drop(db);

    let dest = fx.root.join(CELL).join("notes.md");
    match notes::export(&fx.opts(CLAIMED_RUN)).expect("export") {
        Export::Wrote(path) => assert_eq!(path, dest),
        other => panic!("expected a fresh write, got {other:?}"),
    }

    let text = fs::read_to_string(&dest).expect("notes.md");
    assert!(text.contains("the peaks are attenuated"), "{text}");
    assert!(
        text.contains("## "),
        "one `## <timestamp>` per note: {text}"
    );
    assert!(text.contains(CLAIMED_RUN), "the file should name the run");

    // Re-exporting the same notes says so and does not rewrite.
    match notes::export(&fx.opts(CLAIMED_RUN)).expect("export") {
        Export::Unchanged(path) => assert_eq!(path, dest),
        other => panic!("expected unchanged, got {other:?}"),
    }

    // A new note makes the set differ, so it is rewritten.
    Notes::open(&fx.root)
        .expect("open")
        .add(CLAIMED_RUN, "and the recession is too fast")
        .expect("add");
    match notes::export(&fx.opts(CLAIMED_RUN)).expect("export") {
        Export::Rewrote(path) => assert_eq!(path, dest),
        other => panic!("expected a rewrite, got {other:?}"),
    }
    let text = fs::read_to_string(&dest).expect("notes.md");
    assert!(text.contains("and the recession is too fast"), "{text}");
    assert!(text.contains("the peaks are attenuated"), "{text}");
}

#[test]
fn export_of_an_unclaimed_run_prints_instead_of_writing() {
    let fx = Fixture::new();
    Notes::open(&fx.root)
        .expect("open")
        .add(UNCLAIMED_RUN, "nothing claims this run")
        .expect("add");

    match notes::export(&fx.opts(UNCLAIMED_RUN)).expect("export") {
        Export::Unclaimed(markdown) => {
            assert!(markdown.contains("nothing claims this run"), "{markdown}");
            assert!(markdown.contains(UNCLAIMED_RUN), "{markdown}");
        }
        other => panic!("no cell claims this run, so nothing should be written: {other:?}"),
    }

    // And no stray notes.md appeared anywhere under experiments/.
    assert!(
        !fx.root.join(CELL).join("notes.md").exists(),
        "an unclaimed run must not write into someone else's cell"
    );
}

#[test]
fn export_with_no_notes_is_an_error_not_an_empty_file() {
    let fx = Fixture::new();
    assert!(notes::export(&fx.opts(CLAIMED_RUN)).is_err());
    assert!(!fx.root.join(CELL).join("notes.md").exists());
}
