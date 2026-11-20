//! The drafts' routes for people, driven without a socket over a catalog in
//! memory, with the `Caller` server would put in each request (decisions
//! 0021 and 0038): a draft is read with its lint errors, approved only by
//! whoever may approve rules and manage the dataset and only if it reads
//! and lints, refused when stale, discarded; each change audited.
//! Synthetic names only.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Extension, Router};
use contracts::{Access, AuditEntry, AuditTrail, Caller, Change, Module};
use quality::draft_api::{Approval, DraftCatalog, DraftFiles, DraftInfo};
use quality::module::QualityModule;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

const MAPPING: &str =
    "dataset: loans\nsource: loans\nkey: [Id]\nfields:\n  Id: { column: id, type: text }\n";
const GOOD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Id\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";
const BAD: &str = "dataset: loans\nrules:\n  - id: L-01\n    field: Nope\n    check: required\n    severity: major\n    cause: capture\n    alert: Empty\n";

/// Drafts by branch, with their rules file; `stale` ones refuse to merge.
#[derive(Default)]
struct Memory {
    drafts: Mutex<Vec<(String, String)>>,
    stale: Mutex<Vec<String>>,
    merged: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl DraftCatalog for Memory {
    async fn list(&self, _: Uuid, prefix: &str) -> Result<Vec<DraftInfo>, String> {
        let drafts = self.drafts.lock().unwrap();
        Ok(drafts
            .iter()
            .filter(|(b, _)| b.starts_with(prefix))
            .map(|(branch, _)| DraftInfo {
                branch: branch.clone(),
                commit: "c1".into(),
                author: "Map job <map@example.org>".into(),
                at: "2026-10-31".into(),
            })
            .collect())
    }
    async fn read(&self, _: Uuid, branch: &str, _: &str) -> Result<Option<DraftFiles>, String> {
        let drafts = self.drafts.lock().unwrap();
        Ok(drafts
            .iter()
            .find(|(b, _)| b == branch)
            .map(|(_, rules)| DraftFiles {
                commit: "c1".into(),
                mapping: MAPPING.into(),
                rules: vec![("ClientId.yaml".into(), rules.clone())],
            }))
    }
    async fn approve(
        &self,
        _: Uuid,
        branch: &str,
        _: &str,
        author: &str,
    ) -> Result<Approval, String> {
        assert!(
            author.contains('<') && author.ends_with('>'),
            "Name <email>: {author}"
        );
        if self.stale.lock().unwrap().iter().any(|b| b == branch) {
            return Ok(Approval::Stale);
        }
        let mut drafts = self.drafts.lock().unwrap();
        let Some(at) = drafts.iter().position(|(b, _)| b == branch) else {
            return Ok(Approval::NotFound);
        };
        drafts.remove(at);
        self.merged.lock().unwrap().push(branch.into());
        Ok(Approval::Merged("m1".into()))
    }
    async fn discard(&self, _: Uuid, branch: &str) -> Result<bool, String> {
        let mut drafts = self.drafts.lock().unwrap();
        let before = drafts.len();
        drafts.retain(|(b, _)| b != branch);
        Ok(drafts.len() < before)
    }
    async fn save_file(
        &self,
        _: Uuid,
        _: &quality::draft_api::FileEdit<'_>,
        _: bool,
    ) -> Result<quality::draft_api::FileSaved, String> {
        unreachable!("no edits here")
    }
}

/// May view and approve `loans`' rules and manage it; `cards`' rules it
/// may only view; or nothing.
struct Steward {
    approve: bool,
}
impl Access for Steward {
    fn can(&self, action: &str, resource: &str) -> bool {
        match action {
            "quality:ViewRules" => resource == "catalog/loans",
            "quality:ApproveRules" => self.approve && resource == "catalog/loans",
            "quality:ManageDatasets" => resource == "dataset/loans",
            _ => false,
        }
    }
    fn can_some(&self, action: &str) -> bool {
        self.can(action, "catalog/loans")
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
    router: Router,
    log: Arc<Log>,
}

fn fixture() -> Fixture {
    let memory = Arc::new(Memory::default());
    {
        let mut drafts = memory.drafts.lock().unwrap();
        drafts.push(("proposal/loans/a1".into(), GOOD.into()));
        drafts.push(("proposal/loans/b2".into(), BAD.into()));
        drafts.push(("proposal/loans/c3".into(), GOOD.into()));
        drafts.push(("proposal/cards/d4".into(), GOOD.into()));
    }
    memory
        .stale
        .lock()
        .unwrap()
        .push("proposal/loans/c3".into());
    let router = QualityModule::default()
        .with_drafts(memory.clone())
        .user_router();
    Fixture {
        memory,
        router,
        log: Arc::new(Log::default()),
    }
}

impl Fixture {
    async fn call(&self, approve: bool, method: &str, uri: &str) -> (StatusCode, Value) {
        let caller = Caller::new(
            "00000000-0000-0000-0000-0000000000a1".into(),
            None,
            "192.0.2.1".into(),
            None,
            Arc::new(Steward { approve }),
            self.log.clone(),
        );
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let res = self
            .router
            .clone()
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
}

#[tokio::test]
async fn drafts_are_listed_and_read_with_their_lint_errors() {
    let f = fixture();
    let (status, listed) = f.call(true, "GET", "/quality/drafts").await;
    assert_eq!(status, StatusCode::OK);
    let branches: Vec<_> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["branch"].as_str().unwrap())
        .collect();
    assert_eq!(
        branches,
        [
            "proposal/loans/a1",
            "proposal/loans/b2",
            "proposal/loans/c3"
        ],
        "not cards'"
    );

