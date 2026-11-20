//! Drafts of a tenant's catalog (decisions 0021 and 0038): a branch made
//! from `main`, written on, then approved (merged into `main`, the approver
//! its author) or discarded. A merge runs in a transaction: with conflicts
//! it is rolled back and `main` stays as it was
//! (`tools/verify/dolt-sqlx`).

use sqlx::Row;

use super::{write, DatasetImport, DoltCatalog};

/// A draft: its branch and its last commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub branch: String,
    pub commit: String,
    /// The last commit's author, `Name <email>`.
    pub author: String,
    /// When, as Dolt gives it.
    pub at: String,
}

/// What approving a draft did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// Into `main`: its commit after the merge.
    Merged(String),
    /// It conflicts with `main` in these tables; nothing changed.
    Conflicts(Vec<String>),
    /// `main` changed the dataset after the draft was made: a draft proposes
    /// the dataset whole, so it is proposed again rather than merged cell
    /// by cell into a dataset nobody reviewed; nothing changed.
    Stale,
    NotFound,
}

impl DoltCatalog {
    /// Writes a dataset's mapping and rules on a new branch `branch`, made
    /// from `main`: a draft (decisions 0021 and 0038); `main` is untouched.
    /// Returns the draft's commit; `None` if a branch of that name is
    /// already there: a draft is never written over, and a repeated report
    /// finds its draft made.
    pub async fn draft(
        &self,
        database: &str,
        branch: &str,
        import: &DatasetImport<'_>,
        author: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let mut conn = pool.acquire().await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        let there: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_branches WHERE name = ?")
            .bind(branch)
            .fetch_one(&mut *conn)
            .await?;
        if there > 0 {
            return Ok(None);
        }
        sqlx::query("CALL DOLT_BRANCH(?)")
            .bind(branch)
            .execute(&mut *conn)
            .await?;
        sqlx::query("CALL DOLT_CHECKOUT(?)")
            .bind(branch)
            .execute(&mut *conn)
            .await?;
        write(
            &mut conn,
            import,
            author,
            &format!("catalog: propose {}", import.dataset),
        )
        .await?;
        sqlx::query_scalar("SELECT HASHOF(?)")
            .bind(branch)
            .fetch_one(&mut *conn)
            .await
    }

