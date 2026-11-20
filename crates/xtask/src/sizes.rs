//! `cargo xtask sizes`: no source file grows past its hard limit
//! (decision 0034).
//!
//! | Kind                                   | Target | Hard limit |
//! | -------------------------------------- | ------ | ---------- |
//! | Rust file                              | 400    | 800        |
//! | TypeScript file (`.ts`, `.tsx`)        | 300    | 600        |
//! | Test file (`tests.rs`, `*_it.rs`, …)   | 800    | none       |
//!
//! A file above its hard limit fails, unless it is listed in the exceptions
//! file with a reason. An exception that is no longer needed fails too, so
//! the list only shrinks. Files above their target are listed as a summary,
//! without failing. Lines are physical lines, as `wc -l` counts them.
//! Functions are checked by Clippy (`too_many_lines`) and ESLint
//! (`max-lines-per-function`), not here.

use std::fmt;
use std::path::Path;

/// The directories walked, relative to the repository root.
pub(crate) const ROOTS: &[&str] = &["crates", "packages", "apps/web"];

/// Directories never walked: build output and dependencies.
const SKIPPED_DIRS: &[&str] = &["target", "node_modules", "dist", "coverage", ".astro"];

/// Generated files, exempt from every limit.
const GENERATED: &[&str] = &["packages/api-client/src/schema.ts"];

/// What a file is, for its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Rust,
    TypeScript,
    Test,
}

impl Kind {
    /// The size a file should stay under.
    pub(crate) fn target(self) -> usize {
        match self {
            Kind::Rust => 400,
            Kind::TypeScript => 300,
            Kind::Test => 800,
        }
    }

    /// The size above which CI fails; tests have none (split them by use case).
    pub(crate) fn hard_limit(self) -> Option<usize> {
        match self {
            Kind::Rust => Some(800),
            Kind::TypeScript => Some(600),
            Kind::Test => None,
        }
    }
}

/// Classifies a path relative to the repository root (with `/`). `None`:
/// not checked (another language, or generated).
pub(crate) fn classify(path: &str) -> Option<Kind> {
    if GENERATED.contains(&path) {
        return None;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let is_test = name == "tests.rs"
        || name.ends_with("_it.rs")
        || path.contains("/tests/")
        || [".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx"]
            .iter()
            .any(|s| name.ends_with(s));
    let is_rust = name.ends_with(".rs");
    let is_ts = (name.ends_with(".ts") || name.ends_with(".tsx")) && !name.ends_with(".d.ts");
    match (is_rust || is_ts, is_test, is_rust) {
        (false, _, _) => None,
        (true, true, _) => Some(Kind::Test),
        (true, false, true) => Some(Kind::Rust),
        (true, false, false) => Some(Kind::TypeScript),
    }
}

/// A file allowed above its hard limit, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Exception {
    pub(crate) path: String,
    pub(crate) reason: String,
}

/// Parses the exceptions file: one `path reason…` per line; `#` starts a
/// comment line. Every exception needs a reason.
pub(crate) fn parse_exceptions(text: &str) -> Result<Vec<Exception>, String> {
    let mut exceptions = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (path, reason) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(format!(
                "exceptions, line {}: `{path}` has no reason",
                i + 1
            ));
        }
        exceptions.push(Exception {
            path: path.to_string(),
            reason: reason.to_string(),
        });
    }
    Ok(exceptions)
}

/// A file and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Measured {
    pub(crate) path: String,
    pub(crate) kind: Kind,
    pub(crate) lines: usize,
}

/// What makes the check fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// A file above its hard limit, not in the exceptions.
    TooLarge {
        path: String,
        lines: usize,
        limit: usize,
    },
    /// An exception whose file is gone or back under its hard limit.
    StaleException { path: String },
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::TooLarge { path, lines, limit } => {
                write!(f, "{path}: {lines} lines (hard limit {limit}): split it")
            }
            Failure::StaleException { path } => write!(
                f,
                "{path}: listed as an exception but no longer above its hard limit: remove it \
                 from the exceptions"
            ),
        }
    }
}

/// The result of a check.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) failures: Vec<Failure>,
    /// Files above their target (excepted ones included), largest first.
    pub(crate) above_target: Vec<Measured>,
    pub(crate) checked: usize,
}

/// Checks measured files against their limits and the exceptions.
pub(crate) fn check(files: &[Measured], exceptions: &[Exception]) -> Report {
    let mut report = Report {
        checked: files.len(),
        ..Report::default()
    };
    let excepted = |p: &str| exceptions.iter().any(|e| e.path == p);
    for f in files {
        if f.lines > f.kind.target() {
            report.above_target.push(f.clone());
        }
        if let Some(limit) = f.kind.hard_limit() {
            if f.lines > limit && !excepted(&f.path) {
                report.failures.push(Failure::TooLarge {
                    path: f.path.clone(),
                    lines: f.lines,
                    limit,
                });
            }
        }
    }
    for e in exceptions {
        let still_needed = files
            .iter()
            .any(|f| f.path == e.path && f.kind.hard_limit().is_some_and(|limit| f.lines > limit));
        if !still_needed {
            report.failures.push(Failure::StaleException {
                path: e.path.clone(),
            });
        }
    }
    report
        .above_target
        .sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.path.cmp(&b.path)));
    report
}

