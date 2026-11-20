//! A person's edit of one rules file (decisions 0019 and 0021): saved on a
//! draft's branch, made from `main` on the first save, one commit per save
//! as its author. Only that file's row changes, so the draft's diff is the
//! edit and nothing else; `main` is untouched until it is approved. A file
//! the draft lacks is added only when the edit says it may (a field's own
//! file, decision 0044).

use super::DoltCatalog;

/// One rules file to save on a draft.
#[derive(Debug, Clone, Copy)]
pub struct FileEdit<'a> {
    pub branch: &'a str,
    pub dataset: &'a str,
    pub file: &'a str,
    pub body: &'a str,
    /// `Name <email>`.
    pub author: &'a str,
    /// May add the file if the draft does not have it.
    pub adds: bool,
}

/// What saving an edit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSaved {
    /// The draft's commit after it (the same if nothing changed).
    Saved(String),
    /// No draft of that name, and none was to be made.
    NoDraft,
    /// The dataset has no such rules file, and it was not to be added:
    /// nothing was made.
    NoFile,
}

impl DoltCatalog {
    /// Saves `edit` on its draft; `create` makes the draft from `main` if
    /// it is not there. A file that is not there is added if `edit.adds`;
    /// otherwise a draft made for it is removed again.
    pub async fn save_file(
        &self,
        database: &str,
        edit: &FileEdit<'_>,
        create: bool,
    ) -> Result<FileSaved, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let mut conn = pool.acquire().await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        let there: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_branches WHERE name = ?")
            .bind(edit.branch)
            .fetch_one(&mut *conn)
            .await?;
        let made = there == 0;
        if made && !create {
            return Ok(FileSaved::NoDraft);
        }
        if made {
            sqlx::query("CALL DOLT_BRANCH(?)")
                .bind(edit.branch)
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("CALL DOLT_CHECKOUT(?)")
            .bind(edit.branch)
            .execute(&mut *conn)
            .await?;
        let file: i64 =
            sqlx::query_scalar("SELECT count(*) FROM rules WHERE dataset = ? AND file = ?")
                .bind(edit.dataset)
                .bind(edit.file)
                .fetch_one(&mut *conn)
                .await?;
        if file == 0 && edit.adds {
            sqlx::query("INSERT INTO rules (dataset, file, body) VALUES (?, ?, '')")
                .bind(edit.dataset)
                .bind(edit.file)
                .execute(&mut *conn)
                .await?;
        } else if file == 0 {
            sqlx::query("CALL DOLT_CHECKOUT('main')")
                .execute(&mut *conn)
                .await?;
            if made {
                sqlx::query("CALL DOLT_BRANCH('-D', ?)")
                    .bind(edit.branch)
                    .execute(&mut *conn)
                    .await?;
            }
            return Ok(FileSaved::NoFile);
        }
        sqlx::query("UPDATE rules SET body = ? WHERE dataset = ? AND file = ?")
            .bind(edit.body)
            .bind(edit.dataset)
            .bind(edit.file)
            .execute(&mut *conn)
            .await?;
        let changed: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_status")
            .fetch_one(&mut *conn)
            .await?;
        if changed > 0 {
            let message = format!("catalog: edit {}/{}", edit.dataset, edit.file);
            sqlx::query("CALL DOLT_COMMIT('-am', ?, '--author', ?)")
                .bind(message)
                .bind(edit.author)
                .execute(&mut *conn)
                .await?;
        }
        let commit = sqlx::query_scalar("SELECT HASHOF(?)")
            .bind(edit.branch)
            .fetch_one(&mut *conn)
            .await?;
        Ok(FileSaved::Saved(commit))
    }
}

/// Several rules files to save on a new draft at once: each new text, or
/// `None` to remove it (decision 0044, _Split by field_).
#[derive(Debug, Clone, Copy)]
pub struct FilesEdit<'a> {
    pub branch: &'a str,
    pub dataset: &'a str,
    pub files: &'a [(String, Option<String>)],
    /// The dataset's mapping's new text, if it changes (its fields named
    /// as their columns, decision 0044).
    pub mapping: Option<&'a str>,
    /// `Name <email>`.
    pub author: &'a str,
    pub message: &'a str,
}

