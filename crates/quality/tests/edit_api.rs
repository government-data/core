//! A person's edit of a rules file, driven without a socket over a catalog
//! in memory, with the `Caller` server would put in each request (decisions
//! 0019 and 0021): the first save starts a draft, the next ones save on
//! it, each audited with the person as the commit's author; a file that
//! does not lint is not saved; only whoever may edit the dataset's rules;
//! a column's draft takes no other column's file (decision 0050).
//! Synthetic names only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Extension;
use contracts::{Access, AuditEntry, AuditTrail, Caller, Change, Module};
use quality::draft_api::{Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved};
use quality::draft_history::{DraftHistory, FileChange};
use quality::module::QualityModule;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const MAPPING: &str =
    "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  Id: { column: id, type: text }\n";
const GOOD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Id\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";
const BAD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Nope\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";

/// `main` has `loans` with `basic.yaml`; drafts saved, as (branch, file,
/// body, author).
#[derive(Default)]
struct Memory {
    saved: Mutex<Vec<(String, String, String, String)>>,
}

#[async_trait::async_trait]
impl DraftCatalog for Memory {
    async fn list(&self, _: Uuid, _: &str) -> Result<Vec<DraftInfo>, String> {
        Ok(Vec::new())
    }
    async fn read(
        &self,
        _: Uuid,
        branch: &str,
        dataset: &str,
    ) -> Result<Option<DraftFiles>, String> {
        // Linted against main's mapping; a draft's own global rules are
        // asked for, and these drafts have none.
        if branch != "main" {
            return Ok(None);
        }
        Ok((dataset == "loans").then(|| DraftFiles {
            commit: "c1".into(),
            mapping: MAPPING.into(),
            rules: vec![("basic.yaml".into(), GOOD.into())],
        }))
    }
    async fn approve(&self, _: Uuid, _: &str, _: &str, _: &str) -> Result<Approval, String> {
        unreachable!("not here")
    }
    async fn discard(&self, _: Uuid, _: &str) -> Result<bool, String> {
        unreachable!("not here")
    }
    async fn save_file(
        &self,
        _: Uuid,
        edit: &FileEdit<'_>,
        create: bool,
    ) -> Result<FileSaved, String> {
        if edit.file != "basic.yaml" && !edit.adds {
            return Ok(FileSaved::NoFile);
        }
        let mut saved = self.saved.lock().unwrap();
        if !create && !saved.iter().any(|(b, ..)| b == edit.branch) {
            return Ok(FileSaved::NoDraft);
        }
        saved.push((
            edit.branch.into(),
            edit.file.into(),
            edit.body.into(),
            edit.author.into(),
        ));
        Ok(FileSaved::Saved(format!("c{}", saved.len() + 1)))
    }
    async fn history(&self, _: Uuid, branch: &str) -> Result<Option<DraftHistory>, String> {
        let saved = self.saved.lock().unwrap();
        Ok(saved
            .iter()
            .any(|(b, ..)| b == branch)
            .then(|| DraftHistory {
                base: "c1".into(),
                commits: Vec::new(),
            }))
    }
    /// The files saved on the draft `to`, each once.
    async fn changes(
        &self,
        _: Uuid,
        _: &str,
        to: &str,
        _: &str,
    ) -> Result<Vec<FileChange>, String> {
        let saved = self.saved.lock().unwrap();
        let mut files: Vec<FileChange> = Vec::new();
        for (branch, file, body, _) in saved.iter() {
            if branch == to && !files.iter().any(|c| &c.file == file) {
                files.push(FileChange {
                    file: file.clone(),
                    before: None,
                    after: Some(body.clone()),
                });
            }
        }
        Ok(files)
    }
}

/// May edit the rules of `loans` and `gone` (not on `main`), not `cards`'.
struct Editor;
impl Access for Editor {
    fn can(&self, action: &str, resource: &str) -> bool {
        action == "quality:EditRules" && matches!(resource, "catalog/loans" | "catalog/gone")
    }
    fn can_some(&self, action: &str) -> bool {
        action == "quality:EditRules"
    }
}

#[derive(Default)]
struct Log(Mutex<Vec<AuditEntry>>);
#[async_trait::async_trait]
impl AuditTrail for Log {
    async fn record(&self, _: &Caller, entry: &AuditEntry) -> Result<(), String> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }
}

struct Fixture {
    memory: Arc<Memory>,
    log: Arc<Log>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            memory: Arc::default(),
            log: Arc::default(),
        }
    }

    async fn call(&self, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Editor),
            self.log.clone(),
        );
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = QualityModule::default()
            .with_drafts(self.memory.clone())
            .user_router()
            .layer(Extension(caller))
            .oneshot(req)
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn changes(&self) -> Vec<(String, Change)> {
        let log = self.log.0.lock().unwrap();
        log.iter().map(|e| (e.resource.clone(), e.change)).collect()
    }
}

