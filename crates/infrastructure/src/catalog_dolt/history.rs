//! A draft's history (decision 0021): the commits its branch has and
//! `main` has not, newest first, from where it left `main`; and what a
//! dataset's files were and became between two of them. Verified in
//! `tools/verify/dolt_draft_history.sh` (Dolt 2.6.1): `DOLT_LOG('main..b',
//! '--parents')`, `DOLT_MERGE_BASE`, `DOLT_DIFF(from, to, table)`; the
//! range of DOLT_LOG is bound, DOLT_DIFF's commits are literals (it takes
//! no bound value).
//!
//! A file's history on `main` (decision 0050) walks `main` by first parents
//! and marks where the file differs from its parent's: `dolt_history_*`
//! has the file at every commit, branches' included, and DOLT_LOG has no
//! `--first-parent` (`tools/verify/dolt_column_drafts.sh`).

use sqlx::Row;

use super::DoltCatalog;

/// One commit of a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftCommit {
    pub commit: String,
    /// The commit it follows (the first follows `main`'s).
    pub parent: String,
    /// `Name <email>`.
    pub author: String,
    pub at: String,
    pub message: String,
}

/// A draft's commits, newest first, and the commit of `main` it left from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftHistory {
    pub base: String,
    pub commits: Vec<DraftCommit>,
}

/// A file of a dataset before and after; `None` where it was not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// A rules file's name, or `mapping` for the dataset's mapping.
    pub file: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// A change to one file on `main`: the commit that made it, and the file
/// before and after; `None` where it was not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainChange {
    pub commit: DraftCommit,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The changes to a file along `main`'s first parents from `tip`, newest
/// first, at most `limit`: `log` is every commit (its first parent in
/// `parent`), `bodies` the file at each commit that has it.
pub fn first_parent_changes(
    log: &[DraftCommit],
    bodies: &std::collections::HashMap<String, String>,
    tip: &str,
    limit: usize,
) -> Vec<MainChange> {
    let by_hash: std::collections::HashMap<&str, &DraftCommit> =
        log.iter().map(|c| (c.commit.as_str(), c)).collect();
    let mut out = Vec::new();
    let mut at = by_hash.get(tip).copied();
    while let Some(commit) = at {
        if out.len() >= limit {
            break;
        }
        let after = bodies.get(&commit.commit);
        let before = bodies.get(&commit.parent);
        if after != before {
            out.push(MainChange {
                commit: commit.clone(),
                before: before.cloned(),
                after: after.cloned(),
            });
        }
        at = by_hash.get(commit.parent.as_str()).copied();
    }
    out
}

/// A commit or a branch written into SQL: DOLT_DIFF takes literals only,
/// no bound value (Dolt 2.6.1), so only a hash or a branch's characters
/// (letters, digits, `/`, `_`, `-`) are let through.
fn literal(reference: &str) -> Result<String, sqlx::Error> {
    let safe = !reference.is_empty()
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-'));
    if safe {
        Ok(format!("'{reference}'"))
    } else {
        Err(sqlx::Error::Protocol(format!(
            "not a commit or branch: {reference:?}"
        )))
    }
}