    let (status, good) = f.call(true, "GET", "/quality/drafts/loans/a1").await;
    assert_eq!(
        (status, good["mapping_reads"].as_bool()),
        (StatusCode::OK, Some(true))
    );
    assert_eq!(good["rules"][0]["errors"], Value::Array(Vec::new()));
    let (_, bad) = f.call(true, "GET", "/quality/drafts/loans/b2").await;
    assert!(
        !bad["rules"][0]["errors"].as_array().unwrap().is_empty(),
        "an unknown field"
    );

    for (uri, expected) in [
        ("/quality/drafts/cards/d4", StatusCode::FORBIDDEN),
        ("/quality/drafts/loans/zz", StatusCode::NOT_FOUND),
        ("/quality/drafts/loans/a..b", StatusCode::NOT_FOUND),
    ] {
        assert_eq!(f.call(true, "GET", uri).await.0, expected, "{uri}");
    }
}

#[tokio::test]
async fn a_draft_is_approved_only_if_it_lints_and_is_not_stale_audited() {
    let f = fixture();
    // Without the right to approve rules: not even looked at.
    assert_eq!(
        f.call(false, "POST", "/quality/drafts/loans/a1/approve")
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Its rules do not lint: refused, with what to fix; not merged.
    let (status, out) = f
        .call(true, "POST", "/quality/drafts/loans/b2/approve")
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!out["rules"][0]["errors"].as_array().unwrap().is_empty());
    // Stale: main changed the dataset since.
    let (status, out) = f
        .call(true, "POST", "/quality/drafts/loans/c3/approve")
        .await;
    assert_eq!(
        (status, out["error"].as_str()),
        (StatusCode::CONFLICT, Some("stale"))
    );

    let (status, out) = f
        .call(true, "POST", "/quality/drafts/loans/a1/approve")
        .await;
    assert_eq!(
        (status, out["commit"].as_str()),
        (StatusCode::OK, Some("m1"))
    );
    assert_eq!(*f.memory.merged.lock().unwrap(), ["proposal/loans/a1"]);
    assert_eq!(
        f.call(true, "POST", "/quality/drafts/loans/a1/approve")
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // Discarded: gone.
    assert_eq!(
        f.call(true, "DELETE", "/quality/drafts/loans/b2").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.call(true, "DELETE", "/quality/drafts/loans/b2").await.0,
        StatusCode::NOT_FOUND
    );
    let log = f.log.0.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .map(|e| (e.resource.as_str(), e.change))
            .collect::<Vec<_>>(),
        [
            ("catalog/loans/draft/a1", Change::Approved),
            ("catalog/loans/draft/b2", Change::Deleted)
        ]
    );
}
