//! `retrograde` CLI. Argument parsing only; the work is in the library.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use retrograde::view::{serve, ViewOptions};
use retrograde::{check, sweep, CheckOptions, SweepOptions};

#[derive(Parser)]
#[command(
    name = "retrograde",
    about = "The evenflow experiment operator: run an experiment's arms x \
             seeds through ddrs, record the results, and check them against \
             what the experiment claims."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run every arm x seed of an experiment through ddrs, writing
    /// results/<arm>/seed-<s>/ and results/summary.csv.
    Sweep {
        /// Path to experiments/<name>/experiment.yaml.
        experiment: PathBuf,
        /// Only run cells that are `failed` or `pending`.
        #[arg(long)]
        only_failed: bool,
        /// Workspace root (default: nearest Cargo.toml with [workspace]).
        #[arg(long)]
        root: Option<PathBuf>,
        /// ddrs binary (default: <root>/target/release/ddrs).
        #[arg(long)]
        ddrs: Option<PathBuf>,
        /// ddrs workspace (default: <root>/crates/ddrs/.ddrs).
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// Passed through to `ddrs run`.
        #[arg(long, value_parser = ["cpu", "cuda"])]
        backend: Option<String>,
    },
    /// Compare every done cell's metrics against the experiment's
    /// `expected:` block, within each arm's absolute tolerance.
    Check {
        /// Path to experiments/<name>/experiment.yaml.
        experiment: PathBuf,
    },
    /// Serve a read-only feed of the experiments and the workspace's runs
    /// on 127.0.0.1, with a profile page per run.
    View {
        /// Workspace root holding experiments/ (default: nearest
        /// Cargo.toml with [workspace]).
        #[arg(long)]
        root: Option<PathBuf>,
        /// ddrs workspace (default: <root>/crates/ddrs/.ddrs).
        #[arg(long)]
        workspace: Option<PathBuf>,
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Sweep {
            experiment,
            only_failed,
            root,
            ddrs,
            workspace,
            backend,
        } => {
            let opts = SweepOptions {
                experiment,
                only_failed,
                root,
                ddrs,
                workspace,
                backend,
            };
            match sweep(&opts) {
                Ok(outcome) => {
                    print!("{}", outcome.table);
                    if outcome.all_done {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Cmd::View {
            root,
            workspace,
            port,
        } => {
            let opts = ViewOptions {
                root,
                workspace,
                port,
            };
            // `serve` only returns when the listener fails; Ctrl-C ends the
            // process without coming back through here.
            match serve(&opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Cmd::Check { experiment } => match check(&CheckOptions { experiment }) {
            Ok(outcome) => {
                print!("{}", outcome.report);
                if outcome.passed {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        },
    }
}
