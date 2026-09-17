//! `retrograde plot` driven against a fake `uv` on PATH.
//!
//! No test here runs the real uv project — matplotlib is not a test
//! dependency of the Rust crate. The fake is a POSIX shell script in a
//! tempdir that records its own argv and working directory and then exits
//! with whatever code the test asked for, so what is under test is the
//! command line `plot` builds and the exit code it hands back.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use retrograde::{plot, PlotOptions};

/// `PATH` is process-global, so the tests that rewrite it take turns.
fn path_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const FAKE_UV: &str = r#"#!/bin/sh
FAKE_DIR='__FAKE_DIR__'
: > "$FAKE_DIR/argv.txt"
for a in "$@"; do printf '%s\n' "$a" >> "$FAKE_DIR/argv.txt"; done
printf '%s\n' "$PWD" > "$FAKE_DIR/cwd.txt"
if [ -f "$FAKE_DIR/exit_code" ]; then
  exit "$(cat "$FAKE_DIR/exit_code")"
fi
exit 0
"#;

const RUN_ID: &str = "2026-09-16T01-21-34Z-train-and-test";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    fake_dir: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    /// A workspace root with the uv project in it and one run in the
    /// default ddrs workspace, plus a fake `uv` in its own directory.
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("evenflow");
        fs::create_dir_all(root.join("crates/retrograde/py")).unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        fs::write(root.join("crates/retrograde/py/plot_run.py"), "# fake\n").unwrap();

        let workspace = root.join("crates/ddrs/.ddrs");
        let run_dir = workspace.join("runs").join(RUN_ID);
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            run_dir.join("manifest.json"),
            r#"{"run_id":"x","status":"ok"}"#,
        )
        .unwrap();

        let fake_dir = tmp.path().join("fake-bin");
        fs::create_dir_all(&fake_dir).unwrap();
        let uv = fake_dir.join("uv");
        fs::write(
            &uv,
            FAKE_UV.replace("__FAKE_DIR__", fake_dir.to_str().unwrap()),
        )
        .unwrap();
        fs::set_permissions(&uv, fs::Permissions::from_mode(0o755)).unwrap();

        Fixture {
            _tmp: tmp,
            root,
            fake_dir,
            workspace,
        }
    }

    fn options(&self) -> PlotOptions {
        PlotOptions {
            run_id: RUN_ID.to_string(),
            root: Some(self.root.clone()),
            workspace: None,
        }
    }

    /// Prepend the fake `uv` to `PATH` for the duration of the call.
    fn with_fake_uv<T>(&self, body: impl FnOnce() -> T) -> T {
        let _guard = path_lock();
        let original = std::env::var_os("PATH");
        let mut dirs = vec![self.fake_dir.clone()];
        if let Some(p) = &original {
            dirs.extend(std::env::split_paths(p));
        }
        std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
        let out = body();
        match original {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        out
    }

    /// PATH with nothing on it at all: `uv` cannot be found.
    fn without_uv<T>(&self, body: impl FnOnce() -> T) -> T {
        let _guard = path_lock();
        let original = std::env::var_os("PATH");
        std::env::set_var("PATH", self._tmp.path().join("empty-bin"));
        let out = body();
        match original {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        out
    }

    fn argv(&self) -> Vec<String> {
        let text = fs::read_to_string(self.fake_dir.join("argv.txt")).expect("uv never ran");
        text.lines().map(str::to_string).collect()
    }

    fn uv_ran(&self) -> bool {
        self.fake_dir.join("argv.txt").is_file()
    }

    fn exit_with(&self, code: i32) {
        fs::write(self.fake_dir.join("exit_code"), code.to_string()).unwrap();
    }
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn runs_uv_against_the_py_project_and_the_run_directory() {
    let fx = Fixture::new();
    let code = fx.with_fake_uv(|| plot(&fx.options())).expect("plot");
    assert_eq!(code, 0);

    let project = canonical(&fx.root.join("crates/retrograde/py"));
    let script = format!("{project}/plot_run.py");
    let run_dir = canonical(&fx.workspace.join("runs").join(RUN_ID));
    assert_eq!(
        fx.argv(),
        vec!["run", "--project", &project, "python", &script, &run_dir]
    );
}

#[test]
fn an_explicit_workspace_is_where_the_run_is_looked_up() {
    let fx = Fixture::new();
    let elsewhere = fx._tmp.path().join("other-workspace");
    let run_dir = elsewhere.join("runs").join(RUN_ID);
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(run_dir.join("manifest.json"), "{}").unwrap();

    let mut opts = fx.options();
    opts.workspace = Some(elsewhere.clone());
    fx.with_fake_uv(|| plot(&opts)).expect("plot");

    assert_eq!(fx.argv().last().unwrap(), &canonical(&run_dir));
}

#[test]
fn the_scripts_exit_code_passes_through() {
    let fx = Fixture::new();
    fx.exit_with(3);
    let code = fx.with_fake_uv(|| plot(&fx.options())).expect("plot");
    assert_eq!(code, 3);
}

#[test]
fn an_unknown_run_id_is_an_error_and_never_launches_uv() {
    let fx = Fixture::new();
    let mut opts = fx.options();
    opts.run_id = "no-such-run".to_string();

    let error = fx
        .with_fake_uv(|| plot(&opts))
        .expect_err("an unknown run must not be plotted")
        .to_string();
    assert!(error.contains("no-such-run"), "{error}");
    assert!(error.contains("runs"), "{error}");
    // Not just any failure to touch the path: the message that says what
    // to do about it. A bare canonicalize error would also name the path.
    assert!(error.contains("retrograde view"), "{error}");
    assert!(!fx.uv_ran(), "uv ran for a run that does not exist");
}

#[test]
fn a_run_id_that_is_not_one_path_component_is_refused() {
    let fx = Fixture::new();
    for id in ["..", "../../etc", "a/b", ""] {
        let mut opts = fx.options();
        opts.run_id = id.to_string();
        let error = fx
            .with_fake_uv(|| plot(&opts))
            .expect_err(&format!("{id} must be refused"))
            .to_string();
        assert!(error.contains("run id"), "{id}: {error}");
        assert!(!fx.uv_ran(), "uv ran for run id {id}");
    }
}

#[test]
fn a_missing_uv_says_so() {
    let fx = Fixture::new();
    let error = fx
        .without_uv(|| plot(&fx.options()))
        .expect_err("uv is not on PATH")
        .to_string();
    assert!(error.contains("uv"), "{error}");
    assert!(
        error.contains("PATH") || error.contains("install"),
        "the error should say how to fix it: {error}"
    );
}

#[test]
fn a_missing_py_project_is_an_error_naming_it() {
    let fx = Fixture::new();
    fs::remove_file(fx.root.join("crates/retrograde/py/plot_run.py")).unwrap();
    let error = fx
        .with_fake_uv(|| plot(&fx.options()))
        .expect_err("no script to run")
        .to_string();
    assert!(error.contains("plot_run.py"), "{error}");
    assert!(!fx.uv_ran(), "uv ran without a script to run");
}
