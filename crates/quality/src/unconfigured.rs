//! The drafts' catalog of an installation that has none (no `DOLT_URL`):
//! the drafts' routes are still served, and every call fails with
//! [`NOT_CONFIGURED`], which they answer as `503 catalog_not_configured`
//! (decision 0026), so the web can say there is no catalog.

use async_trait::async_trait;

use crate::catalog::NOT_CONFIGURED;
use crate::draft_catalog::{
    Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved, FilesEdit, MappingEdit,
};
use crate::draft_history::{DraftHistory, FileChange, MainChange};

/// No catalog: every call fails with [`NOT_CONFIGURED`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Unconfigured;

fn none<T>() -> Result<T, String> {
    Err(NOT_CONFIGURED.to_string())
}

#[async_trait]
impl DraftCatalog for Unconfigured {
    async fn list(&self, _prefix: &str) -> Result<Vec<DraftInfo>, String> {
        none()
    }
    async fn read(&self, _branch: &str, _dataset: &str) -> Result<Option<DraftFiles>, String> {
        none()
    }
    async fn approve(
        &self,
        _branch: &str,
        _dataset: &str,
        _author: &str,
    ) -> Result<Approval, String> {
        none()
    }
    async fn discard(&self, _branch: &str) -> Result<bool, String> {
        none()
    }
    async fn save_mapping(
        &self,
        _edit: &MappingEdit<'_>,
        _create: bool,
    ) -> Result<FileSaved, String> {
        none()
    }
    async fn save_files(&self, _edit: &FilesEdit<'_>) -> Result<String, String> {
        none()
    }
    async fn save_file(&self, _edit: &FileEdit<'_>, _create: bool) -> Result<FileSaved, String> {
        none()
    }
    async fn history(&self, _branch: &str) -> Result<Option<DraftHistory>, String> {
        none()
    }
    async fn file_history(
        &self,
        _dataset: &str,
        _file: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<MainChange>, String> {
        none()
    }
    async fn remove_dataset(&self, _dataset: &str, _author: &str) -> Result<bool, String> {
        none()
    }
    async fn changes(
        &self,
        _from: &str,
        _to: &str,
        _dataset: &str,
    ) -> Result<Vec<FileChange>, String> {
        none()
    }
}
