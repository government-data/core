//! A dataset removed from `main` on an ephemeral Dolt (decision 0055): its
//! mapping and rules go as one commit by whoever removed it, another
//! dataset stays, and a second removal changes nothing. Synthetic rules
//! only.

use infrastructure::catalog_dolt::{DatasetImport, DoltCatalog};

fn mapping(dataset: &str) -> String {
    format!("dataset: {dataset}\nsource: {dataset}\nkey: [id]\nfields:\n  id: {{ column: ID, type: text }}\n")
}

#[tokio::test]
async fn a_dataset_leaves_main_as_one_commit_and_another_stays() {
    let dolt = testkit::spawn_dolt().await;
    dolt.create_database("tenant_a").await;
    let catalog = DoltCatalog::new(&dolt.url);
    for dataset in ["loans", "cards"] {
        let rules = vec![(
            "id.yaml".to_string(),
            format!(
                "dataset: {dataset}\nrules:\n  - id: id-01\n    field: id\n    check: required\n"
            ),
        )];
        let m = mapping(dataset);
        let import = DatasetImport {
            dataset,
            description: None,
            mapping: &m,
            rules: &rules,
        };
        catalog
            .import("tenant_a", &import, "Test <test@example.org>")
            .await
            .unwrap();
    }

    assert!(catalog
        .remove("tenant_a", "loans", "Ana <ana@example.org>")
        .await
        .unwrap());
    assert!(
        catalog.files("tenant_a", "loans").await.unwrap().is_none(),
        "its mapping and rules gone"
    );
    assert!(
        catalog.files("tenant_a", "cards").await.unwrap().is_some(),
        "another stays"
    );
    let names: Vec<String> = catalog
        .mappings("tenant_a")
        .await
        .unwrap()
        .into_iter()
        .map(|(d, _)| d)
        .collect();
    assert_eq!(names, ["cards"]);
    assert!(
        !catalog
            .remove("tenant_a", "loans", "Ana <ana@example.org>")
            .await
            .unwrap(),
        "nothing left to remove"
    );
}
