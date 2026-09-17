//! The two pages, rendered from [`crate::view::data`]'s structs.
//!
//! String concatenation, no template engine: the pages are small and the
//! escaping rule is one function. Every value that came off disk goes
//! through [`escape`] before it reaches the page.

use crate::cell::CellStatus;
use crate::view::data::{CheckBadge, Drift, ExperimentCard, Feed, Origin, Profile, RunRow};

/// Minimal HTML escaping for text and attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The document shell. The stylesheet is the vendored Bootstrap this
/// binary serves itself; nothing on the page comes off the network.
fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="/assets/bootstrap.min.css">
</head>
<body class="bg-body-tertiary">
<nav class="navbar navbar-expand bg-dark" data-bs-theme="dark">
  <div class="container">
    <a class="navbar-brand" href="/">retrograde</a>
  </div>
</nav>
<main class="container py-4">
{body}
</main>
</body>
</html>
"#,
        title = escape(title),
    )
}

fn badge(class: &str, text: &str) -> String {
    format!(
        r#"<span class="badge {}">{}</span>"#,
        escape(class),
        escape(text)
    )
}

fn status_badge(status: &str) -> String {
    let class = match status {
        "ok" | "done" => "text-bg-success",
        "failed" | "error" => "text-bg-danger",
        "running" => "text-bg-info",
        _ => "text-bg-secondary",
    };
    badge(class, status)
}

fn cell_status_badge(status: CellStatus) -> String {
    let class = match status {
        CellStatus::Done => "text-bg-success",
        CellStatus::Failed => "text-bg-danger",
        CellStatus::Running => "text-bg-info",
        CellStatus::Pending => "text-bg-secondary",
    };
    badge(class, status.as_str())
}

fn origin_text(origin: &Origin) -> String {
    format!(
        "{} / {} / seed-{}",
        origin.experiment, origin.arm, origin.seed
    )
}

fn dash(value: &Option<String>) -> String {
    escape(value.as_deref().unwrap_or("-"))
}

