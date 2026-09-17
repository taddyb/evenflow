//! `retrograde view`: a local, read-only web view of the experiments tree
//! and the ddrs workspace.
//!
//! The server binds `127.0.0.1` and reads: every page is rendered from
//! files already on disk. The one thing it writes is the notes database
//! (`<root>/.retrograde/notes.sqlite`, see [`crate::notes`]) — nothing
//! under `experiments/` or the ddrs workspace is ever touched. [`data`]
//! gathers, [`html`] renders, and this module is the router that maps a
//! URL onto the two of them.
//!
//! ```text
//! GET /                              feed: experiment cards, then runs
//! GET /run/<id>                      profile: header, metrics, sources,
//!                                    config, plots, log, notes
//! POST /run/<id>/notes               add a note (form-encoded `body`)
//! GET /run/<id>/log                  the whole run.log, text/plain
//! GET /run/<id>/plots/<file>.png     one plot image
//! GET /assets/bootstrap.min.css      the vendored stylesheet
//! ```
//!
//! Run ids and plot file names arriving from a URL are checked to be a
//! single path component before they are joined onto a directory; anything
//! else is a 404.

pub mod data;
pub mod html;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Form, Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::notes::Notes;
use crate::Error;

/// Bootstrap 5.3.3, vendored under `assets/` (MIT; the license text is
/// beside it). Compiled in so the binary has no runtime file dependency and
/// no page ever references a CDN.
const BOOTSTRAP_CSS: &str = include_str!("../../assets/bootstrap.min.css");

#[derive(Debug, Clone)]
pub struct ViewOptions {
    /// Workspace root holding `experiments/` (default: nearest `Cargo.toml`
    /// with a `[workspace]` table, from the current directory).
    pub root: Option<PathBuf>,
    /// ddrs workspace holding `runs/` and `sources.lock`
    /// (default: `<root>/crates/ddrs/.ddrs`).
    pub workspace: Option<PathBuf>,
    pub port: u16,
}

/// The two directories every handler reads from.
#[derive(Debug)]
struct Paths {
    root: PathBuf,
    workspace: PathBuf,
}

/// The router, ready to serve. Exposed so tests can drive it with
/// `tower::ServiceExt::oneshot` instead of a socket.
pub fn app(root: &Path, workspace: &Path) -> Router {
    let paths = Arc::new(Paths {
        root: root.to_path_buf(),
        workspace: workspace.to_path_buf(),
    });
    Router::new()
        .route("/", get(feed))
        .route("/run/{run_id}", get(profile))
        .route("/run/{run_id}/notes", post(add_note))
        .route("/run/{run_id}/log", get(log))
        .route("/run/{run_id}/plots/{file}", get(plot))
        .route("/assets/bootstrap.min.css", get(bootstrap_css))
        .with_state(paths)
}

/// Resolve the options, bind `127.0.0.1:<port>`, and serve until killed.
pub fn serve(opts: &ViewOptions) -> Result<(), Error> {
    let (root, workspace) = resolve(opts)?;
    let app = app(&root, &workspace);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, opts.port));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| Error::Io {
            path: root.clone(),
            source,
        })?;

    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|source| Error::Io {
                path: PathBuf::from(addr.to_string()),
                source,
            })?;
        println!("retrograde view: http://{addr}");
        println!("  experiments: {}", root.display());
        println!("  workspace:   {}", workspace.display());
        axum::serve(listener, app)
            .await
            .map_err(|source| Error::Io {
                path: PathBuf::from(addr.to_string()),
                source,
            })
    })
}

/// `--root` / `--workspace`, or the same defaults `sweep` uses.
fn resolve(opts: &ViewOptions) -> Result<(PathBuf, PathBuf), Error> {
    let root = match &opts.root {
        Some(r) => crate::canonical(r)?,
        None => {
            let cwd = std::env::current_dir().map_err(|source| Error::Io {
                path: PathBuf::from("."),
                source,
            })?;
            crate::find_workspace_root(&cwd)?
        }
    };
    let workspace = match &opts.workspace {
        Some(w) => w.clone(),
        None => root.join("crates/ddrs/.ddrs"),
    };
    Ok((root, workspace))
}

/// A URL segment that is about to be joined onto a directory. Anything
/// that is not one plain path component — `/`, `\`, `.`, `..`, empty, or a
/// NUL — is refused, so no request can walk out of the run directory.
fn safe_component(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains('/')
        && !segment.contains('\\')
        && !segment.contains('\0')
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found\n").into_response()
}

fn failed(error: Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("retrograde view: {error}\n"),
    )
        .into_response()
}

async fn feed(State(paths): State<Arc<Paths>>) -> Response {
    match data::feed(&paths.root, &paths.workspace) {
        Ok(feed) => Html(html::feed(&feed)).into_response(),
        Err(e) => failed(e),
    }
}

async fn profile(State(paths): State<Arc<Paths>>, UrlPath(run_id): UrlPath<String>) -> Response {
    if !safe_component(&run_id) {
        return not_found();
    }
    match data::profile(&paths.root, &paths.workspace, &run_id) {
        Ok(Some(profile)) => Html(html::profile(&profile)).into_response(),
        Ok(None) => not_found(),
        Err(e) => failed(e),
    }
}

/// The `POST /run/<id>/notes` body: one `<textarea name="body">`.
#[derive(Debug, Deserialize)]
struct NoteForm {
    #[serde(default)]
    body: String,
}

/// Add a note to a run, then send the browser back to the profile so a
/// reload cannot repost it (303, not 307).
async fn add_note(
    State(paths): State<Arc<Paths>>,
    UrlPath(run_id): UrlPath<String>,
    Form(form): Form<NoteForm>,
) -> Response {
    // A note names a run: an id that is not a run in this workspace is a
    // 404, the same as asking for its profile.
    if !safe_component(&run_id)
        || !data::run_dir(&paths.workspace, &run_id)
            .join("manifest.json")
            .is_file()
    {
        return not_found();
    }
    if form.body.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "a note needs a body\n").into_response();
    }
    match Notes::open(&paths.root).and_then(|notes| notes.add(&run_id, &form.body)) {
        Ok(()) => Redirect::to(&format!("/run/{run_id}")).into_response(),
        Err(e) => failed(e),
    }
}

async fn log(State(paths): State<Arc<Paths>>, UrlPath(run_id): UrlPath<String>) -> Response {
    if !safe_component(&run_id) {
        return not_found();
    }
    let path = data::run_dir(&paths.workspace, &run_id).join("run.log");
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], bytes).into_response(),
        Err(_) => not_found(),
    }
}

async fn plot(
    State(paths): State<Arc<Paths>>,
    UrlPath((run_id, file)): UrlPath<(String, String)>,
) -> Response {
    if !safe_component(&run_id) || !safe_component(&file) || !file.ends_with(".png") {
        return not_found();
    }
    let path = data::run_dir(&paths.workspace, &run_id)
        .join("plots")
        .join(&file);
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(_) => not_found(),
    }
}

async fn bootstrap_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        BOOTSTRAP_CSS,
    )
        .into_response()
}