/// Walks [`ROOTS`] under `root` and measures every checked file.
pub(crate) fn measure(root: &Path) -> Result<Vec<Measured>, String> {
    let mut files = Vec::new();
    for dir in ROOTS {
        let dir = root.join(dir);
        if dir.is_dir() {
            walk(root, &dir, &mut files)?;
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<Measured>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) {
                walk(root, &path, files)?;
            }
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let Some(kind) = classify(&rel) else {
            continue;
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        files.push(Measured {
            path: rel,
            kind,
            lines: text.lines().count(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, lines: usize) -> Measured {
        Measured {
            path: path.to_string(),
            kind: classify(path).unwrap(),
            lines,
        }
    }

    fn exception(path: &str) -> Exception {
        Exception {
            path: path.to_string(),
            reason: "split pending".to_string(),
        }
    }

    #[test]
    fn files_are_classified_by_name() {
        for (path, kind) in [
            ("crates/api/src/lib.rs", Some(Kind::Rust)),
            ("crates/api/src/settings/tests.rs", Some(Kind::Test)),
            ("crates/api/tests/users_it.rs", Some(Kind::Test)),
            ("crates/api/tests/common/mod.rs", Some(Kind::Test)),
            ("apps/web/src/App.tsx", Some(Kind::TypeScript)),
            ("packages/i18n/src/index.ts", Some(Kind::TypeScript)),
            ("packages/i18n/src/index.test.ts", Some(Kind::Test)),
            ("apps/web/src/Shell.test.tsx", Some(Kind::Test)),
            ("packages/api-client/src/schema.ts", None),
            ("apps/web/src/env.d.ts", None),
            ("crates/api/Cargo.toml", None),
        ] {
            assert_eq!(classify(path), kind, "{path}");
        }
    }

    #[test]
    fn a_file_above_its_hard_limit_fails() {
        let report = check(
            &[
                file("crates/a/src/big.rs", 801),
                file("apps/web/src/Big.tsx", 601),
                file("crates/a/src/ok.rs", 800),
            ],
            &[],
        );
        assert_eq!(
            report.failures,
            vec![
                Failure::TooLarge {
                    path: "crates/a/src/big.rs".into(),
                    lines: 801,
                    limit: 800
                },
                Failure::TooLarge {
                    path: "apps/web/src/Big.tsx".into(),
                    lines: 601,
                    limit: 600
                },
            ]
        );
    }

    #[test]
    fn tests_have_a_target_but_no_hard_limit() {
        let report = check(&[file("crates/a/tests/huge_it.rs", 2000)], &[]);
        assert!(report.failures.is_empty());
        assert_eq!(report.above_target.len(), 1);
    }

    #[test]
    fn files_above_target_are_listed_largest_first() {
        let report = check(
            &[
                file("crates/a/src/x.rs", 401),
                file("crates/a/src/y.rs", 700),
                file("crates/a/src/z.rs", 400),
                file("apps/web/src/W.tsx", 301),
            ],
            &[],
        );
        let paths: Vec<_> = report
            .above_target
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "crates/a/src/y.rs",
                "crates/a/src/x.rs",
                "apps/web/src/W.tsx"
            ]
        );
        assert!(report.failures.is_empty());
    }

    #[test]
    fn an_exception_lets_a_file_pass() {
        let report = check(
            &[file("apps/web/src/Big.tsx", 650)],
            &[exception("apps/web/src/Big.tsx")],
        );
        assert!(report.failures.is_empty());
        assert_eq!(report.above_target.len(), 1);
    }

    #[test]
    fn an_exception_no_longer_needed_fails() {
        let report = check(
            &[file("apps/web/src/Big.tsx", 500)],
            &[exception("apps/web/src/Big.tsx"), exception("gone.rs")],
        );
        assert_eq!(
            report.failures,
            vec![
                Failure::StaleException {
                    path: "apps/web/src/Big.tsx".into()
                },
                Failure::StaleException {
                    path: "gone.rs".into()
                },
            ]
        );
    }

    #[test]
    fn exceptions_need_a_reason() {
        let text = "# comment\n\napps/web/src/Big.tsx   TODO(0034): split by section\n";
        assert_eq!(
            parse_exceptions(text).unwrap(),
            vec![Exception {
                path: "apps/web/src/Big.tsx".into(),
                reason: "TODO(0034): split by section".into(),
            }]
        );
        assert!(parse_exceptions("apps/web/src/Big.tsx\n")
            .unwrap_err()
            .contains("no reason"));
    }

    #[test]
    fn the_repository_complies() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let text = std::fs::read_to_string(root.join(crate::SIZES_EXCEPTIONS)).unwrap();
        let exceptions = parse_exceptions(&text).unwrap();
        let report = check(&measure(&root).unwrap(), &exceptions);
        assert!(report.failures.is_empty(), "{:#?}", report.failures);
    }
}
