//! `cargo xtask migrations`: no new migration may break the previous version
//! (expand/contract).
//!
//! If an update does not start, the previous version comes back on the
//! already migrated database (`run_migrations` tolerates migrations it does
//! not know). That only works if the new one did not remove or change
//! anything the previous one uses. So a new migration only adds: dropping,
//! renaming, changing a type or requiring a value goes in a separate
//! migration, once no version in use needs it, marked with
//!
//! ```sql
//! -- contract: v1.2.0 no longer uses it
//! ```
//!
//! The migrations before [`FIRST_CHECKED`] (the baseline) predate the rule.

use std::fmt;
use std::path::Path;

/// The first migration the rule applies to: everything after the baseline.
pub(crate) const FIRST_CHECKED: u32 = 2;

/// What a migration cannot do without being marked as a contraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) file: String,
    pub(crate) statement: usize,
    pub(crate) reason: &'static str,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (statement {}): {}",
            self.file, self.statement, self.reason
        )
    }
}

/// Checks the `NNNN_*.sql` migrations of `dir`.
pub(crate) fn check_dir(dir: &Path) -> Result<Vec<Finding>, String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("could not read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    let mut findings = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(number) = name.split('_').next().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if number < FIRST_CHECKED {
            continue;
        }
        let sql = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        findings.extend(check(&name, &sql));
    }
    Ok(findings)
}

/// Checks one migration. Marked as a contraction, it is accepted as is.
pub(crate) fn check(file: &str, sql: &str) -> Vec<Finding> {
    if is_contraction(sql) {
        return Vec::new();
    }
    statements(sql)
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            reasons(s).into_iter().map(move |reason| Finding {
                file: file.to_string(),
                statement: i + 1,
                reason,
            })
        })
        .collect()
}

fn is_contraction(sql: &str) -> bool {
    sql.lines().any(|l| {
        l.trim_start()
            .strip_prefix("--")
            .is_some_and(|rest| rest.trim_start().to_lowercase().starts_with("contract:"))
    })
}

/// What breaks the previous version in a statement (upper case, without
/// comments or literals).
fn reasons(t: &[String]) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    let has = |seq: &[&str]| {
        t.windows(seq.len())
            .any(|w| w.iter().zip(seq).all(|(a, b)| a == b))
    };
    for (seq, reason) in [
        (&["DROP", "TABLE"][..], "drops a table"),
        (&["DROP", "COLUMN"], "drops a column"),
        (&["DROP", "VIEW"], "drops a view"),
        (&["DROP", "MATERIALIZED"], "drops a view"),
        (&["DROP", "FUNCTION"], "drops a function"),
        (&["DROP", "TRIGGER"], "drops a trigger"),
        (&["DROP", "TYPE"], "drops a type"),
        (
            &["DROP", "INDEX"],
            "drops an index (an ON CONFLICT of the previous version may depend on it)",
        ),
        (
            &["RENAME"],
            "renames (the previous version keeps using the old name)",
        ),
        (
            &["SET", "NOT", "NULL"],
            "requires a value the previous version does not write",
        ),
    ] {
        if has(seq) {
            reasons.push(reason);
        }
    }
    // A TRUNCATE statement; a trigger's `BEFORE TRUNCATE` empties nothing.
    if t.first().is_some_and(|w| w == "TRUNCATE") {
        reasons.push("empties a table");
    }
    if drops_a_constraint(t) {
        reasons
            .push("drops a constraint (an ON CONFLICT of the previous version may depend on it)");
    }
    if changes_type(t) {
        reasons.push("changes a column's type");
    }
    if t.first().is_some_and(|w| w == "ALTER") && adds_required_without_value(t) {
        reasons
            .push("adds a NOT NULL column without DEFAULT (the previous version's INSERTs fail)");
    }
    reasons
}