impl DoltCatalog {
    /// Makes the draft `edit.branch` from `main` with every file of `edit`
    /// written or removed, in one commit; its commit. A draft of that name
    /// already there is refused (`None`).
    pub async fn save_files(
        &self,
        database: &str,
        edit: &FilesEdit<'_>,
    ) -> Result<Option<String>, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let mut conn = pool.acquire().await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        let there: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_branches WHERE name = ?")
            .bind(edit.branch)
            .fetch_one(&mut *conn)
            .await?;
        if there > 0 {
            return Ok(None);
        }
        sqlx::query("CALL DOLT_BRANCH(?)")
            .bind(edit.branch)
            .execute(&mut *conn)
            .await?;
        sqlx::query("CALL DOLT_CHECKOUT(?)")
            .bind(edit.branch)
            .execute(&mut *conn)
            .await?;
        for (file, body) in edit.files {
            match body {
                Some(body) => {
                    sqlx::query(
                        "INSERT INTO rules (dataset, file, body) VALUES (?, ?, ?) \
                         ON DUPLICATE KEY UPDATE body = VALUES(body)",
                    )
                    .bind(edit.dataset)
                    .bind(file)
                    .bind(body)
                    .execute(&mut *conn)
                    .await?;
                }
                None => {
                    sqlx::query("DELETE FROM rules WHERE dataset = ? AND file = ?")
                        .bind(edit.dataset)
                        .bind(file)
                        .execute(&mut *conn)
                        .await?;
                }
            }
        }
        if let Some(mapping) = edit.mapping {
            sqlx::query("UPDATE mappings SET body = ? WHERE dataset = ?")
                .bind(mapping)
                .bind(edit.dataset)
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("CALL DOLT_COMMIT('-Am', ?, '--author', ?, '--allow-empty')")
            .bind(edit.message)
            .bind(edit.author)
            .execute(&mut *conn)
            .await?;
        let commit = sqlx::query_scalar("SELECT HASHOF(?)")
            .bind(edit.branch)
            .fetch_one(&mut *conn)
            .await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        Ok(Some(commit))
    }
}

/// A dataset's mapping, edited on a draft: a field added to it (decision
/// 0043, _Add to the mapping_).
#[derive(Debug, Clone, Copy)]
pub struct MappingEdit<'a> {
    pub branch: &'a str,
    pub dataset: &'a str,
    pub body: &'a str,
    /// `Name <email>`.
    pub author: &'a str,
    /// May start the dataset if the draft has no mapping for it.
    pub starts: bool,
}

impl DoltCatalog {
    /// Saves `edit` as the dataset's mapping on its draft, or on `main`
    /// itself (decision 0047), one commit; `create` makes the draft from
    /// `main` if it is not there. A dataset
    /// the draft lacks is started if `edit.starts`; otherwise nothing
    /// changes (`NoFile`) and a draft made for it is removed again.
    pub async fn save_mapping(
        &self,
        database: &str,
        edit: &MappingEdit<'_>,
        create: bool,
    ) -> Result<FileSaved, sqlx::Error> {
        let (pool, _entry) = self.pool(database).await?;
        let mut conn = pool.acquire().await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        let there: i64 = sqlx::query_scalar("SELECT count(*) FROM dolt_branches WHERE name = ?")
            .bind(edit.branch)
            .fetch_one(&mut *conn)
            .await?;
        let made = there == 0;
        if made && !create {
            return Ok(FileSaved::NoDraft);
        }
        if made {
            sqlx::query("CALL DOLT_BRANCH(?)")
                .bind(edit.branch)
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("CALL DOLT_CHECKOUT(?)")
            .bind(edit.branch)
            .execute(&mut *conn)
            .await?;
        let mapped: i64 = sqlx::query_scalar("SELECT count(*) FROM mappings WHERE dataset = ?")
            .bind(edit.dataset)
            .fetch_one(&mut *conn)
            .await?;
        if mapped == 0 && !edit.starts {
            sqlx::query("CALL DOLT_CHECKOUT('main')")
                .execute(&mut *conn)
                .await?;
            if made {
                sqlx::query("CALL DOLT_BRANCH('-D', ?)")
                    .bind(edit.branch)
                    .execute(&mut *conn)
                    .await?;
            }
            return Ok(FileSaved::NoFile);
        }
        if mapped == 0 {
            sqlx::query("INSERT INTO datasets (name, description) VALUES (?, NULL)")
                .bind(edit.dataset)
                .execute(&mut *conn)
                .await?;
        }
        sqlx::query("REPLACE INTO mappings (dataset, body) VALUES (?, ?)")
            .bind(edit.dataset)
            .bind(edit.body)
            .execute(&mut *conn)
            .await?;
        let message = format!("catalog: edit {}/mapping", edit.dataset);
        sqlx::query("CALL DOLT_COMMIT('-Am', ?, '--author', ?, '--allow-empty')")
            .bind(message)
            .bind(edit.author)
            .execute(&mut *conn)
            .await?;
        let commit = sqlx::query_scalar("SELECT HASHOF(?)")
            .bind(edit.branch)
            .fetch_one(&mut *conn)
            .await?;
        sqlx::query("CALL DOLT_CHECKOUT('main')")
            .execute(&mut *conn)
            .await?;
        Ok(FileSaved::Saved(commit))
    }
}
