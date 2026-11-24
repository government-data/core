//! The catalog as the drafts' routes see it (decisions 0021 and
//! 0038): the port `server` implements over Dolt, and what it reads and
//! writes.

use async_trait::async_trait;
use serde::Serialize;
use utoipa::ToSchema;

use crate::draft_history::{DraftHistory, FileChange, MainChange};

/// A draft as the catalog lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct DraftInfo {
    pub branch: String,
    pub commit: String,
    pub author: String,
    pub at: String,
}

/// A draft's dataset as its branch has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftFiles {
    pub commit: String,
    pub mapping: String,
    pub rules: Vec<(String, String)>,
}

/// One rules file a person saves on a draft.
#[derive(Debug, Clone, Copy)]
pub struct FileEdit<'a> {
    pub branch: &'a str,
    pub dataset: &'a str,
    pub file: &'a str,
    pub body: &'a str,
    /// `Name <email>`.
    pub author: &'a str,
    /// May add the file if the draft does not have it: a field's own file
    /// (decision 0044).
    pub adds: bool,
}

/// Several rules files saved on a draft at once, one commit: each new
/// text, or `None` to remove the file (decision 0044, _Split by field_).
#[derive(Debug, Clone, Copy)]
pub struct FilesEdit<'a> {
    pub branch: &'a str,
    pub dataset: &'a str,
    pub files: &'a [(String, Option<String>)],
    /// The dataset's mapping's new text, if it changes (decision 0044).
    pub mapping: Option<&'a str>,
    /// `Name <email>`.
    pub author: &'a str,
    pub message: &'a str,
}

/// A dataset's mapping, edited on a draft (decision 0043).
#[derive(Debug, Clone, Copy)]
pub struct MappingEdit<'a> {
    pub branch: &'a str,
    pub dataset: &'a str,
    pub body: &'a str,
    /// `Name <email>`.
    pub author: &'a str,
    /// May start the dataset if the draft has no mapping for it: a table
    /// no dataset reads.
    pub starts: bool,
}

/// What saving a rules file did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSaved {
    /// The draft's commit after it.
    Saved(String),
    NoDraft,
    /// The dataset has no such rules file, and it was not to be added.
    NoFile,
}

/// What approving did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    Merged(String),
    Conflicts(Vec<String>),
    /// `main` changed the dataset since the draft was made.
    Stale,
    NotFound,
}

/// The catalog, for its drafts.
#[async_trait]
pub trait DraftCatalog: Send + Sync {
    async fn list(&self, prefix: &str) -> Result<Vec<DraftInfo>, String>;
    async fn read(&self, branch: &str, dataset: &str) -> Result<Option<DraftFiles>, String>;
    async fn approve(&self, branch: &str, dataset: &str, author: &str) -> Result<Approval, String>;
    async fn discard(&self, branch: &str) -> Result<bool, String>;
    /// Saves a dataset's mapping on a draft, made from `main` if `create`
    /// (decision 0043, _Add to the mapping_). A catalog that cannot says so.
    async fn save_mapping(
        &self,
        _edit: &MappingEdit<'_>,
        _create: bool,
    ) -> Result<FileSaved, String> {
        Err("this catalog edits no mapping".into())
    }
    /// Saves several rules files on a new draft made from `main`, in one
    /// commit (decision 0044). A catalog that cannot says so.
    async fn save_files(&self, _edit: &FilesEdit<'_>) -> Result<String, String> {
        Err("this catalog saves one file at a time".into())
    }
    /// Saves one rules file on a draft, made from `main` if `create`
    /// (decision 0019's editor; `crate::edit_api`).
    async fn save_file(&self, edit: &FileEdit<'_>, create: bool) -> Result<FileSaved, String>;
    /// A draft's commits and where it left `main`; `None` if there is no
    /// such branch (`crate::draft_history`). A catalog without history
    /// says so.
    async fn history(&self, _branch: &str) -> Result<Option<DraftHistory>, String> {
        Err("this catalog keeps no history".into())
    }
    /// The changes on `main` to the dataset's rules file `file`, or to its
    /// mapping without one, newest first, at most `limit` (decision 0050).
    async fn file_history(
        &self,
        _dataset: &str,
        _file: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<MainChange>, String> {
        Err("this catalog keeps no history".into())
    }
    /// Removes a dataset from `main`, as one commit by `author` (decision
    /// 0055); `false` if `main` had none of it. A catalog that cannot says so.
    async fn remove_dataset(&self, _dataset: &str, _author: &str) -> Result<bool, String> {
        Err("this catalog deletes no dataset".into())
    }
    /// The dataset's files that changed from `from` to `to` (commits or
    /// branches), before and after.
    async fn changes(
        &self,
        _from: &str,
        _to: &str,
        _dataset: &str,
    ) -> Result<Vec<FileChange>, String> {
        Err("this catalog keeps no history".into())
    }
}
