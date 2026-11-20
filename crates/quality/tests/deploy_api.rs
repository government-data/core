//! A column's rules file deployed to `main` (decision 0051), driven without
//! a socket over a catalog in memory: deployed by whoever may approve,
//! audited, on `main`; refused if `main`'s file changed since it was
//! opened, if it does not lint, or for whoever may only edit. Synthetic
//! names only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Extension;
use contracts::{Access, AuditEntry, AuditTrail, Caller, Module};
use quality::draft_api::{Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved};
use quality::module::QualityModule;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const MAPPING: &str =
    "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  Id: { column: id, type: text }\n";
const GOOD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Id\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";
const BAD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Nope\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";

/// `main` has `loans` with `Id.yaml`; what is saved, as (branch, file, body).
#[derive(Default)]
struct Memory {
    saved: Mutex<Vec<(String, String, String)>>,
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
        if branch != "main" {
            return Ok(None);
        }
        Ok((dataset == "loans").then(|| DraftFiles {
            commit: "c1".into(),
            mapping: MAPPING.into(),
            rules: vec![("Id.yaml".into(), GOOD.into())],
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
        assert!(!create, "main is never made");
        let mut saved = self.saved.lock().unwrap();
        saved.push((edit.branch.into(), edit.file.into(), edit.body.into()));
        Ok(FileSaved::Saved("c2".into()))
    }
}

/// May approve `loans`' rules, or, as `Editor`, only edit them.
struct Approver(bool);
impl Access for Approver {
    fn can(&self, action: &str, resource: &str) -> bool {
        match action {
            "quality:EditRules" | "quality:ApproveRules" => {
                resource == "catalog/loans" && (self.0 || action == "quality:EditRules")
            }
            "quality:ManageDatasets" => self.0 && resource == "dataset/loans",
            _ => false,
        }
    }
    fn can_some(&self, _: &str) -> bool {
        true
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

async fn deploy(
    memory: &Arc<Memory>,
    log: &Arc<Log>,
    approves: bool,
    body: Value,
) -> (StatusCode, Value) {
    let caller = Caller::new(
        "00000000-0000-0000-0000-0000000000a1".into(),
        None,
        "192.0.2.1".into(),
        None,
        Arc::new(Approver(approves)),
        log.clone(),
    );
    let req = Request::put("/quality/datasets/loans/files/Id.yaml")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = QualityModule::default()
        .with_drafts(memory.clone())
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

#[tokio::test]
async fn whoever_may_approve_deploys_a_columns_file_to_main_audited() {
    let (memory, log) = (Arc::default(), Arc::default());
    let edited = GOOD.replace("major", "minor");
    let (status, out) = deploy(
        &memory,
        &log,
        true,
        json!({ "yaml": edited, "expected": GOOD }),
    )
    .await;
    assert_eq!(
        (status, out["commit"].as_str()),
        (StatusCode::OK, Some("c2")),
        "{out}"
    );
    assert_eq!(
        memory.saved.lock().unwrap().as_slice(),
        [("main".to_string(), "Id.yaml".to_string(), edited)]
    );
    let entries = log.0.lock().unwrap();
    assert_eq!(entries[0].resource, "catalog/loans/file/Id.yaml");
}

#[tokio::test]
async fn a_deploy_over_a_changed_file_a_bad_one_or_by_an_editor_is_refused() {
    let (memory, log): (Arc<Memory>, Arc<Log>) = (Arc::default(), Arc::default());
    // main's file is not what was opened: someone deployed meanwhile.
    let (status, out) = deploy(
        &memory,
        &log,
        true,
        json!({ "yaml": GOOD, "expected": null }),
    )
    .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("stale"))
    );
    let (status, out) = deploy(
        &memory,
        &log,
        true,
        json!({ "yaml": BAD, "expected": GOOD }),
    )
    .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("lint"))
    );
    let (status, _) = deploy(
        &memory,
        &log,
        false,
        json!({ "yaml": GOOD, "expected": GOOD }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(memory.saved.lock().unwrap().is_empty(), "nothing deployed");
    assert!(log.0.lock().unwrap().is_empty(), "nothing audited");
}
