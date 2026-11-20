//! Drafts of one column each on an ephemeral Dolt (decision 0050): two
//! columns' drafts are approved one after the other, neither stale for the
//! other; a column's draft whose file `main` changed since is; each
//! column's history on `main` is the commits that changed its file.
//! Synthetic rules only.

use infrastructure::catalog_dolt::{DatasetImport, DoltCatalog, FileEdit, FileSaved, Merged};

const MAPPING: &str = "dataset: loans\nsource: loans\nkey: [id]\nfields:\n  \
                       id: { column: ID, type: text }\n  amount: { column: AMT, type: number }\n";

fn file(field: &str, check: &str) -> String {
    format!("dataset: loans\nrules:\n  - id: {field}-01\n    field: {field}\n    check: {check}\n")
}

async fn save(catalog: &DoltCatalog, branch: &str, name: &str, body: &str, create: bool) {
    let edit = FileEdit {
        branch,
        dataset: "loans",
        file: name,
        body,
        author: "Ana <ana@example.org>",
        adds: false,
    };
    let saved = catalog.save_file("tenant_a", &edit, create).await.unwrap();
    assert!(matches!(saved, FileSaved::Saved(_)), "{saved:?}");
}

async fn approve(catalog: &DoltCatalog, branch: &str) -> Merged {
    catalog
        .approve(
            "tenant_a",
            branch,
            "loans",
            "Rev <rev@example.org>",
            "approve",
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn columns_drafts_are_approved_alone_and_stale_only_by_their_own_file() {
    let dolt = testkit::spawn_dolt().await;
    dolt.create_database("tenant_a").await;
    let catalog = DoltCatalog::new(&dolt.url);
    let rules = vec![
        ("id.yaml".to_string(), file("id", "required")),
        ("amount.yaml".to_string(), file("amount", "required")),
    ];
    let import = DatasetImport {
        dataset: "loans",
        description: None,
        mapping: MAPPING,
        rules: &rules,
    };
    catalog
        .import("tenant_a", &import, "Test <test@example.org>")
        .await
        .unwrap();

    // Two columns, two drafts, both from the same main.
    save(
        &catalog,
        "proposal/loans/id",
        "id.yaml",
        &file("id", "unique"),
        true,
    )
    .await;
    save(
        &catalog,
        "proposal/loans/amount",
        "amount.yaml",
        &file("amount", "unique"),
        true,
    )
    .await;
    // A third on id, from that main too.
    save(
        &catalog,
        "proposal/loans/id2",
        "id.yaml",
        &file("id", "required"),
        true,
    )
    .await;

    // id's is approved; amount's is not stale for it, and goes in after.
    assert!(matches!(
        approve(&catalog, "proposal/loans/id").await,
        Merged::Merged(_)
    ));
    assert!(matches!(
        approve(&catalog, "proposal/loans/amount").await,
        Merged::Merged(_)
    ));
    let main = catalog.files("tenant_a", "loans").await.unwrap().unwrap();
    assert_eq!(
        main.rules,
        vec![
            ("amount.yaml".to_string(), file("amount", "unique")),
            ("id.yaml".to_string(), file("id", "unique")),
        ]
    );

    // The other draft of id left a main whose id.yaml has changed since.
    assert_eq!(approve(&catalog, "proposal/loans/id2").await, Merged::Stale);

    // id.yaml's history: its approval, then the import; amount's approval
    // is not in it. The mapping's: the import only.
    let history = catalog
        .file_history("tenant_a", "loans", Some("id.yaml"), 50)
        .await
        .unwrap();
    let seen: Vec<_> = history
        .iter()
        .map(|c| (c.before.clone(), c.after.clone()))
        .collect();
    assert_eq!(
        seen,
        [
            (Some(file("id", "required")), Some(file("id", "unique"))),
            (None, Some(file("id", "required"))),
        ]
    );
    assert!(history[0].commit.author.contains("rev@example.org"));
    let mapping = catalog
        .file_history("tenant_a", "loans", None, 50)
        .await
        .unwrap();
    assert_eq!(mapping.len(), 1);
    assert_eq!(mapping[0].after.as_deref(), Some(MAPPING));
}

#[tokio::test]
async fn a_columns_file_is_deployed_to_main_as_one_commit_in_its_history() {
    // Decision 0051: saved on `main` itself, no draft made.
    let dolt = testkit::spawn_dolt().await;
    dolt.create_database("tenant_a").await;
    let catalog = DoltCatalog::new(&dolt.url);
    let rules = vec![("id.yaml".to_string(), file("id", "required"))];
    let import = DatasetImport {
        dataset: "loans",
        description: None,
        mapping: MAPPING,
        rules: &rules,
    };
    catalog
        .import("tenant_a", &import, "Test <test@example.org>")
        .await
        .unwrap();
    save(&catalog, "main", "id.yaml", &file("id", "unique"), false).await;
    let edit = FileEdit {
        branch: "main",
        dataset: "loans",
        file: "amount.yaml",
        body: &file("amount", "required"),
        author: "Ana <ana@example.org>",
        adds: true,
    };
    let saved = catalog.save_file("tenant_a", &edit, false).await.unwrap();
    assert!(matches!(saved, FileSaved::Saved(_)));
    let main = catalog.files("tenant_a", "loans").await.unwrap().unwrap();
    assert_eq!(main.rules.len(), 2);
    assert!(catalog
        .drafts("tenant_a", "proposal/")
        .await
        .unwrap()
        .is_empty());
    let history = catalog
        .file_history("tenant_a", "loans", Some("id.yaml"), 50)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[0].after.as_deref(),
        Some(file("id", "unique").as_str())
    );
    assert!(history[0].commit.author.contains("ana@example.org"));
}
