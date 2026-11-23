//! The tenant's catalog for the quality module (decisions 0021 and 0038):
//! its `main` for jobs, its drafts for map jobs and for people, over Dolt
//! (`infrastructure::catalog_dolt`); and the texts of the rules a map
//! proposes.

use infrastructure::catalog_dolt::{DatasetImport, DoltCatalog, Merged};
use quality::catalog::{Catalog, CatalogSnapshot, DatasetMapping, RulesSource};
use quality::draft_api::{Approval, DraftCatalog, DraftFiles, DraftInfo};
use quality::draft_history::{DraftCommit, DraftHistory, FileChange, MainChange};
use quality::job_map::Drafts;
use quality::proposal::Alerts;
use texts::{text_in, Language};
use uuid::Uuid;

/// A dataset's catalog on `main`, from the tenant's Dolt database. One
/// for the whole API: its gate holds every module's operations while Dolt
/// restarts (0066, point 11).
pub(crate) struct DoltSnapshots(pub(crate) DoltCatalog);

impl DoltSnapshots {
    /// From `DOLT_URL`; `None` without it.
    pub(crate) fn from_env() -> Result<Option<std::sync::Arc<Self>>, String> {
        Ok(DoltCatalog::from_env()?.map(|c| std::sync::Arc::new(Self(c))))
    }
}

/// Dolt's hold for its restart when free (0066, point 11).
#[async_trait::async_trait]
impl application::DoltStore for DoltSnapshots {
    async fn hold(&self, ttl: std::time::Duration) -> application::DoltHold {
        let hold = self.0.hold(ttl).await;
        application::DoltHold {
            free: hold.free,
            in_flight: hold.in_flight,
            running: hold.running,
        }
    }

    fn release(&self) -> bool {
        self.0.release()
    }
}

#[async_trait::async_trait]
impl Catalog for DoltSnapshots {
    async fn snapshot(&self, dataset: &str) -> Result<Option<CatalogSnapshot>, String> {
        let files = self
            .0
            .files(quality::catalog::DATABASE, dataset)
            .await
            .map_err(|e| e.to_string())?;
        Ok(files.map(|f| CatalogSnapshot {
            commit: f.commit,
            mapping: f.mapping,
            rules: f
                .rules
                .into_iter()
                .map(|(file, yaml)| RulesSource { file, yaml })
                .collect(),
            others: f
                .others
                .into_iter()
                .map(|(dataset, yaml)| DatasetMapping { dataset, yaml })
                .collect(),
        }))
    }

    async fn mappings(&self) -> Result<Vec<DatasetMapping>, String> {
        let mappings = self
            .0
            .mappings(quality::catalog::DATABASE)
            .await
            .map_err(|e| e.to_string())?;
        Ok(mappings
            .into_iter()
            .map(|(dataset, yaml)| DatasetMapping { dataset, yaml })
            .collect())
    }
}

/// The texts of the rules a map proposes (decision 0038).
pub(crate) fn proposal_alerts(language: Language) -> Alerts {
    let t = |key: &str| text_in(language, &format!("quality.proposal.alerts.{key}"), &[]);
    Alerts {
        required: t("required"),
        unique: t("unique"),
        email: t("email"),
        mobile: t("mobile"),
        document: t("document"),
    }
}

/// A map job's proposal, published on `main` of the tenant's catalog
/// (decision 0053), unless the dataset is there.
#[async_trait::async_trait]
impl Drafts for DoltSnapshots {
    async fn publish(
        &self,
        tenant: Uuid,
        dataset: &str,
        mapping: &str,
        rules: &[(String, String)],
    ) -> Result<Option<String>, String> {
        let database = quality::catalog::database(tenant);
        let there = self
            .0
            .files(&database, dataset)
            .await
            .map_err(|e| e.to_string())?;
        if there.is_some() {
            return Ok(None);
        }
        let import = DatasetImport {
            dataset,
            description: None,
            mapping,
            rules,
        };
        self.0
            .import(
                &database,
                &import,
                "Quality Data map <map@quality-data.invalid>",
            )
            .await
            .map(Some)
            .map_err(|e| e.to_string())
    }
}

/// A commit of the catalog as the drafts' routes show it.
fn commit(c: infrastructure::catalog_dolt::DraftCommit) -> DraftCommit {
    DraftCommit {
        commit: c.commit,
        parent: c.parent,
        author: c.author,
        at: c.at,
        message: c.message,
    }
}

/// The tenant's drafts, for people to review, approve or discard.
#[async_trait::async_trait]
impl DraftCatalog for DoltSnapshots {
    async fn history(&self, tenant: Uuid, branch: &str) -> Result<Option<DraftHistory>, String> {
        let history = self
            .0
            .history(&quality::catalog::database(tenant), branch)
            .await
            .map_err(|e| e.to_string())?;
        Ok(history.map(|h| DraftHistory {
            base: h.base,
            commits: h.commits.into_iter().map(commit).collect(),
        }))
    }

    async fn remove_dataset(
        &self,
        tenant: Uuid,
        dataset: &str,
        author: &str,
    ) -> Result<bool, String> {
        self.0
            .remove(&quality::catalog::database(tenant), dataset, author)
            .await
            .map_err(|e| e.to_string())
    }

