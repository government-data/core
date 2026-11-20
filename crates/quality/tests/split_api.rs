//! _Split by field_ through the API (decisions 0044 and 0045): a dataset's
//! files on `main` become a new draft with its global rules and a file per
//! field, in one save, audited; already split, nothing; refused with why.
//! Synthetic names only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Extension;
use contracts::{Access, AuditEntry, AuditTrail, Caller, Module};
use quality::draft_api::{
    Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved, FilesEdit,
};
use quality::module::QualityModule;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const MAPPING: &str = "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  Id: { column: Id, type: text }\n  Kind: { column: Kind, type: text }\n";
/// Fields named otherwise than their columns: renamed as they are split.
const RENAMED: &str = "dataset: codes\nsource: codes\nkey: [Code]\nfields:\n  Code: { column: CODE, type: text }\n  Other: { column: Other, type: text }\n";
const CODES: &str = "dataset: codes\nrules:\n  # Code is there when Other is.\n  - id: C-01\n    field: Code\n    check: required\n    when: Other IS NOT NULL\n    severity: major\n    cause: capture\n    alert: No Code\n";
const GROUP: &str = "dataset: loans\nlists:\n  kinds: [A, B]\nrules:\n  - id: L-01\n    field: Id\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n\n  - id: L-02\n    field: Kind\n    check: domain\n    in: { list: kinds }\n    severity: minor\n    cause: capture\n    alert: Unknown\n";
const SPLIT: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Id\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";

/// A draft saved: its branch, each file's new text or `None`, and its
/// mapping's if it changed.
type Saved = (String, Vec<(String, Option<String>)>, Option<String>);

/// `loans` by field group, `done` already split, `twice` with a list
/// declared two ways; the drafts saved.
#[derive(Default)]
struct Memory {
    saved: Mutex<Vec<Saved>>,
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
        assert_eq!(branch, "main", "split from main");
        let other = GROUP
            .replace("[A, B]", "[A]")
            .replace("L-01", "L-03")
            .replace("L-02", "L-04");
        let rules = match dataset {
            "loans" => vec![("group.yaml".into(), GROUP.into())],
            "done" => vec![("Id.yaml".into(), SPLIT.into())],
            "twice" => vec![("a.yaml".into(), GROUP.into()), ("b.yaml".into(), other)],
            "codes" | "locked" => {
                return Ok(Some(DraftFiles {
                    commit: "c1".into(),
                    mapping: RENAMED.into(),
                    rules: vec![("codes.yaml".into(), CODES.into())],
                }))
            }
            _ => return Ok(None),
        };
        Ok(Some(DraftFiles {
            commit: "c1".into(),
            mapping: MAPPING.into(),
            rules,
        }))
    }
    async fn approve(&self, _: Uuid, _: &str, _: &str, _: &str) -> Result<Approval, String> {
        unreachable!("not here")
    }
    async fn discard(&self, _: Uuid, _: &str) -> Result<bool, String> {
        unreachable!("not here")
    }
    async fn save_file(&self, _: Uuid, _: &FileEdit<'_>, _: bool) -> Result<FileSaved, String> {
        unreachable!("one save for every file")
    }
    async fn save_files(&self, _: Uuid, edit: &FilesEdit<'_>) -> Result<String, String> {
        self.saved.lock().unwrap().push((
            edit.branch.into(),
            edit.files.to_vec(),
            edit.mapping.map(str::to_string),
        ));
        Ok("c2".into())
    }
}

struct Editor;
impl Access for Editor {
    fn can(&self, action: &str, resource: &str) -> bool {
        (action == "quality:EditRules" && resource != "catalog/cards")
            || (action == "quality:ManageDatasets" && resource != "dataset/locked")
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

async fn split(memory: &Arc<Memory>, log: &Arc<Log>, dataset: &str) -> (StatusCode, Value) {
    let caller = Caller::new(
        "00000000-0000-0000-0000-0000000000a1".into(),
        None,
        "192.0.2.1".into(),
        None,
        Arc::new(Editor),
        log.clone(),
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/quality/datasets/{dataset}/split"))
        .body(Body::empty())
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
async fn a_datasets_files_are_split_on_a_new_draft_in_one_save() {
    let (memory, log) = (Arc::<Memory>::default(), Arc::<Log>::default());
    let (status, out) = split(&memory, &log, "loans").await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    assert_eq!(
        out["branch"],
        format!("proposal/loans/{}", out["id"].as_str().unwrap())
    );
    let saved = memory.saved.lock().unwrap();
    let files: Vec<(&str, bool)> = saved[0]
        .1
        .iter()
        .map(|(f, t)| (f.as_str(), t.is_some()))
        .collect();
    assert_eq!(
        files,
        [
            ("reglas_globales.yaml", true),
            ("Id.yaml", true),
            ("Kind.yaml", true),
            ("group.yaml", false)
        ]
    );
    assert_eq!(log.0.lock().unwrap().len(), 1, "audited");
}

#[tokio::test]
async fn split_files_change_nothing_and_what_cannot_be_split_says_why() {
    let (memory, log) = (Arc::<Memory>::default(), Arc::<Log>::default());
    assert_eq!(split(&memory, &log, "done").await.0, StatusCode::NO_CONTENT);
    let (status, out) = split(&memory, &log, "twice").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        (
            out["reason"].as_str(),
            out["kind"].as_str(),
            out["name"].as_str()
        ),
        (Some("conflict"), Some("lists"), Some("kinds"))
    );
    assert_eq!(split(&memory, &log, "cards").await.0, StatusCode::FORBIDDEN);
    assert!(memory.saved.lock().unwrap().is_empty());
    assert!(log.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn fields_take_their_columns_names_with_the_rules_that_name_them() {
    let (memory, log) = (Arc::<Memory>::default(), Arc::<Log>::default());
    let (status, out) = split(&memory, &log, "codes").await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    let (_, files, mapping) = memory.saved.lock().unwrap()[0].clone();
    assert_eq!(
        mapping.as_deref(),
        Some("dataset: codes\nsource: codes\nkey: [CODE]\nfields:\n  CODE: { column: CODE, type: text }\n  Other: { column: Other, type: text }\n")
    );
    assert_eq!(
        files,
        [
            (
                "CODE.yaml".to_string(),
                Some("dataset: codes\nrules:\n  # Code is there when Other is.\n  - id: C-01\n    field: CODE\n    check: required\n    when: Other IS NOT NULL\n    severity: major\n    cause: capture\n    alert: No Code\n".to_string())
            ),
            ("codes.yaml".to_string(), None)
        ]
    );
    // Renaming changes the mapping: who may not manage the dataset may not.
    let (status, out) = split(&memory, &log, "locked").await;
    assert_eq!(
        (status, out),
        (
            StatusCode::FORBIDDEN,
            serde_json::json!({ "error": "manage" })
        )
    );
}
