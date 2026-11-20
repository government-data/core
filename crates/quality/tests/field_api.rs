//! _Map the whole table_ through the API (decisions 0043 and 0047): the
//! columns no field reads become fields of the dataset's mapping on
//! `main`, at once, audited; a table no dataset reads starts one; what
//! cannot be added says why. Synthetic names only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Extension;
use contracts::{Access, AuditEntry, AuditTrail, Caller, Change, Module};
use quality::draft_api::{
    Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved, MappingEdit,
};
use quality::module::QualityModule;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const MAPPING: &str =
    "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  Id: { column: id, type: text }\n";

/// `loans` on `main` and on the drafts saved, by branch.
#[derive(Default)]
struct Memory {
    drafts: Mutex<Vec<(String, String)>>,
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
        let drafts = self.drafts.lock().unwrap();
        let mapping = match drafts.iter().rev().find(|(b, _)| b == branch) {
            Some((_, body)) => body.clone(),
            None if branch == "main" && dataset == "loans" => MAPPING.to_string(),
            None => return Ok(None),
        };
        let known = dataset == "loans" || branch.starts_with(&format!("proposal/{dataset}/"));
        Ok((known && !mapping.is_empty()).then(|| DraftFiles {
            commit: "c1".into(),
            mapping,
            rules: Vec::new(),
        }))
    }
    async fn approve(&self, _: Uuid, _: &str, _: &str, _: &str) -> Result<Approval, String> {
        unreachable!("not here")
    }
    async fn discard(&self, _: Uuid, _: &str) -> Result<bool, String> {
        unreachable!("not here")
    }
    async fn save_file(&self, _: Uuid, _: &FileEdit<'_>, _: bool) -> Result<FileSaved, String> {
        unreachable!("the mapping, not a rules file")
    }
    async fn save_mapping(
        &self,
        _: Uuid,
        edit: &MappingEdit<'_>,
        create: bool,
    ) -> Result<FileSaved, String> {
        let mut drafts = self.drafts.lock().unwrap();
        assert_eq!((edit.branch, create), ("main", false), "on main, no draft");
        assert_eq!(
            edit.starts,
            edit.dataset != "loans",
            "only a new dataset starts"
        );
        drafts.push((edit.branch.into(), edit.body.into()));
        Ok(FileSaved::Saved(format!("c{}", drafts.len() + 1)))
    }
}

struct Editor;
impl Access for Editor {
    fn can(&self, action: &str, resource: &str) -> bool {
        action == "quality:ManageDatasets" && resource != "dataset/cards"
    }
    fn can_some(&self, action: &str) -> bool {
        action == "quality:ManageDatasets"
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

async fn add(
    log: &Arc<Log>,
    memory: &Arc<Memory>,
    dataset: &str,
    body: Value,
) -> (StatusCode, Value) {
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
        .uri(format!("/quality/datasets/{dataset}/fields"))
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

fn columns(names: &[(&str, &str)]) -> Value {
    names
        .iter()
        .map(|(column, kind)| json!({ "column": column, "type": kind }))
        .collect()
}

#[tokio::test]
async fn a_tables_columns_not_read_become_fields_on_main_at_once() {
    let (memory, log) = (Arc::<Memory>::default(), Arc::<Log>::default());
    let all = columns(&[
        ("id", "VARCHAR"),
        ("loan_amount", "DECIMAL(12,2)"),
        ("opened", "DATE"),
    ]);
    let (status, out) = add(&log, &memory, "loans", json!({ "columns": all })).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    assert_eq!(
        out["fields"],
        json!(["loan_amount", "opened"]),
        "id is read already"
    );
    assert_eq!(out["commit"], "c2");
    let drafts = memory.drafts.lock().unwrap();
    assert_eq!(drafts.len(), 1, "one save for every column");
    assert!(drafts[0].1.ends_with(
        "  loan_amount: { column: \"loan_amount\", type: number }\n  opened: { column: \"opened\", type: date }\n"
    ));
    let entries = log.0.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].change, Change::Changed);
    assert_eq!(entries[0].resource, "dataset/loans");
}

#[tokio::test]
async fn a_table_no_dataset_reads_starts_one_with_all_its_columns() {
    let (memory, log) = (Arc::<Memory>::default(), Arc::<Log>::default());
    let all = columns(&[("advisor_id", "INTEGER"), ("Zone", "VARCHAR")]);
    let table = json!({ "source": "core", "table": "dbo.Advisors" });
    let body = json!({ "columns": all, "table": table });
    let (status, out) = add(&log, &memory, "advisors", body).await;
    assert_eq!(status, StatusCode::CREATED, "{out}");
    assert_eq!(out["fields"], json!(["advisor_id", "Zone"]));
    let entries = log.0.lock().unwrap();
    assert_eq!(entries[0].change, Change::Created);
    assert_eq!(entries[0].resource, "dataset/advisors");
    let (_, body) = memory.drafts.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        body,
        "dataset: advisors\nsource: \"advisors\"\nfrom: { source: core, table: \"dbo.Advisors\" }\nfields:\n  advisor_id: { column: \"advisor_id\", type: number }\n  Zone: { column: \"Zone\", type: text }\n"
    );
}

#[tokio::test]
async fn what_cannot_be_added_says_why() {
    let (memory, log) = (Arc::<Memory>::default(), Arc::<Log>::default());
    let lake = json!({ "table": "loans" });
    let cases = [
        (
            "loans",
            json!({ "columns": columns(&[("id", "VARCHAR")]) }),
            StatusCode::CONFLICT,
            json!({ "error": "mapped" }),
        ),
        (
            "loans",
            json!({ "columns": columns(&[("x", "TEXT"), ("-id", "VARCHAR")]) }),
            StatusCode::CONFLICT,
            json!({ "error": "field", "reason": "taken", "column": "-id" }),
        ),
        (
            "loans",
            json!({ "columns": [] }),
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": "columns" }),
        ),
        (
            "loans",
            json!({ "columns": columns(&[("x", "TEXT")]), "table": lake }),
            StatusCode::CONFLICT,
            json!({ "error": "dataset_taken" }),
        ),
        (
            "other",
            json!({ "columns": columns(&[("x", "TEXT")]) }),
            StatusCode::NOT_FOUND,
            json!({ "error": "not_found" }),
        ),
        (
            "cards",
            json!({ "columns": columns(&[("x", "TEXT")]) }),
            StatusCode::FORBIDDEN,
            json!({ "error": "forbidden" }),
        ),
    ];
    for (dataset, body, code, error) in cases {
        let (status, out) = add(&log, &memory, dataset, body).await;
        assert_eq!((status, out), (code, error));
    }
    assert!(memory.drafts.lock().unwrap().is_empty());
    assert!(log.0.lock().unwrap().is_empty());
}