    async fn file_history(
        &self,
        tenant: Uuid,
        dataset: &str,
        file: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MainChange>, String> {
        let changes = self
            .0
            .file_history(&quality::catalog::database(tenant), dataset, file, limit)
            .await
            .map_err(|e| e.to_string())?;
        Ok(changes
            .into_iter()
            .map(|c| MainChange {
                commit: commit(c.commit),
                before: c.before,
                after: c.after,
            })
            .collect())
    }

    async fn changes(
        &self,
        tenant: Uuid,
        from: &str,
        to: &str,
        dataset: &str,
    ) -> Result<Vec<FileChange>, String> {
        let changes = self
            .0
            .changes(&quality::catalog::database(tenant), from, to, dataset)
            .await
            .map_err(|e| e.to_string())?;
        Ok(changes
            .into_iter()
            .map(|c| FileChange {
                file: c.file,
                before: c.before,
                after: c.after,
            })
            .collect())
    }

    async fn list(&self, tenant: Uuid, prefix: &str) -> Result<Vec<DraftInfo>, String> {
        let drafts = self
            .0
            .drafts(&quality::catalog::database(tenant), prefix)
            .await
            .map_err(|e| e.to_string())?;
        Ok(drafts
            .into_iter()
            .map(|d| DraftInfo {
                branch: d.branch,
                commit: d.commit,
                author: d.author,
                at: d.at,
            })
            .collect())
    }

    async fn read(
        &self,
        tenant: Uuid,
        branch: &str,
        dataset: &str,
    ) -> Result<Option<DraftFiles>, String> {
        let files = self
            .0
            .files_at(&quality::catalog::database(tenant), branch, dataset)
            .await
            .map_err(|e| e.to_string())?;
        Ok(files.map(|f| DraftFiles {
            commit: f.commit,
            mapping: f.mapping,
            rules: f.rules,
        }))
    }

    async fn approve(
        &self,
        tenant: Uuid,
        branch: &str,
        dataset: &str,
        author: &str,
    ) -> Result<Approval, String> {
        let merged = self
            .0
            .approve(
                &quality::catalog::database(tenant),
                branch,
                dataset,
                author,
                &format!("catalog: approve {dataset}"),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(match merged {
            Merged::Merged(commit) => Approval::Merged(commit),
            Merged::Conflicts(tables) => Approval::Conflicts(tables),
            Merged::Stale => Approval::Stale,
            Merged::NotFound => Approval::NotFound,
        })
    }

    async fn discard(&self, tenant: Uuid, branch: &str) -> Result<bool, String> {
        self.0
            .discard(&quality::catalog::database(tenant), branch)
            .await
            .map_err(|e| e.to_string())
    }

    async fn save_files(
        &self,
        tenant: Uuid,
        edit: &quality::draft_api::FilesEdit<'_>,
    ) -> Result<String, String> {
        let at = infrastructure::catalog_dolt::FilesEdit {
            branch: edit.branch,
            dataset: edit.dataset,
            files: edit.files,
            mapping: edit.mapping,
            author: edit.author,
            message: edit.message,
        };
        self.0
            .save_files(&quality::catalog::database(tenant), &at)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{} is already there", edit.branch))
    }

    async fn save_mapping(
        &self,
        tenant: Uuid,
        edit: &quality::draft_api::MappingEdit<'_>,
        create: bool,
    ) -> Result<quality::draft_api::FileSaved, String> {
        use infrastructure::catalog_dolt::{FileSaved, MappingEdit};
        let at = MappingEdit {
            branch: edit.branch,
            dataset: edit.dataset,
            body: edit.body,
            author: edit.author,
            starts: edit.starts,
        };
        let saved = self
            .0
            .save_mapping(&quality::catalog::database(tenant), &at, create)
            .await
            .map_err(|e| e.to_string())?;
        Ok(match saved {
            FileSaved::Saved(commit) => quality::draft_api::FileSaved::Saved(commit),
            FileSaved::NoDraft => quality::draft_api::FileSaved::NoDraft,
            FileSaved::NoFile => quality::draft_api::FileSaved::NoFile,
        })
    }

    async fn save_file(
        &self,
        tenant: Uuid,
        edit: &quality::draft_api::FileEdit<'_>,
        create: bool,
    ) -> Result<quality::draft_api::FileSaved, String> {
        use infrastructure::catalog_dolt::{FileEdit, FileSaved};
        let at = FileEdit {
            branch: edit.branch,
            dataset: edit.dataset,
            file: edit.file,
            body: edit.body,
            author: edit.author,
            adds: edit.adds,
        };
        let saved = self
            .0
            .save_file(&quality::catalog::database(tenant), &at, create)
            .await
            .map_err(|e| e.to_string())?;
        Ok(match saved {
            FileSaved::Saved(commit) => quality::draft_api::FileSaved::Saved(commit),
            FileSaved::NoDraft => quality::draft_api::FileSaved::NoDraft,
            FileSaved::NoFile => quality::draft_api::FileSaved::NoFile,
        })
    }
}