#[tokio::test]
async fn the_first_save_starts_a_draft_and_the_next_ones_save_on_it() {
    let f = Fixture::new();
    let edited = GOOD.replace("major", "minor");
    let (status, out) = f
        .call(
            "POST",
            "/quality/datasets/loans/drafts",
            json!({ "file": "basic.yaml", "yaml": edited }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    assert!(Uuid::parse_str(&id).is_ok());
    assert_eq!(out["branch"], format!("proposal/loans/{id}"));
    {
        let saved = f.memory.saved.lock().unwrap();
        let (branch, file, body, author) = &saved[0];
        assert_eq!(
            (branch.as_str(), file.as_str()),
            (out["branch"].as_str().unwrap(), "basic.yaml")
        );
        assert_eq!(body, &edited);
        assert!(
            author.starts_with("00000000-0000-0000-0000-0000000000a1 <"),
            "{author}"
        );
    }

    let uri = format!("/quality/drafts/loans/{id}/files/basic.yaml");
    let (status, out) = f.call("PUT", &uri, json!({ "yaml": GOOD })).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["id"], id);
    let resource = format!("catalog/loans/draft/{id}");
    assert_eq!(
        f.changes(),
        [
            (resource.clone(), Change::Created),
            (resource, Change::Changed)
        ]
    );
}

#[tokio::test]
async fn a_file_that_does_not_lint_is_not_saved() {
    let f = Fixture::new();
    let (status, out) = f
        .call(
            "POST",
            "/quality/datasets/loans/drafts",
            json!({ "file": "basic.yaml", "yaml": BAD }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(out["error"], "lint");
    let finding = &out["findings"][0];
    assert_eq!(
        (finding["severity"].as_str(), finding["rule"].as_str()),
        (Some("error"), Some("L-01"))
    );
    assert!(f.memory.saved.lock().unwrap().is_empty());
    assert!(f.changes().is_empty(), "nothing saved, nothing audited");
}

#[tokio::test]
async fn only_an_editor_saves_a_file_the_dataset_has_on_a_draft_that_is_there() {
    let f = Fixture::new();
    let new = |dataset: &str, file: &str| {
        let uri = format!("/quality/datasets/{dataset}/drafts");
        let body = json!({ "file": file, "yaml": GOOD });
        let f = &f;
        async move { f.call("POST", &uri, body).await.0 }
    };
    assert_eq!(new("cards", "basic.yaml").await, StatusCode::FORBIDDEN);
    assert_eq!(new("loans", "../basic.yaml").await, StatusCode::FORBIDDEN);
    assert_eq!(new("loans", "basic.yml").await, StatusCode::FORBIDDEN);
    assert_eq!(new("loans", "other.yaml").await, StatusCode::NOT_FOUND);
    assert_eq!(new("gone", "basic.yaml").await, StatusCode::NOT_FOUND);
    let (status, _) = f
        .call(
            "PUT",
            "/quality/drafts/loans/abc-1/files/basic.yaml",
            json!({ "yaml": GOOD }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no such draft");
    assert!(f.changes().is_empty());
}

#[tokio::test]
async fn a_draft_adds_a_fields_own_file_and_no_other() {
    let f = Fixture::new();
    let new = |file: &str, yaml: &str| {
        let body = json!({ "file": file, "yaml": yaml });
        let f = &f;
        async move {
            f.call("POST", "/quality/datasets/loans/drafts", body)
                .await
                .0
        }
    };
    // A field of the mapping, and one derived in the file itself.
    assert_eq!(new("Id.yaml", GOOD).await, StatusCode::CREATED);
    let derived = "dataset: loans\nderived:\n  Code: upper(Id)\nrules:\n  - id: L-02\n    field: Code\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";
    assert_eq!(new("Code.yaml", derived).await, StatusCode::CREATED);
    // The table's global rules (decision 0045).
    let globals = "dataset: loans\nlists:\n  codes: [A, B]\n";
    assert_eq!(
        new("reglas_globales.yaml", globals).await,
        StatusCode::CREATED
    );
    // A name that is no field: refused, as a file the dataset lacks.
    assert_eq!(new("Other.yaml", GOOD).await, StatusCode::NOT_FOUND);
    let saved = f.memory.saved.lock().unwrap();
    let files: Vec<_> = saved.iter().map(|(_, file, ..)| file.as_str()).collect();
    assert_eq!(files, ["Id.yaml", "Code.yaml", "reglas_globales.yaml"]);
}

#[tokio::test]
async fn a_columns_draft_takes_no_other_columns_file() {
    let f = Fixture::new();
    let (status, out) = f
        .call(
            "POST",
            "/quality/datasets/loans/drafts",
            json!({ "file": "basic.yaml", "yaml": GOOD.replace("major", "minor") }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    let id = out["id"].as_str().unwrap().to_string();

    // Its own file again: saved. A field's file of its own: refused.
    let own = format!("/quality/drafts/loans/{id}/files/basic.yaml");
    assert_eq!(
        f.call("PUT", &own, json!({ "yaml": GOOD })).await.0,
        StatusCode::OK
    );
    let other = format!("/quality/drafts/loans/{id}/files/Id.yaml");
    let (status, out) = f.call("PUT", &other, json!({ "yaml": GOOD })).await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("other_file"))
    );
    assert_eq!(f.memory.saved.lock().unwrap().len(), 2);

    // That column is a draft of its own.
    let (status, out) = f
        .call(
            "POST",
            "/quality/datasets/loans/drafts",
            json!({ "file": "Id.yaml", "yaml": GOOD }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
}