/// A `DROP CONSTRAINT c` the same statement does not add again as a
/// `CHECK` (`ADD CONSTRAINT c CHECK …`): replacing a check, to widen it,
/// leaves nothing an `ON CONFLICT` needs, which is an index.
fn drops_a_constraint(t: &[String]) -> bool {
    let named = |i: usize| {
        let at = i + 2 + usize::from(t.get(i + 2).is_some_and(|w| w == "IF"));
        let at = if t.get(at).is_some_and(|w| w == "EXISTS") {
            at + 1
        } else {
            at
        };
        t.get(at)
    };
    t.windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] == "DROP" && w[1] == "CONSTRAINT")
        .any(|(i, _)| {
            let name = named(i);
            !t.windows(4).any(|w| {
                w[0] == "ADD" && w[1] == "CONSTRAINT" && Some(&w[2]) == name && w[3] == "CHECK"
            })
        })
}

/// `ALTER COLUMN c TYPE …` or `ALTER COLUMN c SET DATA TYPE …`.
fn changes_type(t: &[String]) -> bool {
    t.windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] == "ALTER" && w[1] == "COLUMN")
        .any(|(i, _)| {
            let rest = &t[i + 2..];
            matches!(rest.get(1).map(String::as_str), Some("TYPE"))
                || matches!(
                    (rest.get(1), rest.get(2), rest.get(3)),
                    (Some(a), Some(b), Some(c)) if a == "SET" && b == "DATA" && c == "TYPE"
                )
        })
}

/// An `ADD [COLUMN] … NOT NULL` action without `DEFAULT` or `GENERATED`.
fn adds_required_without_value(t: &[String]) -> bool {
    // The actions of an ALTER TABLE are separated by commas outside parentheses.
    let mut actions: Vec<&[String]> = Vec::new();
    let (mut from, mut depth) = (0, 0i32);
    for (i, w) in t.iter().enumerate() {
        match w.as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            "," if depth == 0 => {
                actions.push(&t[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    actions.push(&t[from..]);
    actions.iter().any(|a| {
        let Some(add) = a.iter().position(|w| w == "ADD") else {
            return false;
        };
        let a = &a[add..];
        let is_constraint = a.get(1).is_some_and(|w| {
            matches!(
                w.as_str(),
                "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "CHECK" | "FOREIGN" | "EXCLUDE"
            )
        });
        !is_constraint
            && a.windows(2).any(|w| w[0] == "NOT" && w[1] == "NULL")
            && !a.iter().any(|w| w == "DEFAULT" || w == "GENERATED")
    })
}

/// A script's statements, each as upper-case words and loose symbols.
/// Comments, literals ('…') and `$$` bodies (functions) are skipped; quoted
/// identifiers are kept as they are.
fn statements(sql: &str) -> Vec<Vec<String>> {
    let c: Vec<char> = sql.chars().collect();
    let mut all = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '-' && c.get(i + 1) == Some(&'-') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if ch == '\'' {
            i += 1;
            while i < c.len() {
                if c[i] == '\'' {
                    if c.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            current.push("'".into());
        } else if ch == '$' {
            // $tag$ … $tag$
            let mut j = i + 1;
            while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_') {
                j += 1;
            }
            if c.get(j) == Some(&'$') {
                let tag: String = c[i..=j].iter().collect();
                let body: String = c[j + 1..].iter().collect();
                let end = body.find(&tag).map_or(c.len(), |p| {
                    j + 1 + body[..p].chars().count() + tag.chars().count()
                });
                i = end;
                current.push("$$".into());
            } else {
                i += 1;
            }
        } else if ch == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' {
                j += 1;
            }
            current.push(c[i..(j + 1).min(c.len())].iter().collect());
            i = j + 1;
        } else if ch == ';' {
            if !current.is_empty() {
                all.push(std::mem::take(&mut current));
            }
            i += 1;
        } else if ch.is_alphanumeric() || ch == '_' {
            let mut j = i;
            while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_') {
                j += 1;
            }
            current.push(c[i..j].iter().collect::<String>().to_uppercase());
            i = j;
        } else {
            if !ch.is_whitespace() {
                current.push(ch.to_string());
            }
            i += 1;
        }
    }
    if !current.is_empty() {
        all.push(current);
    }
    all
}

#[cfg(test)]
mod tests;
