//! A draft read as the branch it is (decision 0021), driven without a
//! socket over a catalog in memory, with the `Caller` server would put in
//! each request: its commits from where it left `main`, what the whole
//! draft changed and what one commit did; only for whoever may view the
//! dataset's rules, and only a commit of that draft; and a column's history
//! on `main` (decision 0050). Synthetic names only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Extension;
use contracts::{Access, AuditEntry, AuditTrail, Caller, Module};
use quality::draft_api::{Approval, DraftCatalog, DraftFiles, DraftInfo, FileEdit, FileSaved};
use quality::draft_history::{DraftCommit, DraftHistory, FileChange, MainChange};
use quality::module::QualityModule;
use serde_json::{json, Value};
use tower::ServiceExt;

const BASE: &str = "b0000000000000000000000000000000";
const ONE: &str = "c1000000000000000000000000000000";
const TWO: &str = "c2000000000000000000000000000000";

/// `loans` has draft `h1`, two commits on it; the diffs asked are kept.
#[derive(Default)]
struct Memory {
    asked: Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl DraftCatalog for Memory {
    async fn list(&self, _: &str) -> Result<Vec<DraftInfo>, String> {
        Ok(Vec::new())
    }
    async fn read(&self, _: &str, _: &str) -> Result<Option<DraftFiles>, String> {
        unreachable!("not read here")
    }
    async fn approve(&self, _: &str, _: &str, _: &str) -> Result<Approval, String> {
        unreachable!("nothing here writes")
    }
    async fn discard(&self, _: &str) -> Result<bool, String> {
        unreachable!("nothing here writes")
    }
    async fn save_file(&self, _: &FileEdit<'_>, _: bool) -> Result<FileSaved, String> {
        unreachable!("nothing here writes")
    }
    async fn history(&self, branch: &str) -> Result<Option<DraftHistory>, String> {
        let commit = |commit: &str, parent: &str, author: &str| DraftCommit {
            commit: commit.into(),
            parent: parent.into(),
            author: author.into(),
            at: "2026-11-01 10:00:00".into(),
            message: "edit".into(),
        };
        Ok((branch == "proposal/loans/h1").then(|| DraftHistory {
            base: BASE.into(),
            commits: vec![
                commit(TWO, ONE, "Ben <ben@example.org>"),
                commit(ONE, BASE, "Ana <ana@example.org>"),
            ],
        }))
    }
    async fn changes(
        &self,
        from: &str,
        to: &str,
        dataset: &str,
    ) -> Result<Vec<FileChange>, String> {
        assert_eq!(dataset, "loans");
        self.asked.lock().unwrap().push((from.into(), to.into()));
        Ok(vec![FileChange {
            file: "basic.yaml".into(),
            before: Some("dataset: loans\nrules: []\n".into()),
            after: None,
        }])
    }
    async fn file_history(
        &self,
        dataset: &str,
        file: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MainChange>, String> {
        let what = format!("{dataset}/{}/{limit}", file.unwrap_or("mapping"));
        self.asked.lock().unwrap().push((what, String::new()));
        Ok(vec![MainChange {
            commit: DraftCommit {
                commit: ONE.into(),
                parent: BASE.into(),
                author: "Ana <ana@example.org>".into(),
                at: "2026-11-04 10:00:00".into(),
                message: "approve".into(),
            },
            before: None,
            after: Some("dataset: loans\nrules: []\n".into()),
        }])
    }
}

/// May view the rules of `loans`, not `cards`'.
struct Analyst;
impl Access for Analyst {
    fn can(&self, action: &str, resource: &str) -> bool {
        action == "quality:ViewRules" && resource == "catalog/loans"
    }
    fn can_some(&self, action: &str) -> bool {
        action == "quality:ViewRules"
    }
}

struct NoLog;
#[async_trait::async_trait]
impl AuditTrail for NoLog {
    async fn record(&self, _: &Caller, _: &AuditEntry) -> Result<(), String> {
        unreachable!("reading is not audited")
    }
}

async fn get(memory: &Arc<Memory>, uri: &str) -> (StatusCode, Value) {
    let caller = Caller::new(
        "00000000-0000-0000-0000-0000000000a1".into(),
        None,
        "192.0.2.1".into(),
        None,
        Arc::new(Analyst),
        Arc::new(NoLog),
    );
    let res = QualityModule::default()
        .with_drafts(memory.clone())
        .user_router()
        .layer(Extension(caller))
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
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
async fn a_drafts_commits_are_read_newest_first_from_where_it_left_main() {
    let memory = Arc::default();
    let (status, out) = get(&memory, "/quality/drafts/loans/h1/history").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["base"], BASE);
    assert_eq!(out["commits"][0]["author"], "Ben <ben@example.org>");
    assert_eq!(out["commits"][1]["parent"], BASE);
}

#[tokio::test]
async fn the_whole_draft_or_one_of_its_commits_is_compared() {
    let memory: Arc<Memory> = Arc::default();
    let (status, out) = get(&memory, "/quality/drafts/loans/h1/changes").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (out["from"].as_str(), out["to"].as_str()),
        (Some(BASE), Some("proposal/loans/h1"))
    );
    assert_eq!(
        out["files"],
        json!([{ "file": "basic.yaml", "before": "dataset: loans\nrules: []\n", "after": null }])
    );
    let uri = format!("/quality/drafts/loans/h1/changes?commit={TWO}");
    let (status, out) = get(&memory, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (out["from"].as_str(), out["to"].as_str()),
        (Some(ONE), Some(TWO))
    );
    assert_eq!(memory.asked.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn only_a_commit_of_that_draft_and_only_for_whoever_may_view() {
    let memory: Arc<Memory> = Arc::default();
    let uri = format!("/quality/drafts/loans/h1/changes?commit={BASE}");
    assert_eq!(
        get(&memory, &uri).await.0,
        StatusCode::NOT_FOUND,
        "main's, not the draft's"
    );
    assert_eq!(
        get(&memory, "/quality/drafts/loans/zz/history").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&memory, "/quality/drafts/cards/h1/history").await.0,
        StatusCode::FORBIDDEN
    );
    assert!(memory.asked.lock().unwrap().is_empty(), "nothing compared");
}

#[tokio::test]
async fn a_columns_history_is_its_files_changes_on_main() {
    let memory: Arc<Memory> = Arc::default();
    let (status, out) = get(&memory, "/quality/datasets/loans/history?file=Id.yaml").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out[0]["commit"]["author"], "Ana <ana@example.org>");
    assert_eq!(out[0]["before"], Value::Null);
    let (status, _) = get(&memory, "/quality/datasets/loans/history").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        memory
            .asked
            .lock()
            .unwrap()
            .iter()
            .map(|(w, _)| w.as_str())
            .collect::<Vec<_>>(),
        ["loans/Id.yaml/50", "loans/mapping/50"]
    );
    assert_eq!(
        get(&memory, "/quality/datasets/loans/history?file=..%2Fx")
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        get(&memory, "/quality/datasets/cards/history?file=Id.yaml")
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(memory.asked.lock().unwrap().len(), 2, "nothing more asked");
}