/// `GET /` — experiments, then runs.
pub fn feed(feed: &Feed) -> String {
    let mut body = String::new();

    body.push_str(r#"<h1 class="h3 mb-3">Experiments</h1>"#);
    if feed.experiments.is_empty() {
        body.push_str(r#"<p class="text-muted">no experiments/*/experiment.yaml found</p>"#);
    }
    for card in &feed.experiments {
        body.push_str(&experiment_card(card));
    }

    body.push_str(r#"<h1 class="h3 mt-5 mb-3" id="runs">Runs</h1>"#);
    body.push_str(&runs_table(&feed.runs));

    page("retrograde", &body)
}

fn experiment_card(card: &ExperimentCard) -> String {
    // An experiment that would not load says so on its own card, named by
    // its directory, and costs the feed nothing else.
    if let Some(error) = &card.error {
        return format!(
            r#"<div class="card mb-3">
  <div class="card-body">
    <h2 class="card-title h5">{name}</h2>
    <p class="card-text text-danger mb-0">{error}</p>
  </div>
</div>
"#,
            name = escape(&card.name),
            error = escape(error),
        );
    }

    let check = match card.check {
        CheckBadge::Pass => badge("text-bg-success", "CHECK PASS"),
        CheckBadge::Fail => badge("text-bg-danger", "CHECK FAIL"),
        CheckBadge::None => badge("text-bg-secondary", "no expected:"),
    };

    let mut rows = String::new();
    for cell in &card.cells {
        let label = format!("{} / seed-{}", cell.arm, cell.seed);
        let link = match &cell.run_id {
            Some(id) => format!(r#"<a href="/run/{id}">{id}</a>"#, id = escape(id)),
            None => "-".to_string(),
        };
        let metrics = if cell.status == CellStatus::Done {
            format!(
                "<td>{}</td><td>{}</td>",
                dash(&cell.median_nse),
                dash(&cell.median_kge)
            )
        } else {
            "<td>-</td><td>-</td>".to_string()
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td>{}<td>{}</td></tr>",
            escape(&label),
            cell_status_badge(cell.status),
            metrics,
            link,
        ));
    }

    format!(
        r#"<div class="card mb-3">
  <div class="card-body">
    <div class="d-flex justify-content-between align-items-start">
      <h2 class="card-title h5">{name}</h2>
      {check}
    </div>
    <p class="card-text text-muted">{question}</p>
    <p class="card-text"><small class="text-muted">{arms} arm(s), {cells} cell(s)</small></p>
    <table class="table table-sm align-middle mb-0">
      <thead><tr><th>cell</th><th>status</th><th>median NSE</th><th>median KGE</th><th>run</th></tr></thead>
      <tbody>{rows}</tbody>
    </table>
  </div>
</div>
"#,
        name = escape(&card.name),
        question = escape(&card.question),
        arms = card.arm_count,
        cells = card.cells.len(),
    )
}

fn runs_table(runs: &[RunRow]) -> String {
    if runs.is_empty() {
        return r#"<p class="text-muted">no runs in this workspace</p>"#.to_string();
    }
    let mut rows = String::new();
    for run in runs {
        let progress = if run.in_progress {
            badge("text-bg-warning", "in progress")
        } else {
            String::new()
        };
        let origin = match &run.origin {
            Some(o) => escape(&origin_text(o)),
            None => "-".to_string(),
        };
        rows.push_str(&format!(
            r#"<tr>
  <td><a href="/run/{id}">{id}</a> {progress}</td>
  <td>{status}</td>
  <td>{workflow}</td>
  <td>{origin}</td>
  <td>{started}</td>
  <td>{duration}</td>
  <td>{nse}</td>
  <td>{kge}</td>
  <td><code>{sha}</code></td>
</tr>
"#,
            id = escape(&run.run_id),
            status = status_badge(&run.status),
            workflow = escape(&run.workflow),
            started = escape(&run.started),
            duration = escape(&run.duration),
            nse = dash(&run.median_nse),
            kge = dash(&run.median_kge),
            sha = escape(&run.ddrs_sha),
        ));
    }
    format!(
        r#"<div class="card"><div class="card-body p-0">
<table class="table table-sm table-hover align-middle mb-0">
<thead><tr><th>run</th><th>status</th><th>workflow</th><th>experiment</th><th>started</th><th>duration</th><th>median NSE</th><th>median KGE</th><th>ddrs</th></tr></thead>
<tbody>{rows}</tbody>
</table>
</div></div>
"#
    )
}

/// `GET /run/<id>` — header, Metrics, Sources, Config, Plots, Log, Notes.
pub fn profile(profile: &Profile) -> String {
    let mut body = String::new();
    body.push_str(&header_card(profile));
    body.push_str(&metrics_card(profile));
    body.push_str(&sources_card(profile));
    body.push_str(&config_card(profile));
    body.push_str(&plots_card(profile));
    body.push_str(&log_card(profile));
    body.push_str(&notes_card(profile));
    page(&profile.run_id, &body)
}

fn card(title: &str, inner: &str) -> String {
    format!(
        r#"<div class="card mb-3">
  <div class="card-body">
    <h2 class="card-title h5">{title}</h2>
    {inner}
  </div>
</div>
"#,
        title = escape(title),
    )
}

fn header_card(p: &Profile) -> String {
    let origin = match &p.origin {
        Some(o) => format!(
            r#"<p class="card-text text-muted">{}</p>"#,
            escape(&origin_text(o))
        ),
        None => String::new(),
    };
    let dirty = if p.dirty {
        badge("text-bg-warning", "dirty")
    } else {
        String::new()
    };
    format!(
        r#"<div class="card mb-3">
  <div class="card-body">
    <div class="d-flex justify-content-between align-items-start">
      <h1 class="card-title h4"><code>{id}</code></h1>
      {status}
    </div>
    {origin}
    <dl class="row mb-0">
      <dt class="col-sm-3">workflow</dt><dd class="col-sm-9">{workflow}</dd>
      <dt class="col-sm-3">ddrs</dt><dd class="col-sm-9"><code>{sha}</code> on <code>{branch}</code> {dirty}</dd>
      <dt class="col-sm-3">started</dt><dd class="col-sm-9">{started}</dd>
      <dt class="col-sm-3">finished</dt><dd class="col-sm-9">{finished}</dd>
      <dt class="col-sm-3">duration</dt><dd class="col-sm-9">{duration}</dd>
    </dl>
  </div>
</div>
"#,
        id = escape(&p.run_id),
        status = status_badge(&p.status),
        workflow = escape(&p.workflow),
        sha = escape(&p.ddrs_sha),
        branch = escape(&p.branch),
        started = escape(&p.started),
        finished = escape(&p.finished),
        duration = escape(&p.duration),
    )
}

fn metrics_card(p: &Profile) -> String {
    if p.metrics.is_empty() {
        return card("Metrics", r#"<p class="text-muted mb-0">no metrics</p>"#);
    }
    let mut rows = String::new();
    for (key, value) in &p.metrics {
        rows.push_str(&format!(
            "<tr><td>{}</td><td><code>{}</code></td></tr>",
            escape(key),
            escape(value)
        ));
    }
    card(
        "Metrics",
        &format!(r#"<table class="table table-sm mb-0"><tbody>{rows}</tbody></table>"#),
    )
}

fn sources_card(p: &Profile) -> String {
    if p.sources.is_empty() {
        return card("Sources", r#"<p class="text-muted mb-0">no sources</p>"#);
    }
    let mut rows = String::new();
    for source in &p.sources {
        let drift = match source.drift {
            Drift::Same => String::new(),
            Drift::Drift => badge("text-bg-danger", "drift"),
            Drift::Unknown => badge("text-bg-secondary", "unknown"),
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td><code>{}</code></td><td><code>{}</code></td><td>{}</td></tr>",
            escape(&source.name),
            escape(&source.path),
            escape(&source.fingerprint),
            drift,
        ));
    }
    card(
        "Sources",
        &format!(
            r#"<table class="table table-sm align-middle mb-0">
<thead><tr><th>name</th><th>path</th><th>fingerprint</th><th>lock</th></tr></thead>
<tbody>{rows}</tbody></table>"#
        ),
    )
}

fn config_card(p: &Profile) -> String {
    match &p.config {
        Some(text) => card(
            "Config",
            &format!(r#"<pre class="mb-0"><code>{}</code></pre>"#, escape(text)),
        ),
        None => card("Config", r#"<p class="text-muted mb-0">no config.yaml</p>"#),
    }
}

fn plots_card(p: &Profile) -> String {
    if p.plots.is_empty() {
        return card(
            "Plots",
            &format!(
                r#"<p class="text-muted mb-0">no plots yet: retrograde plot {}</p>"#,
                escape(&p.run_id)
            ),
        );
    }
    let mut imgs = String::new();
    for name in &p.plots {
        imgs.push_str(&format!(
            r#"<figure class="mb-3"><img class="img-fluid" src="/run/{id}/plots/{name}" alt="{name}"><figcaption class="text-muted">{name}</figcaption></figure>"#,
            id = escape(&p.run_id),
            name = escape(name),
        ));
    }
    card("Plots", &imgs)
}

fn log_card(p: &Profile) -> String {
    let link = format!(
        r#"<p><a href="/run/{id}/log">whole log</a></p>"#,
        id = escape(&p.run_id)
    );
    match &p.log_tail {
        Some(tail) => card(
            "Log",
            &format!(
                r#"{link}<pre class="mb-0"><code>{}</code></pre>"#,
                escape(tail)
            ),
        ),
        None => card("Log", r#"<p class="text-muted mb-0">no run.log</p>"#),
    }
}

/// Existing notes newest first, then the form that adds one. Bodies are
/// shown in a `<pre>` so a pasted traceback or a table keeps its shape —
/// escaped like every other value, never rendered as markup.
fn notes_card(p: &Profile) -> String {
    let mut inner = String::new();

    if let Some(error) = &p.notes_error {
        inner.push_str(&format!(
            r#"<p class="text-danger">notes unavailable: {}</p>"#,
            escape(error)
        ));
    } else if p.notes.is_empty() {
        inner.push_str(r#"<p class="text-muted">no notes yet</p>"#);
    }

    for note in &p.notes {
        inner.push_str(&format!(
            r#"<div class="mb-3 border-start border-3 ps-3">
  <div class="small text-muted">{at}</div>
  <pre class="mb-0"><code>{body}</code></pre>
</div>
"#,
            at = escape(&note.created_at),
            body = escape(&note.body),
        ));
    }

    inner.push_str(&format!(
        r#"<form method="post" action="/run/{id}/notes">
  <div class="mb-2">
    <textarea class="form-control" name="body" rows="3" placeholder="What did this run show?"></textarea>
  </div>
  <button class="btn btn-primary btn-sm" type="submit">Add note</button>
  <span class="ms-2 small text-muted">retrograde notes export {id} puts these in the cell</span>
</form>
"#,
        id = escape(&p.run_id),
    ));

    card("Notes", &inner)
}
