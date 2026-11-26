use std::sync::Mutex;

use super::*;
use crate::updates::tests::MemoryMaintenance;
use crate::updates::InstalledModule;

/// Two fake modules: one refuses, one does not answer; what each was asked.
#[derive(Default)]
struct Fake {
    asked: Mutex<Vec<(String, String, String)>>,
}

#[async_trait]
impl RemovalChecks for Fake {
    async fn ask(
        &self,
        endpoints: &[RemovalEndpoint],
        removed: &str,
    ) -> Vec<(String, Result<RemovalAnswer, String>)> {
        endpoints
            .iter()
            .map(|e| {
                let call = (e.module.clone(), e.path.clone(), removed.to_string());
                self.asked.lock().unwrap().push(call);
                let answer = match e.module.as_str() {
                    "orchestrator" if removed == "engine-trino" => Ok(RemovalAnswer {
                        refuse: true,
                        reason: Some("flows are pinned to its line".into()),
                        counts: [("flows".to_string(), 3), ("runs".to_string(), 1)].into(),
                    }),
                    "orchestrator" => Ok(RemovalAnswer {
                        refuse: false,
                        reason: None,
                        counts: BTreeMap::new(),
                    }),
                    "garbled" => Ok(RemovalAnswer {
                        refuse: true,
                        reason: None,
                        counts: [("Flows".to_string(), 1)].into(),
                    }),
                    _ => Err("no answer within 5s".into()),
                };
                (e.module.clone(), answer)
            })
            .collect()
    }
}

fn module(name: &str, removal_path: Option<&str>) -> InstalledModule {
    serde_json::from_value(serde_json::json!({
        "name": name, "version": "1.0.0", "removal_path": removal_path,
    }))
    .unwrap()
}

async fn service(modules: Vec<InstalledModule>, fake: Arc<Fake>) -> UpdatesService {
    let service =
        UpdatesService::new(Arc::new(MemoryMaintenance::default())).with_removal_checks(fake);
    service.set_modules(modules).await.unwrap();
    service
}

#[tokio::test]
async fn every_declaring_module_is_asked_and_a_silent_one_refuses() {
    let fake = Arc::new(Fake::default());
    let modules = vec![
        module("orchestrator", Some("/internal/removal")),
        module("engine-trino", Some("/internal/removal")),
        module("docs", None),
    ];
    let service = service(modules, fake.clone()).await;
    let refusals = service.removal_refusals("engine-trino").await.unwrap();
    assert_eq!(refusals.len(), 2, "{refusals:?}");
    assert_eq!(refusals[0].module, "orchestrator");
    assert_eq!(refusals[0].reason, "flows are pinned to its line");
    assert_eq!(refusals[0].counts["flows"], 3);
    assert_eq!(refusals[1].module, "engine-trino", "silent: refuses");
    assert!(refusals[1].counts.is_empty());
    let asked = fake.asked.lock().unwrap().clone();
    let expected = |m: &str| {
        (
            m.to_string(),
            "/internal/removal".to_string(),
            "engine-trino".to_string(),
        )
    };
    assert_eq!(
        asked,
        [expected("orchestrator"), expected("engine-trino")],
        "docs declares none"
    );
}

#[tokio::test]
async fn no_refusal_lets_it_go_and_none_declared_asks_nobody() {
    let fake = Arc::new(Fake::default());
    let service = service(vec![module("orchestrator", Some("/r"))], fake.clone()).await;
    assert!(service.removal_refusals("docs").await.unwrap().is_empty());
    let none = service_without_paths().await;
    assert!(none.removal_refusals("docs").await.unwrap().is_empty());
}

async fn service_without_paths() -> UpdatesService {
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    service
        .set_modules(vec![module("docs", None)])
        .await
        .unwrap();
    service
}

#[tokio::test]
async fn an_answer_that_does_not_read_refuses_and_a_bad_name_is_invalid() {
    let fake = Arc::new(Fake::default());
    let service = service(vec![module("garbled", Some("/r"))], fake).await;
    let refusals = service.removal_refusals("docs").await.unwrap();
    assert_eq!(refusals.len(), 1);
    assert!(
        refusals[0].counts.is_empty(),
        "its counts are not passed on"
    );
    let bad = service.removal_refusals("../etc").await;
    assert!(matches!(bad, Err(UpdatesError::Invalid("module", _))));
}

#[tokio::test]
async fn a_bad_removal_path_is_refused() {
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    for path in ["removal", "/../x", "/r?x=1"] {
        let refused = service
            .set_modules(vec![module("orchestrator", Some(path))])
            .await;
        assert!(matches!(refused, Err(UpdatesError::Invalid(..))), "{path}");
    }
}
