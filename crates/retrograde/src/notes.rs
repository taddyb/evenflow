//! Run notes: what a person concluded about a run, kept beside the repo.
//!
//! The store is one SQLite file at `<root>/.retrograde/notes.sqlite`,
//! created on first use and gitignored. It is deliberately *not* in git:
//! notes accumulate while you browse, and a half-thought typed into a
//! textarea is not a commit. [`export`] is the one way a note reaches git
//! — it renders a run's notes as `notes.md` inside the experiment cell that
//! claims the run, where `sweep` already writes and a reviewer already
//! looks.
//!
//! ```text
//! <root>/.retrograde/notes.sqlite     every note, by run id
//! experiments/<dir>/results/<arm>/seed-<s>/notes.md   what `export` commits
//! ```
//!
//! Both the `view` handlers and the `notes` subcommand go through [`Notes`];
//! there is one schema and one insert path.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::Error;

/// One note, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// RFC3339, seconds resolution — the same stamp `status.json` uses.
    pub created_at: String,
    pub body: String,
}

/// `<root>/.retrograde/notes.sqlite`.
pub fn db_path(root: &Path) -> PathBuf {
    root.join(".retrograde").join("notes.sqlite")
}

/// The one table, plus the index the only query needs. `IF NOT EXISTS`
/// throughout, so opening an existing database is a no-op.
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS notes (
    id         INTEGER PRIMARY KEY,
    run_id     TEXT NOT NULL,
    created_at TEXT NOT NULL,
    body       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_run_id ON notes(run_id);
";

/// An open notes database.
pub struct Notes {
    conn: Connection,
    path: PathBuf,
}

impl Notes {
    /// Open `<root>/.retrograde/notes.sqlite`, creating the directory, the
    /// file, and the schema if they are not there yet. Opening is the only
    /// thing that creates them: there is no separate init step.
    pub fn open(root: &Path) -> Result<Notes, Error> {
        let path = db_path(root);
        let dir = path.parent().expect("db_path always has a parent");
        fs::create_dir_all(dir).map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let conn = Connection::open(&path).map_err(|source| Error::Sqlite {
            path: path.clone(),
            source,
        })?;
        let notes = Notes { conn, path };
        notes.sql(notes.conn.execute_batch(SCHEMA))?;
        Ok(notes)
    }

    /// Store a note against `run_id`. The body is trimmed and its line
    /// endings normalised — a `<textarea>` posts CRLF — and a body that is
    /// empty once trimmed is refused here rather than stored as a blank
    /// row, so the web handler and the CLI cannot disagree about it.
    pub fn add(&self, run_id: &str, body: &str) -> Result<(), Error> {
        let body = normalize(body);
        if body.is_empty() {
            return Err(Error::Invalid("a note needs a body".to_string()));
        }
        self.sql(self.conn.execute(
            "INSERT INTO notes (run_id, created_at, body) VALUES (?1, ?2, ?3)",
            (run_id, crate::now(), &body),
        ))?;
        Ok(())
    }

    /// Every note for `run_id`, newest first. `created_at` is only second
    /// resolution, so the row id breaks ties: two notes typed in the same
    /// second still come back in the order they were written.
    pub fn list(&self, run_id: &str) -> Result<Vec<Note>, Error> {
        let mut stmt = self.sql(self.conn.prepare(
            "SELECT created_at, body FROM notes WHERE run_id = ?1 \
             ORDER BY created_at DESC, id DESC",
        ))?;
        let rows = self.sql(stmt.query_map([run_id], |row| {
            Ok(Note {
                created_at: row.get(0)?,
                body: row.get(1)?,
            })
        }))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(self.sql(row)?);
        }
        Ok(out)
    }

    /// Attach the database path to a rusqlite error, the way every other
    /// [`Error`] variant in this crate carries its source path.
    fn sql<T>(&self, result: rusqlite::Result<T>) -> Result<T, Error> {
        result.map_err(|source| Error::Sqlite {
            path: self.path.clone(),
            source,
        })
    }
}

/// CRLF from a `<textarea>` becomes LF, and surrounding whitespace goes.
fn normalize(body: &str) -> String {
    body.replace("\r\n", "\n").trim().to_string()
}

/// A run's notes as the markdown [`export`] writes: a title naming the run,
/// then one `## <timestamp>` section per note, newest first — the same
/// order the profile page and `notes list` show.
pub fn markdown(run_id: &str, notes: &[Note]) -> String {
    let mut out = format!("# Notes: {run_id}\n");
    for note in notes {
        out.push_str(&format!("\n## {}\n\n{}\n", note.created_at, note.body));
    }
    out
}

/// What the `notes` subcommands were pointed at.
#[derive(Debug, Clone)]
pub struct NotesOptions {
    pub run_id: String,
    /// Workspace root (default: nearest `Cargo.toml` with `[workspace]`).
    pub root: Option<PathBuf>,
}

impl NotesOptions {
    fn root(&self) -> Result<PathBuf, Error> {
        match &self.root {
            Some(r) => crate::canonical(r),
            None => {
                let cwd = std::env::current_dir().map_err(|source| Error::Io {
                    path: PathBuf::from("."),
                    source,
                })?;
                crate::find_workspace_root(&cwd)
            }
        }
    }
}

/// `retrograde notes list <run-id>`: every note, newest first, as the text
/// to print.
pub fn list(opts: &NotesOptions) -> Result<String, Error> {
    let root = opts.root()?;
    let notes = Notes::open(&root)?.list(&opts.run_id)?;
    if notes.is_empty() {
        return Ok(format!("no notes for run {}\n", opts.run_id));
    }
    let mut out = String::new();
    for note in &notes {
        out.push_str(&format!("{}\n{}\n\n", note.created_at, note.body));
    }
    Ok(out)
}

/// What `export` did, so the caller can say so. Export never overwrites in
/// silence: an existing `notes.md` is rewritten only when the note set
/// differs from what it already holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Export {
    /// There was no `notes.md` in the cell; now there is.
    Wrote(PathBuf),
    /// The cell's `notes.md` said something else and was replaced.
    Rewrote(PathBuf),
    /// The cell's `notes.md` already said exactly this. Nothing was written.
    Unchanged(PathBuf),
    /// No experiment cell claims this run, so there is nowhere in git to
    /// put the file. The markdown is returned for the caller to print.
    Unclaimed(String),
}

/// `retrograde notes export <run-id>`: render the run's notes as markdown
/// and put them in the cell that claims the run, if one does.
pub fn export(opts: &NotesOptions) -> Result<Export, Error> {
    let root = opts.root()?;
    let notes = Notes::open(&root)?.list(&opts.run_id)?;
    if notes.is_empty() {
        return Err(Error::Invalid(format!(
            "no notes for run {} — nothing to export",
            opts.run_id
        )));
    }
    let markdown = markdown(&opts.run_id, &notes);

    // `results/<arm>/seed-<s>/` is where `sweep` already writes this run's
    // config, status, and manifest; the note belongs beside them.
    let Some(cell) = crate::view::data::claiming_cell(&root, &opts.run_id) else {
        return Ok(Export::Unclaimed(markdown));
    };
    let dest = cell.join("notes.md");

    match fs::read_to_string(&dest) {
        Ok(existing) if existing == markdown => Ok(Export::Unchanged(dest)),
        Ok(_) => {
            crate::write_file(&dest, &markdown)?;
            Ok(Export::Rewrote(dest))
        }
        Err(_) => {
            crate::write_file(&dest, &markdown)?;
            Ok(Export::Wrote(dest))
        }
    }
}