/// A commit's hash as Dolt writes them: base 32, lowercase.
pub fn is_commit(hash: &str) -> bool {
    hash.len() == 32
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// A row of `DOLT_LOG(…, '--parents')` as a commit, its first parent kept.
fn commit_of(r: &sqlx::mysql::MySqlRow) -> Result<DraftCommit, sqlx::Error> {
    let parents: String = r.try_get("parents")?;
    Ok(DraftCommit {
        commit: r.try_get("commit_hash")?,
        parent: parents
            .split(',')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
        author: format!(
            "{} <{}>",
            r.try_get::<String, _>("committer")?,
            r.try_get::<String, _>("email")?
        ),
        at: r.try_get("at")?,
        message: r.try_get("message")?,
    })
}

impl DoltCatalog {
    /// The history of draft `branch`; `None` if there is no such branch.
    pub async fn history(
        &self,
        database: &str,
        branch: &str,
    ) -> Result<Option<DraftHistory>, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let there: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_branches WHERE name = ?")
            .bind(branch)
            .fetch_one(&pool)
            .await?;
        if there == 0 {
            return Ok(None);
        }
        let base: String = sqlx::query_scalar("SELECT DOLT_MERGE_BASE('main', ?)")
            .bind(branch)
            .fetch_one(&pool)
            .await?;
        let rows = sqlx::query(
            "SELECT commit_hash, parents, committer, email, CAST(date AS CHAR) AS at, message
               FROM DOLT_LOG(?, '--parents')",
        )
        // One argument: DOLT_LOG takes literals and bound values, not an
        // expression such as CONCAT.
        .bind(format!("main..{branch}"))
        .fetch_all(&pool)
        .await?;
        let commits = rows
            .iter()
            .map(commit_of)
            .collect::<Result<_, sqlx::Error>>()?;
        Ok(Some(DraftHistory { base, commits }))
    }

    /// The changes on `main` to `dataset`'s rules file `file`, or to its
    /// mapping without one, newest first, at most `limit`.
    pub async fn file_history(
        &self,
        database: &str,
        dataset: &str,
        file: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MainChange>, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let tip: String = sqlx::query_scalar("SELECT HASHOF('main')")
            .fetch_one(&pool)
            .await?;
        let rows = sqlx::query(
            "SELECT commit_hash, parents, committer, email, CAST(date AS CHAR) AS at, message
               FROM DOLT_LOG('main', '--parents')",
        )
        .fetch_all(&pool)
        .await?;
        let log = rows
            .iter()
            .map(commit_of)
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let bodies = match file {
            Some(file) => sqlx::query_as::<_, (String, String)>(
                "SELECT commit_hash, body FROM dolt_history_rules WHERE dataset = ? AND file = ?",
            )
            .bind(dataset)
            .bind(file),
            None => sqlx::query_as::<_, (String, String)>(
                "SELECT commit_hash, body FROM dolt_history_mappings WHERE dataset = ?",
            )
            .bind(dataset),
        }
        .fetch_all(&pool)
        .await?
        .into_iter()
        .collect();
        Ok(first_parent_changes(&log, &bodies, &tip, limit))
    }

    /// What `dataset`'s mapping and rules files were at `from` and became
    /// at `to` (commits or branches), the files that changed only.
    pub async fn changes(
        &self,
        database: &str,
        from: &str,
        to: &str,
        dataset: &str,
    ) -> Result<Vec<FileChange>, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let (from, to) = (literal(from)?, literal(to)?);
        let mapping = sqlx::query(&format!(
            "SELECT from_body, to_body FROM DOLT_DIFF({from}, {to}, 'mappings')
              WHERE COALESCE(to_dataset, from_dataset) = ?"
        ))
        .bind(dataset)
        .fetch_all(&pool)
        .await?;
        let rules = sqlx::query(&format!(
            "SELECT COALESCE(to_file, from_file) AS file, from_body, to_body
               FROM DOLT_DIFF({from}, {to}, 'rules')
              WHERE COALESCE(to_dataset, from_dataset) = ?
              ORDER BY file"
        ))
        .bind(dataset)
        .fetch_all(&pool)
        .await?;
        let mut out = Vec::new();
        for r in &mapping {
            out.push(FileChange {
                file: "mapping".into(),
                before: r.try_get("from_body")?,
                after: r.try_get("to_body")?,
            });
        }
        for r in &rules {
            out.push(FileChange {
                file: r.try_get("file")?,
                before: r.try_get("from_body")?,
                after: r.try_get("to_body")?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parent: &str) -> DraftCommit {
        DraftCommit {
            commit: hash.into(),
            parent: parent.into(),
            author: "A <a@example.org>".into(),
            at: String::new(),
            message: hash.into(),
        }
    }

    #[test]
    fn a_files_history_follows_mains_first_parents_only() {
        // main: m1 → m2 (merges a's branch, b1) → m3 (merges b's, b2).
        // b1 changed the file on a branch; m2 brought it; m3 did not touch it.
        let log = vec![
            commit("m3", "m2"),
            commit("b2", "m1"),
            commit("m2", "m1"),
            commit("b1", "m1"),
            commit("m1", "m0"),
            commit("m0", ""),
        ];
        let bodies = [
            ("m1", "v1"),
            ("b1", "v2"),
            ("m2", "v2"),
            ("b2", "v1"),
            ("m3", "v2"),
        ]
        .into_iter()
        .map(|(h, b)| (h.to_string(), b.to_string()))
        .collect();
        let changes = first_parent_changes(&log, &bodies, "m3", 10);
        let seen: Vec<_> = changes
            .iter()
            .map(|c| {
                (
                    c.commit.commit.as_str(),
                    c.before.as_deref(),
                    c.after.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [("m2", Some("v1"), Some("v2")), ("m1", None, Some("v1"))]
        );
        assert_eq!(first_parent_changes(&log, &bodies, "m3", 1).len(), 1);
    }
}
