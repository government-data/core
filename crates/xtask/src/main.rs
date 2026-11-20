//! Workspace automation entry point (the `cargo xtask` pattern).
//!
//! Keeping dev/CI chores here — rather than in shell scripts scattered across
//! the repo — gives the monorepo a single, discoverable task runner.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod migrations;
mod sizes;

/// The files allowed above their hard limit, relative to the repository root.
pub(crate) const SIZES_EXCEPTIONS: &str = "crates/xtask/sizes-exceptions.txt";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let task = args.next();
    match task.as_deref() {
        Some("help") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("migrations") => {
            // One directory, or every crate's `migrations/`: infrastructure's
            // and the feature modules' (contracts::Module).
            let dirs = match args.next() {
                Some(dir) => vec![PathBuf::from(dir)],
                None => migration_dirs(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..")),
            };
            let checked: Result<Vec<_>, _> =
                dirs.iter().map(|d| migrations::check_dir(d)).collect();
            match checked.map(|all| all.into_iter().flatten().collect::<Vec<_>>()) {
                Ok(findings) if findings.is_empty() => {
                    println!("✓ the new migrations only add");
                    ExitCode::SUCCESS
                }
                Ok(findings) => {
                    for f in &findings {
                        eprintln!("✕ {f}");
                    }
                    eprintln!(
                        "\nA migration cannot remove or change anything the previous version \
                         uses: if the update does not start, the previous version comes back on \
                         this database.\nAdd now, and contract in a separate migration once no \
                         version in use needs it, marked with `-- contract: vX.Y.Z no longer \
                         uses it`."
                    );
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("xtask: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("sizes") => {
            let root = args.next().map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
                PathBuf::from,
            );
            match run_sizes(&root) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("xtask: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(other) => {
            eprintln!("xtask: unknown task `{other}`\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

/// `cargo xtask sizes`: prints the files above their target and fails on
/// the hard limits (decision 0034).
fn run_sizes(root: &Path) -> Result<ExitCode, String> {
    let path = root.join(SIZES_EXCEPTIONS);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let exceptions = sizes::parse_exceptions(&text)?;
    let report = sizes::check(&sizes::measure(root)?, &exceptions);
    if !report.above_target.is_empty() {
        println!("Above their target (split them when you touch them):");
        for f in &report.above_target {
            let excepted = exceptions.iter().any(|e| e.path == f.path);
            println!(
                "  {:>5} / {:<3} {}{}",
                f.lines,
                f.kind.target(),
                f.path,
                if excepted { "  (exception)" } else { "" }
            );
        }
        println!();
    }
    if report.failures.is_empty() {
        println!(
            "✓ {} files within their hard limits ({} above target)",
            report.checked,
            report.above_target.len()
        );
        return Ok(ExitCode::SUCCESS);
    }
    for f in &report.failures {
        eprintln!("✕ {f}");
    }
    eprintln!(
        "\nA Rust file stops at 800 lines and a TypeScript file at 600 (decision 0034): split \
         it by responsibility. An exception goes in {SIZES_EXCEPTIONS}, with its reason, only \
         while the split is pending."
    );
    Ok(ExitCode::FAILURE)
}

fn print_help() {
    println!(
        "\
Usage: cargo xtask <task>

Tasks:
    help                 Show this message
    migrations [DIR]     Check that new migrations only add (expand/contract)
    sizes [ROOT]         Check file sizes against their limits (decision 0034)"
    );
}

/// The migrations on the platform's database: infrastructure's and those of
/// the feature modules (the crates that implement `contracts::Module`), in
/// name order. Other applications (the cloud's licensing) keep their own.
fn migration_dirs(crates: &Path) -> Vec<PathBuf> {
    let is_platform = |dir: &Path| {
        dir.ends_with("infrastructure")
            || std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|toml| {
                toml.contains("contracts.workspace = true") && !dir.ends_with("server")
            })
    };
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(crates)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|crate_dir| is_platform(crate_dir))
        .map(|crate_dir| crate_dir.join("migrations"))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}
