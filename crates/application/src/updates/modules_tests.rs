use std::sync::Arc;

use super::*;
use crate::updates::tests::MemoryMaintenance;

fn module(name: &str, drain_path: Option<&str>) -> InstalledModule {
    InstalledModule {
        name: name.into(),
        version: "1.0.0".into(),
        drain_path: drain_path.map(Into::into),
        route_path: None,
        menu: None,
        permissions: None,
        provides: Vec::new(),
        roles: Vec::new(),
        removal_path: None,
        events: None,
    }
}

#[tokio::test]
async fn a_modules_roles_are_kept_and_a_bad_list_keeps_the_one_before() {
    use crate::updates::module_roles::tests::{file, role};
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    let mut lake = role("lake");
    lake.files = vec![file("compose.yml", b"services: {}\n")];
    let kept = vec![InstalledModule {
        roles: vec![lake],
        ..module("lake-rustfs", None)
    }];
    service.set_modules(kept.clone()).await.unwrap();
    assert_eq!(service.installed_modules().await.unwrap(), kept);
    let refused = service
        .set_modules(vec![InstalledModule {
            roles: vec![role("app")],
            ..module("lake-rustfs", None)
        }])
        .await;
    assert!(matches!(refused, Err(UpdatesError::Invalid("modules", _))));
    assert_eq!(service.installed_modules().await.unwrap(), kept, "kept");
    let older = serde_json::json!({"name": "docs", "version": "1.0.0"});
    let older: InstalledModule = serde_json::from_value(older).unwrap();
    assert!(older.roles.is_empty(), "an agent that does not send them");
}

#[tokio::test]
async fn a_module_says_the_capabilities_it_provides() {
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    let lake = InstalledModule {
        provides: vec!["lake".into()],
        ..module("lake-rustfs", None)
    };
    service.set_modules(vec![lake.clone()]).await.unwrap();
    assert_eq!(service.installed_modules().await.unwrap(), [lake]);
    let unread = serde_json::json!({"name": "docs", "version": "1.0.0"});
    let older: InstalledModule = serde_json::from_value(unread).unwrap();
    assert!(older.provides.is_empty(), "an agent that does not say them");
    for bad in [vec!["Lake".to_string()], vec!["x".to_string(); 17]] {
        let module = InstalledModule {
            provides: bad,
            ..module("lake-rustfs", None)
        };
        let refused = service.set_modules(vec![module]).await;
        assert!(
            matches!(refused, Err(UpdatesError::Invalid(..))),
            "{refused:?}"
        );
    }
}

#[tokio::test]
async fn the_whole_list_replaces_the_one_before() {
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    assert!(service.installed_modules().await.unwrap().is_empty());
    let first = vec![
        module("quality-data", None),
        module("orchestrator", Some("/internal/drain")),
    ];
    service.set_modules(first.clone()).await.unwrap();
    service.set_modules(first.clone()).await.unwrap();
    assert_eq!(service.installed_modules().await.unwrap(), first);
    service
        .set_modules(vec![module("quality-data", None)])
        .await
        .unwrap();
    assert_eq!(
        service.installed_modules().await.unwrap(),
        [module("quality-data", None)]
    );
}

#[tokio::test]
async fn a_bad_module_is_refused_and_nothing_changes() {
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    let kept = vec![module("orchestrator", Some("/drain"))];
    service.set_modules(kept.clone()).await.unwrap();
    let mut no_version = module("grafana", None);
    no_version.version = " ".into();
    for bad in [
        vec![module("Orchestrator", None)],
        vec![module("", None)],
        vec![module("a/b", None)],
        vec![module("orchestrator", None), module("orchestrator", None)],
        vec![no_version],
        vec![module("orchestrator", Some("drain"))],
        vec![module("orchestrator", Some("/"))],
        vec![module("orchestrator", Some("/../api/drain"))],
        vec![module("orchestrator", Some("/drain?x=1"))],
        vec![module("orchestrator", Some("//other/drain"))],
    ] {
        let refused = service.set_modules(bad.clone()).await;
        assert!(
            matches!(refused, Err(UpdatesError::Invalid("modules", _))),
            "{bad:?}"
        );
    }
    assert_eq!(service.installed_modules().await.unwrap(), kept);
}

#[tokio::test]
async fn a_removed_modules_permissions_are_kept() {
    let service = UpdatesService::new(Arc::new(MemoryMaintenance::default()));
    let mut bi = module("bi-superset", None);
    bi.permissions = Some(ModulePermissions::default());
    service.set_modules(vec![bi]).await.unwrap();
    let declared = service.declared_modules().await.unwrap();
    assert_eq!(
        (declared[0].name.as_str(), declared[0].installed),
        ("bi-superset", true)
    );
    service.set_modules(vec![]).await.unwrap();
    let declared = service.declared_modules().await.unwrap();
    assert_eq!(declared.len(), 1);
    assert!(!declared[0].installed, "removed, kept");
}