    /// The drafts whose branch starts with `prefix` (`proposal/`), by name.
    pub async fn drafts(&self, database: &str, prefix: &str) -> Result<Vec<Draft>, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let rows = sqlx::query(
            "SELECT name, hash, latest_committer, latest_committer_email,
                    CAST(latest_commit_date AS CHAR) AS at
               FROM dolt_branches WHERE name LIKE CONCAT(?, '%') ORDER BY name",
        )
        .bind(prefix)
        .fetch_all(&pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Draft {
                    branch: r.try_get("name")?,
                    commit: r.try_get("hash")?,
                    author: format!(
                        "{} <{}>",
                        r.try_get::<String, _>("latest_committer")?,
                        r.try_get::<String, _>("latest_committer_email")?
                    ),
                    at: r.try_get("at")?,
                })
            })
            .collect()
    }

    /// Merges the draft `branch` of `dataset` into `main` as `author`
    /// (`--no-ff`: the approval is its own commit), then deletes the branch;
    /// refused if `main` changed the dataset since the draft was made; with
    /// conflicts, rolled back, saying in which tables.
    pub async fn approve(
        &self,
        database: &str,
        branch: &str,
        dataset: &str,
        author: &str,
        message: &str,
    ) -> Result<Merged, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let mut conn = pool.acquire().await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        let there: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_branches WHERE name = ?")
            .bind(branch)
            .fetch_one(&mut *conn)
            .await?;
        if there == 0 {
            return Ok(Merged::NotFound);
        }
        let base: String = sqlx::query_scalar("SELECT DOLT_MERGE_BASE(?, 'main')")
            .bind(branch)
            .fetch_one(&mut *conn)
            .await?;
        if !base.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(sqlx::Error::Protocol(format!(
                "unexpected commit hash {base:?}"
            )));
        }
        // A column's draft (decision 0050) is stale only if `main` changed
        // its file; a draft of the dataset, if `main` changed the dataset.
        let only = column_file(self, database, &base, branch, dataset).await?;
        if main_changed(&mut conn, &base, dataset, only.as_deref()).await? {
            return Ok(Merged::Stale);
        }
        sqlx::query("SET @@dolt_allow_commit_conflicts = 1")
            .execute(&mut *conn)
            .await?;
        sqlx::query("START TRANSACTION").execute(&mut *conn).await?;
        let merged = sqlx::query("CALL DOLT_MERGE(?, '--no-ff', '-m', ?, '--author', ?)")
            .bind(branch)
            .bind(message)
            .bind(author)
            .fetch_one(&mut *conn)
            .await;
        let conflicts: i64 = match &merged {
            Ok(row) => row.try_get(2)?,
            Err(_) => 0,
        };
        let outcome = if merged.is_err() || conflicts > 0 {
            let tables: Vec<String> = if conflicts > 0 {
                sqlx::query_scalar("SELECT `table` FROM dolt_conflicts")
                    .fetch_all(&mut *conn)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            sqlx::query("ROLLBACK").execute(&mut *conn).await?;
            sqlx::query("SET @@dolt_allow_commit_conflicts = 0")
                .execute(&mut *conn)
                .await?;
            merged?;
            Merged::Conflicts(tables)
        } else {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            sqlx::query("SET @@dolt_allow_commit_conflicts = 0")
                .execute(&mut *conn)
                .await?;
            sqlx::query("CALL DOLT_BRANCH('-d', ?)")
                .bind(branch)
                .execute(&mut *conn)
                .await?;
            Merged::Merged(
                sqlx::query_scalar("SELECT HASHOF('main')")
                    .fetch_one(&mut *conn)
                    .await?,
            )
        };
        Ok(outcome)
    }

    /// Deletes the draft `branch` unmerged; `false` if it is not there.
    pub async fn discard(&self, database: &str, branch: &str) -> Result<bool, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let mut conn = pool.acquire().await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        let there: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_branches WHERE name = ?")
            .bind(branch)
            .fetch_one(&mut *conn)
            .await?;
        if there == 0 {
            return Ok(false);
        }
        sqlx::query("CALL DOLT_BRANCH('-D', ?)")
            .bind(branch)
            .execute(&mut *conn)
            .await?;
        Ok(true)
    }
}

/// Whether `main` changed `dataset`'s mapping or rules after `base`.
/// The one rules file a draft changes since `base`, if that is all it
/// changes: a column's draft (decision 0050); `None` for a draft of the
/// dataset (its mapping, or several files).
async fn column_file(
    catalog: &DoltCatalog,
    database: &str,
    base: &str,
    branch: &str,
    dataset: &str,
) -> Result<Option<String>, sqlx::Error> {
    let changes = catalog.changes(database, base, branch, dataset).await?;
    Ok(match changes.as_slice() {
        [one] if one.file != "mapping" => Some(one.file.clone()),
        _ => None,
    })
}

/// Whether `main` changed, since `base`, the dataset or, given, only its
/// rules file `file`.
async fn main_changed(
    conn: &mut sqlx::pool::PoolConnection<sqlx::MySql>,
    base: &str,
    dataset: &str,
    file: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let head: String = sqlx::query_scalar("SELECT HASHOF('main')")
        .fetch_one(&mut **conn)
        .await?;
    if head == base {
        return Ok(false);
    }
    let tables: &[&str] = if file.is_some() {
        &["rules"]
    } else {
        &["mappings", "rules"]
    };
    for table in tables {
        let of_file = if file.is_some() {
            " AND COALESCE(to_file, from_file) = ?"
        } else {
            ""
        };
        let sql = format!(
            "SELECT count(*) FROM DOLT_DIFF('{base}', 'main', '{table}')
              WHERE COALESCE(to_dataset, from_dataset) = ?{of_file}"
        );
        let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(dataset);
        if let Some(file) = file {
            query = query.bind(file);
        }
        let changed = query.fetch_one(&mut **conn).await;
        match changed {
            Ok(0) => {}
            Ok(_) => return Ok(true),
            // The table is not there at `base`: `main` made it since.
            Err(sqlx::Error::Database(e)) if e.message().contains("not found") => return Ok(true),
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}
